use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures_util::stream::{self, StreamExt};
use tokio::sync::mpsc;

use crate::client::DiscordClient;
use crate::config::ContentFilter;
use crate::error::DiscordError;
use crate::error::DiscordResult;
use crate::events::{ChannelStats, LogLevel, Phase, PrunerEvent};
use crate::history::{text_preview, AttachmentMeta};
use crate::model::{Channel, Message};
use crate::util::truncate;
use serde_json::Value;

use super::dates::{datetime_to_snowflake, message_time};
use super::filters::should_delete_message;
use super::resolve::ResolvedTargets;
use super::{ItemKind, Plan, PlannedDelete};

// Discord caps history `limit` at 100; 100 keeps round-trips minimal.
const MESSAGE_PAGE_SIZE: u32 = 100;
const GUILD_SEARCH_PAGE_SIZE: usize = 25;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanLimits {
    pub emit_channel_events: bool,
    pub concurrency: u32,
}

impl ScanLimits {
    pub fn first_pass(concurrency: u32) -> Self {
        Self {
            emit_channel_events: true,
            concurrency: concurrency.max(1),
        }
    }

    pub fn verify_pass(concurrency: u32) -> Self {
        Self {
            emit_channel_events: false,
            concurrency: concurrency.max(1),
        }
    }
}

pub(crate) struct ScanCtx<'a> {
    pub client: &'a DiscordClient,
    pub before: Option<DateTime<Utc>>,
    pub after: Option<DateTime<Utc>>,
    pub content_filter: ContentFilter,
    pub keep_pinned: bool,
    pub limits: ScanLimits,
    pub cancel: &'a Arc<AtomicBool>,
    pub tx: &'a mpsc::Sender<PrunerEvent>,
}

impl ScanCtx<'_> {
    async fn channel_started(&self, id: &str, name: &str, index: usize, total: usize) {
        if !self.limits.emit_channel_events {
            return;
        }
        let _ = self
            .tx
            .send(PrunerEvent::ChannelStarted {
                channel_id: id.to_string(),
                channel_name: name.to_string(),
                index,
                total,
                phase: Phase::Scan,
            })
            .await;
    }

    async fn channel_done(&self, id: &str, scanned: u64, added: u64, failed: u64) {
        if !self.limits.emit_channel_events {
            return;
        }
        let _ = self
            .tx
            .send(PrunerEvent::ChannelDone {
                channel_id: id.to_string(),
                stats: ChannelStats {
                    scanned,
                    planned: added,
                    deleted: 0,
                    skipped: scanned.saturating_sub(added),
                    failed,
                },
                phase: Phase::Scan,
            })
            .await;
    }

    fn walker(&self) -> PageWalker<'_> {
        PageWalker {
            limits: self.limits,
            cancel: self.cancel.as_ref(),
            tx: self.tx,
        }
    }
}

#[derive(Default)]
struct PassOutcome {
    scanned: u64,
    added: u64,
    failed: u64,
    entries: Vec<PlannedDelete>,
}

struct PageLog<'a> {
    label: &'a str,
    error: String,
    suffix: &'a str,
    emit_progress: bool,
}

/// The slice of `ScanCtx` that a page walk needs, so the walk can be driven
/// without a client.
struct PageWalker<'a> {
    limits: ScanLimits,
    cancel: &'a AtomicBool,
    tx: &'a mpsc::Sender<PrunerEvent>,
}

impl PageWalker<'_> {
    async fn warn(&self, message: String) {
        let _ = self
            .tx
            .send(PrunerEvent::Log {
                level: LogLevel::Warning,
                message,
            })
            .await;
    }

    async fn emit_progress(&self, log: &PageLog<'_>, pages: u64, out: &PassOutcome) {
        if !log.emit_progress || !self.limits.emit_channel_events {
            return;
        }
        let _ = self
            .tx
            .send(PrunerEvent::Log {
                level: LogLevel::Info,
                message: format!(
                    "···  {}  p{pages}  {} seen · {} {}",
                    log.label, out.scanned, out.added, log.suffix
                ),
            })
            .await;
    }

    async fn walk<T, F, Fut, V>(
        &self,
        limit: usize,
        seed: PassOutcome,
        start: Option<String>,
        log: &PageLog<'_>,
        mut fetch: F,
        mut visit: V,
    ) -> PassOutcome
    where
        F: FnMut(Option<String>) -> Fut,
        Fut: Future<Output = DiscordResult<(Vec<T>, Option<String>)>> + Send,
        V: FnMut(&[T], &mut PassOutcome) -> bool,
    {
        let mut out = seed;
        let mut cursor = start;
        let mut pages = 0u64;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            if limit > 0 && out.entries.len() >= limit {
                break;
            }
            let (items, next) = match fetch(cursor.clone()).await {
                Ok(page) => page,
                Err(DiscordError::Cancelled) => break,
                Err(e) => {
                    out.failed += 1;
                    self.warn(format!("{}: {e}", log.error)).await;
                    break;
                }
            };
            if items.is_empty() {
                break;
            }
            pages = pages.saturating_add(1);
            let stop = visit(&items, &mut out);
            self.emit_progress(log, pages, &out).await;
            if stop {
                break;
            }
            match next {
                Some(n) if !n.is_empty() && Some(n.clone()) != cursor => cursor = Some(n),
                _ => break,
            }
        }

        out
    }
}

pub(crate) async fn build_plan(ctx: &ScanCtx<'_>, scope: &ResolvedTargets, limit: usize) -> Plan {
    let mut plan = Plan::default();

    // 1. Use indexed search as an efficient first pass for guild messages.
    if !scope.guilds.is_empty() {
        merge_plan(&mut plan, search_guilds(ctx, scope, limit).await);
    }

    // 2. Walk channel histories too. Indexed guild search can omit messages, so it
    //    supplements the channel history scan instead of replacing it.
    let channels_to_scan: Vec<&Channel> = scope.channels.iter().collect();

    let remaining = if limit == 0 {
        0
    } else {
        limit.saturating_sub(plan.entries.len())
    };
    if !channels_to_scan.is_empty() && (limit == 0 || remaining > 0) {
        let channel_plan =
            scan_channels(ctx, &channels_to_scan, remaining, 0, channels_to_scan.len()).await;
        merge_plan(&mut plan, channel_plan);
    }

    if limit > 0 {
        plan.entries.truncate(limit);
    }
    plan
}

fn merge_plan(plan: &mut Plan, extra: Plan) {
    let mut seen: HashSet<(String, String)> = plan
        .entries
        .iter()
        .map(|entry| (entry.target_id.clone(), entry.item_id.clone()))
        .collect();
    plan.entries.extend(
        extra
            .entries
            .into_iter()
            .filter(|entry| seen.insert((entry.target_id.clone(), entry.item_id.clone()))),
    );
    plan.messages_scanned = plan.messages_scanned.saturating_add(extra.messages_scanned);
    plan.channels_processed = plan
        .channels_processed
        .saturating_add(extra.channels_processed);
}

async fn scan_channels(
    ctx: &ScanCtx<'_>,
    channels: &[&Channel],
    limit: usize,
    index_base: usize,
    total: usize,
) -> Plan {
    let mut plan = Plan::default();
    if limit == 0 && channels.len() > 1 {
        let owned: Vec<_> = channels.iter().map(|channel| (*channel).clone()).collect();
        scan_channels_parallel(ctx, &owned, total, index_base, &mut plan).await;
        return plan;
    }
    for (offset, channel) in channels.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) || (limit > 0 && plan.entries.len() >= limit) {
            break;
        }
        let id = channel.id();
        let name = channel.display_name();
        ctx.channel_started(&id, &name, index_base + offset, total)
            .await;
        let remaining = limit.saturating_sub(plan.entries.len());
        let outcome = scan_channel(ctx, channel, remaining).await;
        plan.entries.extend(outcome.entries);
        plan.messages_scanned = plan.messages_scanned.saturating_add(outcome.scanned);
        plan.channels_processed += 1;
        ctx.channel_done(&id, outcome.scanned, outcome.added, outcome.failed)
            .await;
    }
    plan
}

async fn search_guilds(ctx: &ScanCtx<'_>, scope: &ResolvedTargets, limit: usize) -> Plan {
    let mut plan = Plan::default();
    let uid = ctx.client.user_id();
    let uname = ctx.client.username();
    let channel_names: HashMap<String, String> = scope
        .channels
        .iter()
        .map(|c| (c.id(), c.display_name()))
        .collect();
    let guild_channel_ids: HashSet<String> = scope
        .channels
        .iter()
        .filter(|channel| channel.guild_id.is_some())
        .map(Channel::id)
        .collect();
    let mut seen = HashSet::new();

    let initial_max_id = ctx
        .before
        .map(|d| datetime_to_snowflake(&d, false).to_string());
    let min_id_str = ctx
        .after
        .map(|d| datetime_to_snowflake(&d, true).to_string());

    for (guild_id, guild_name) in &scope.guilds {
        let mut current_max_id = initial_max_id.clone();
        let mut guild_plan = Plan::default();
        let mut total_results: Option<u64> = None;
        let mut total_fetched: u64 = 0;

        loop {
            if ctx.cancel.load(Ordering::Relaxed)
                || (limit > 0 && plan.entries.len() + guild_plan.entries.len() >= limit)
            {
                break;
            }
            let mut params = vec![("author_id", uid.as_str()), ("limit", "25")];
            if let Some(max_id) = &current_max_id {
                params.push(("max_id", max_id.as_str()));
            }
            if let Some(min_id) = &min_id_str {
                params.push(("min_id", min_id.as_str()));
            }
            let payload = match ctx.client.search_guild_messages(guild_id, &params).await {
                Ok(value) => value,
                Err(error) => {
                    let _ = ctx
                        .tx
                        .send(PrunerEvent::Log {
                            level: LogLevel::Warning,
                            message: format!(
                                "guild {guild_id} ({guild_name}) indexed search failed: {error}; channel history will still be scanned"
                            ),
                        })
                        .await;
                    break;
                }
            };
            // In discord.py-self, payload["messages"] is a List[List[Message]].
            let raw_messages = payload.get("messages").and_then(Value::as_array);
            let Some(raw_messages) = raw_messages else {
                let _ = ctx
                    .tx
                    .send(PrunerEvent::Log {
                        level: LogLevel::Warning,
                        message: format!(
                            "guild {guild_id} ({guild_name}) search response missing 'messages' array; channel history will still be scanned"
                        ),
                    })
                    .await;
                break;
            };

            if total_results.is_none() {
                total_results = payload.get("total_results").and_then(Value::as_u64);
            }

            // Search results group each target message with its surrounding context.
            let page: Vec<Message> = raw_messages
                .iter()
                .filter_map(|entry| {
                    let target = if let Some(arr) = entry.as_array() {
                        arr.first()?
                    } else {
                        entry
                    };
                    serde_json::from_value(target.clone()).ok()
                })
                .collect();

            if page.is_empty() {
                break;
            }

            total_fetched += raw_messages.len() as u64;
            guild_plan.messages_scanned += page.len() as u64;

            for message in &page {
                if message.author_id() != uid
                    || !should_delete_message(
                        message,
                        &uid,
                        ctx.before,
                        ctx.after,
                        ctx.content_filter,
                        ctx.keep_pinned,
                    )
                {
                    continue;
                }
                let channel_id = message.channel_id.clone();
                if channel_id.is_empty()
                    || !guild_channel_ids.contains(&channel_id)
                    || !seen.insert((channel_id.clone(), message.message_id()))
                {
                    continue;
                }
                let channel_name = channel_names
                    .get(&channel_id)
                    .cloned()
                    .unwrap_or_else(|| format!("#{guild_name}"));
                guild_plan.entries.push(PlannedDelete::from_message(
                    message,
                    &channel_id,
                    &channel_name,
                    &uid,
                    &uname,
                ));
                if limit > 0 && plan.entries.len() + guild_plan.entries.len() >= limit {
                    break;
                }
            }

            // Cursor pagination: advance `max_id` to the oldest message in this batch,
            // exactly like discord.py-self's `state = Object(id=int(data['messages'][-1][0]['id']))`.
            if let Some(last_msg) = page.last() {
                let last_id = last_msg.message_id();
                if current_max_id.as_deref() == Some(&last_id) {
                    break;
                }
                current_max_id = Some(last_id);
            } else {
                break;
            }

            // If we received fewer messages than the page size (25), or reached total_results, we are done.
            if raw_messages.len() < GUILD_SEARCH_PAGE_SIZE {
                break;
            }
            if let Some(total) = total_results {
                if total_fetched >= total {
                    break;
                }
            }
        }

        if ctx.cancel.load(Ordering::Relaxed) {
            merge_plan(&mut plan, guild_plan);
            break;
        }
        merge_plan(&mut plan, guild_plan);
    }
    plan
}

async fn scan_channels_parallel(
    ctx: &ScanCtx<'_>,
    channels: &[Channel],
    total: usize,
    index_base: usize,
    plan: &mut Plan,
) {
    let _ = ctx
        .tx
        .send(PrunerEvent::Log {
            level: LogLevel::Info,
            message: format!(
                "SCAN  {} channel(s) ×{} parallel · full history · own only",
                channels.len(),
                ctx.limits.concurrency
            ),
        })
        .await;

    let mut results: Vec<(usize, PassOutcome)> = stream::iter(channels.iter().cloned().enumerate())
        .map(|(offset, channel)| {
            let index = index_base + offset;
            async move {
                let cid = channel.id();
                let cname = channel.display_name();
                ctx.channel_started(&cid, &cname, index, total).await;
                let out = scan_channel(ctx, &channel, 0).await;
                ctx.channel_done(&cid, out.scanned, out.added, out.failed)
                    .await;
                (index, out)
            }
        })
        .buffer_unordered(ctx.limits.concurrency as usize)
        .collect()
        .await;

    results.sort_by_key(|(index, _)| *index);
    for (_index, out) in results {
        plan.entries.extend(out.entries);
        plan.messages_scanned += out.scanned;
        plan.channels_processed += 1;
    }
}

async fn scan_channel(ctx: &ScanCtx<'_>, channel: &Channel, remaining_limit: usize) -> PassOutcome {
    let client = ctx.client;
    let cid = channel.id();
    let cid_ref = cid.as_str();
    let cname = channel.display_name();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let out = PassOutcome::default();

    let label = truncate(&cname, 28);

    ctx.walker()
        .walk::<Message, _, _, _>(
            remaining_limit,
            out,
            None,
            &PageLog {
                label: label.as_str(),
                error: format!("channel {cid} page failed"),
                suffix: "own",
                emit_progress: true,
            },
            move |cursor: Option<String>| async move {
                let page = client
                    .channel_messages(cid_ref, MESSAGE_PAGE_SIZE, cursor.as_deref(), None, None)
                    .await?;
                // Discord returns newest-first; next page walks `before` the oldest id.
                let next = page.last().map(|m| m.id.clone()).filter(|s| !s.is_empty());
                Ok::<_, DiscordError>((page, next))
            },
            |items: &[Message], out: &mut PassOutcome| {
                let remaining = remaining_limit.saturating_sub(out.added as usize);
                absorb_message_page(ctx, &cid, &cname, items, remaining, &mut seen_ids, out)
            },
        )
        .await
}

fn absorb_message_page(
    ctx: &ScanCtx<'_>,
    cid: &str,
    cname: &str,
    items: &[Message],
    remaining: usize,
    seen_ids: &mut HashSet<String>,
    out: &mut PassOutcome,
) -> bool {
    let mut page_all_old = ctx.after.is_some();
    let mut saw_timestamp = false;
    let mut page_added = 0u64;
    let uid = ctx.client.user_id();
    let uname = ctx.client.username();

    for msg in items {
        out.scanned += 1;
        match message_time(msg) {
            Some(t) => {
                saw_timestamp = true;
                if let Some(a) = ctx.after {
                    if t > a {
                        page_all_old = false;
                    }
                }
            }
            None => page_all_old = false,
        }

        if !should_delete_message(
            msg,
            &uid,
            ctx.before,
            ctx.after,
            ctx.content_filter,
            ctx.keep_pinned,
        ) {
            continue;
        }
        let mid = msg.message_id();
        if !seen_ids.insert(mid) {
            continue;
        }
        out.entries
            .push(PlannedDelete::from_message(msg, cid, cname, &uid, &uname));
        out.added += 1;
        page_added += 1;
        if remaining > 0 && page_added as usize >= remaining {
            break;
        }
    }

    ctx.after.is_some() && page_all_old && saw_timestamp
}

impl PlannedDelete {
    pub fn from_message(
        msg: &Message,
        channel_id: &str,
        channel_name: &str,
        self_id: &str,
        self_username: &str,
    ) -> Self {
        let content = msg.content.clone();
        let preview = text_preview(&content, "message");
        let mut attachments = Vec::new();
        for a in &msg.attachments {
            if a.url.is_empty() {
                continue;
            }
            attachments.push(AttachmentMeta {
                filename: if a.filename.is_empty() {
                    format!("{}.bin", a.id)
                } else {
                    a.filename.clone()
                },
                url: a.url.clone(),
            });
        }
        Self {
            target_id: channel_id.to_string(),
            target_name: channel_name.to_string(),
            item_id: msg.message_id(),
            kind: ItemKind::Message,
            preview: truncate(&preview, 80),
            author_id: self_id.to_string(),
            author_name: self_username.to_string(),
            content,
            timestamp: msg.timestamp.clone(),
            media_type: Some("message".into()),
            attachments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::mpsc;

    use crate::events::PrunerEvent;

    struct WalkHarness {
        limits: ScanLimits,
        tx: mpsc::Sender<PrunerEvent>,
        rx: mpsc::Receiver<PrunerEvent>,
        cancel: AtomicBool,
        calls: Arc<AtomicUsize>,
    }

    impl WalkHarness {
        fn new() -> Self {
            let (tx, rx) = mpsc::channel(64);
            Self {
                limits: ScanLimits {
                    emit_channel_events: true,
                    concurrency: 1,
                },
                tx,
                rx,
                cancel: AtomicBool::new(false),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn walker(&self) -> PageWalker<'_> {
            PageWalker {
                limits: self.limits,
                cancel: &self.cancel,
                tx: &self.tx,
            }
        }

        fn counter(&self) -> Arc<AtomicUsize> {
            Arc::clone(&self.calls)
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }

        fn warnings(&mut self) -> usize {
            use crate::events::LogLevel;
            let mut count = 0;
            while let Ok(event) = self.rx.try_recv() {
                if matches!(
                    event,
                    PrunerEvent::Log {
                        level: LogLevel::Warning,
                        ..
                    }
                ) {
                    count += 1;
                }
            }
            count
        }
    }

    fn page_log() -> PageLog<'static> {
        PageLog {
            label: "test",
            error: "test page failed".into(),
            suffix: "seen",
            emit_progress: false,
        }
    }

    fn count_items(items: &[u32], out: &mut PassOutcome) -> bool {
        out.scanned += items.len() as u64;
        out.added += items.len() as u64;
        false
    }

    #[tokio::test]
    async fn walk_advances_while_the_cursor_keeps_changing() {
        let h = WalkHarness::new();
        let seen = h.counter();
        let out = h
            .walker()
            .walk(
                0,
                PassOutcome::default(),
                None,
                &page_log(),
                move |cursor: Option<String>| {
                    let seen = Arc::clone(&seen);
                    async move {
                        let page = match cursor.as_deref() {
                            None => (vec![1], Some("a".to_string())),
                            Some("a") => (vec![2], Some("b".to_string())),
                            _ => (vec![3], Some("b".to_string())),
                        };
                        seen.fetch_add(1, Ordering::Relaxed);
                        Ok::<_, DiscordError>(page)
                    }
                },
                count_items,
            )
            .await;

        assert_eq!(h.calls(), 3);
        assert_eq!(out.scanned, 3);
        assert_eq!(out.failed, 0);
    }

    #[tokio::test]
    async fn walk_has_no_fixed_page_ceiling() {
        let h = WalkHarness::new();
        let seen = h.counter();
        let out = h
            .walker()
            .walk(
                0,
                PassOutcome::default(),
                None,
                &page_log(),
                move |cursor: Option<String>| {
                    let seen = Arc::clone(&seen);
                    async move {
                        let page = seen.fetch_add(1, Ordering::Relaxed);
                        let next = if page < 600 {
                            Some(format!("cursor-{page}"))
                        } else {
                            None
                        };
                        let _ = cursor;
                        Ok::<_, DiscordError>((vec![1], next))
                    }
                },
                count_items,
            )
            .await;

        assert_eq!(h.calls(), 601);
        assert_eq!(out.scanned, 601);
    }

    #[tokio::test]
    async fn walk_stops_on_an_empty_page() {
        let h = WalkHarness::new();
        let seen = h.counter();
        let out = h
            .walker()
            .walk(
                0,
                PassOutcome::default(),
                None,
                &page_log(),
                move |_cursor: Option<String>| {
                    let seen = Arc::clone(&seen);
                    async move {
                        seen.fetch_add(1, Ordering::Relaxed);
                        Ok::<_, DiscordError>((Vec::<u32>::new(), Some("a".to_string())))
                    }
                },
                count_items,
            )
            .await;

        assert_eq!(h.calls(), 1);
        assert_eq!(out.scanned, 0);
    }

    #[tokio::test]
    async fn walk_stops_when_the_cursor_repeats_without_an_arbitrary_page_cap() {
        let mut h = WalkHarness::new();
        let seen = h.counter();
        let out = h
            .walker()
            .walk(
                0,
                PassOutcome::default(),
                None,
                &page_log(),
                move |_cursor: Option<String>| {
                    let seen = Arc::clone(&seen);
                    async move {
                        let page = seen.fetch_add(1, Ordering::Relaxed);
                        let next = if page < 600 {
                            Some(format!("c{page}"))
                        } else {
                            Some("c599".to_string())
                        };
                        Ok::<_, DiscordError>((vec![1], next))
                    }
                },
                count_items,
            )
            .await;

        assert_eq!(h.calls(), 601);
        assert_eq!(out.scanned, 601);
        assert_eq!(h.warnings(), 0);
    }
}

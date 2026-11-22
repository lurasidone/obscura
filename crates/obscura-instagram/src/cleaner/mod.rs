use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::client::InstagramClient;
use crate::config::DmKind;
use crate::events::{CleanerEvent, LogLevel, RetryCount, RunStats};
use crate::history::HistoryStore;
use crate::ratelimit::RateLimiter;

pub use crate::config::ContentFlags;

pub mod dates;
mod execute;
mod filters;
mod resolve;
mod scan;

#[cfg(test)]
mod tests;

const MAX_VERIFY_PASSES: u8 = 3;

#[derive(Debug, Clone)]
pub enum TargetResolution {
    Posts,
    AllDms,
    Threads(Vec<String>),
    ThreadObjects(Vec<crate::model::DirectThread>),
}

#[derive(Debug, Clone)]
pub struct CleanerConfig {
    pub targets: Vec<TargetResolution>,
    pub content: ContentFlags,
    pub before: Option<String>,
    pub after: Option<String>,
    pub limit: usize,
    pub dry_run: bool,
    pub throttle_ms: u64,
    pub scan_throttle_ms: u64,
    pub concurrency: u32,
    pub hide_threads: bool,
    pub protect_users: Vec<String>,
    pub dm_kind: DmKind,
    pub dm_media_only: bool,
    /// False when threads are only in scope to have reactions removed.
    pub unsend_dms: bool,
    pub history_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ItemKind {
    #[default]
    Media,
    Dm,
    Like,
    Saved,
    Comment,
    Archived,
    Repost,
    DmReaction,
    Unfollow,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Media => "media",
            Self::Dm => "dm",
            Self::Like => "like",
            Self::Saved => "saved",
            Self::Comment => "comment",
            Self::Archived => "archived",
            Self::Repost => "repost",
            Self::DmReaction => "dm-reaction",
            Self::Unfollow => "unfollow",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PlannedDelete {
    pub target_id: String,
    pub target_name: String,
    pub item_id: String,
    pub kind: ItemKind,
    pub preview: String,
    pub author_id: String,
    pub author_name: String,
    pub content: String,
    pub timestamp: String,
    pub media_type: Option<String>,
    pub original_message_client_context: Option<String>,
    pub attachments: Vec<crate::history::AttachmentMeta>,
}

/// Identifies one write: a message and your reaction on it are separate items.
pub(crate) type ItemKey = (String, String, ItemKind);

impl PlannedDelete {
    pub(crate) fn key(&self) -> ItemKey {
        (self.target_id.clone(), self.item_id.clone(), self.kind)
    }

    pub fn to_record(
        &self,
        outcome: crate::history::DeleteOutcome,
    ) -> crate::history::DeletedRecord {
        crate::history::DeletedRecord {
            deleted_at: chrono::Utc::now(),
            outcome,
            target_id: self.target_id.clone(),
            target_name: self.target_name.clone(),
            item_id: self.item_id.clone(),
            kind: self.kind.as_str().to_string(),
            author_id: self.author_id.clone(),
            author_name: self.author_name.clone(),
            content: self.content.clone(),
            timestamp: self.timestamp.clone(),
            media_type: self.media_type.clone(),
            attachments: self.attachments.clone(),
            preview: self.preview.clone(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Plan {
    pub entries: Vec<PlannedDelete>,
    pub channels_processed: usize,
    pub messages_scanned: u64,
    /// Pages or checks the scan could not complete, so it may have missed items.
    pub scan_failures: u64,
    /// DM threads the scan could not read to the end.
    pub failed_threads: HashSet<String>,
}

/// A filter that failed to parse must stop the run: read as "no filter", it would
/// delete everything outside the dates the user asked for.
fn parse_bound(
    raw: Option<&str>,
    bound: dates::DateBound,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(text) => dates::parse_to_datetime(text, bound)
            .map(Some)
            .ok_or_else(|| format!("invalid date {text:?}")),
    }
}

pub async fn run(
    client: InstagramClient,
    config: CleanerConfig,
    rate_limiter: Arc<RateLimiter>,
    tx: mpsc::Sender<CleanerEvent>,
    cancel: Arc<AtomicBool>,
) -> RunStats {
    rate_limiter.set_user_throttle(std::time::Duration::from_millis(config.scan_throttle_ms));
    rate_limiter.set_parallelism(config.concurrency);

    let bounds = parse_bound(config.before.as_deref(), dates::DateBound::Before).and_then(|b| {
        parse_bound(config.after.as_deref(), dates::DateBound::After).map(|a| (b, a))
    });
    let (before, after) = match bounds {
        Ok(bounds) => bounds,
        Err(message) => {
            let _ = tx.send(CleanerEvent::Error { message }).await;
            return RunStats::default();
        }
    };

    let history = HistoryStore::with_root(&config.history_dir);
    let notes = scan::RunNotes::new(history.clone(), config.dry_run);
    let scope = match resolve_scope(&client, &config, &tx).await {
        Some(s) => s,
        None => return RunStats::default(),
    };

    let target_count = resolve::target_count(&scope);
    let _ = tx
        .send(CleanerEvent::Started {
            channel_count: target_count,
        })
        .await;
    emit_dry_run_banner(config.dry_run, &tx).await;
    let _ = tx
        .send(CleanerEvent::Log {
            level: LogLevel::Info,
            message: format!(
                "settings: scan every {}ms ×{} · delete every {}ms · also {}",
                config.scan_throttle_ms,
                config.concurrency,
                config.throttle_ms,
                config.content.label()
            ),
        })
        .await;

    let scan_ctx = scan::ScanCtx {
        client: &client,
        dm_unsend: scope.unsend_dms,
        dm_media_only: config.dm_media_only,
        dm_reactions: config.content.dm_reactions,
        protect_users: &config.protect_users,
        notes: &notes,
        before,
        after,
        limits: scan::ScanLimits::first_pass(config.concurrency),
        cancel: &cancel,
        tx: &tx,
    };
    let mut plan = scan::build_plan(&scan_ctx, &scope, config.limit).await;
    finalize_plan(&mut plan);

    rate_limiter.set_user_throttle(std::time::Duration::from_millis(config.throttle_ms));

    rate_limiter.set_parallelism(1);

    let mut settled = Settled {
        unconfirmed: plan.failed_threads.clone(),
        ..Settled::default()
    };

    let mut stats = if plan.entries.is_empty() {
        let was_cancelled = cancel.load(Ordering::Relaxed);
        if was_cancelled {
            let _ = tx
                .send(CleanerEvent::Log {
                    level: LogLevel::Info,
                    message: "scan cancelled, nothing to delete".into(),
                })
                .await;
            let _ = tx.send(CleanerEvent::Cancelled).await;
        }
        RunStats {
            channel_count: target_count,
            channels_processed: plan.channels_processed,
            messages_scanned: plan.messages_scanned,
            messages_deleted: 0,
            dm_messages_deleted: 0,
            messages_skipped: 0,
            messages_failed: 0,
            rate_limit_hits: 0,
            plan_total: 0,
            was_cancelled,
            unfollow_blocked: false,
        }
    } else {
        let _ = tx
            .send(CleanerEvent::PlanReady {
                total: plan.entries.len(),
            })
            .await;
        let (s, items) =
            execute::execute_plan(&client, plan, config.dry_run, &history, &tx, &cancel).await;
        settled.absorb(items);
        s
    };

    if cancel.load(Ordering::Relaxed) && !stats.was_cancelled {
        stats.was_cancelled = true;
        let _ = tx.send(CleanerEvent::Cancelled).await;
    }

    if should_verify_leftovers(&config, &stats) {
        let verify_ctx = scan::ScanCtx {
            client: &client,
            dm_unsend: scope.unsend_dms,
            dm_media_only: config.dm_media_only,
            dm_reactions: config.content.dm_reactions,
            protect_users: &config.protect_users,
            notes: &notes,
            before,
            after,
            limits: scan::ScanLimits::verify_pass(config.concurrency),
            cancel: &cancel,
            tx: &tx,
        };
        let rescan = settled.rescan_scope(&scope);
        stats = verify_leftovers(
            &verify_ctx,
            &rescan,
            &config,
            rate_limiter.clone(),
            stats,
            &mut settled,
        )
        .await;
    } else if stats.plan_total == 0 && !stats.was_cancelled {
        let _ = tx
            .send(CleanerEvent::Log {
                level: LogLevel::Info,
                message: "plan empty, nothing to delete".into(),
            })
            .await;
    }

    hide_threads_if_requested(&client, &config, &stats, &scope.threads, &settled, &tx).await;

    stats.rate_limit_hits = rate_limiter.hits();
    stats
}

async fn resolve_scope(
    client: &InstagramClient,
    config: &CleanerConfig,
    tx: &mpsc::Sender<CleanerEvent>,
) -> Option<resolve::ResolvedTargets> {
    match resolve::resolve_targets(
        client,
        &config.targets,
        config.content,
        &config.protect_users,
        config.dm_kind,
        config.unsend_dms,
        tx,
    )
    .await
    {
        Ok(s) => Some(s),
        Err(e) => {
            let _ = tx
                .send(CleanerEvent::Error {
                    message: format!("could not load targets: {e}"),
                })
                .await;
            None
        }
    }
}

fn should_verify_leftovers(config: &CleanerConfig, stats: &RunStats) -> bool {
    !config.dry_run
        && config.limit == 0
        && !stats.was_cancelled
        && !stats.unfollow_blocked
        && stats.plan_total > 0
}

/// What the run knows about items it has already attempted, across verify passes.
#[derive(Default)]
struct Settled {
    seen: HashSet<ItemKey>,
    changed_items: HashSet<ItemKey>,
    failed: HashSet<ItemKey>,
    skipped: HashSet<ItemKey>,
    /// Threads the latest read could not show to be free of your messages.
    unconfirmed: HashSet<String>,
}

impl Settled {
    fn absorb(&mut self, items: execute::ExecutedItems) {
        self.changed_items.extend(items.succeeded.iter().cloned());
        self.seen.extend(items.succeeded);
        self.failed.extend(items.failed);
        self.skipped.extend(items.skipped);
    }

    /// Forgets the outcome of every planned item that is being attempted again and
    /// returns how many of each target had failed or been skipped.
    fn take_retries(&mut self, plan: &Plan) -> Vec<RetryCount> {
        let mut by_target: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for entry in &plan.entries {
            let key = entry.key();
            if self.failed.remove(&key) {
                by_target.entry(&entry.target_id).or_default().0 += 1;
            } else if self.skipped.remove(&key) {
                by_target.entry(&entry.target_id).or_default().1 += 1;
            }
        }
        by_target
            .into_iter()
            .map(|(target_id, (failed, skipped))| RetryCount {
                target_id: target_id.to_string(),
                failed,
                skipped,
            })
            .collect()
    }

    /// Planned items no earlier pass has attempted.
    fn count_new(&self, plan: &Plan) -> usize {
        plan.entries
            .iter()
            .filter(|entry| {
                let key = entry.key();
                !self.failed.contains(&key) && !self.skipped.contains(&key)
            })
            .count()
    }

    /// A DM of yours may still be in this thread, so it must not be hidden.
    fn has_unresolved_dm(&self, thread_id: &str) -> bool {
        self.failed
            .iter()
            .chain(&self.skipped)
            .any(|(target_id, _, kind)| *kind == ItemKind::Dm && target_id == thread_id)
    }

    /// Everything but the threads this run never had anything to do in, which can
    /// only cost requests: nothing of yours was found there the first time.
    fn rescan_scope(&self, scope: &resolve::ResolvedTargets) -> resolve::ResolvedTargets {
        let touched: HashSet<&str> = self
            .seen
            .iter()
            .chain(&self.failed)
            .chain(&self.skipped)
            .map(|(target_id, _, _)| target_id.as_str())
            .chain(self.unconfirmed.iter().map(String::as_str))
            .collect();
        resolve::ResolvedTargets {
            posts: scope.posts,
            threads: scope
                .threads
                .iter()
                .filter(|thread| touched.contains(thread.id().as_str()))
                .cloned()
                .collect(),
            content: scope.content,
            unsend_dms: scope.unsend_dms,
        }
    }

    /// Takes in what a re-scan saw, including messages already reported as unsent.
    fn note_rescan(&mut self, plan: &Plan) {
        self.unconfirmed = plan.failed_threads.clone();
        self.unconfirmed.extend(
            plan.entries
                .iter()
                .filter(|entry| entry.kind == ItemKind::Dm)
                .map(|entry| entry.target_id.clone()),
        );
    }

    /// Threads where a message was unsent, split into the ones known to hold no more
    /// of yours and how many others have to stay. Removing a reaction leaves the
    /// thread's messages, so it never justifies hiding it.
    fn threads_to_hide(
        &self,
        threads: &[crate::model::DirectThread],
    ) -> (Vec<crate::model::DirectThread>, usize) {
        let changed: HashSet<&str> = self
            .changed_items
            .iter()
            .filter(|(_, _, kind)| *kind == ItemKind::Dm)
            .map(|(target_id, _, _)| target_id.as_str())
            .collect();
        let (hide, keep): (Vec<_>, Vec<_>) = threads
            .iter()
            .filter(|thread| changed.contains(thread.id().as_str()))
            .cloned()
            .partition(|thread| {
                let id = thread.id();
                !self.has_unresolved_dm(&id) && !self.unconfirmed.contains(&id)
            });
        (hide, keep.len())
    }
}

async fn emit_cancelled_once(stats: &mut RunStats, tx: &mpsc::Sender<CleanerEvent>) {
    if stats.was_cancelled {
        return;
    }
    stats.was_cancelled = true;
    let _ = tx.send(CleanerEvent::Cancelled).await;
}

async fn verify_leftovers(
    ctx: &scan::ScanCtx<'_>,
    scope: &resolve::ResolvedTargets,
    config: &CleanerConfig,
    rate_limiter: Arc<RateLimiter>,
    mut stats: RunStats,
    settled: &mut Settled,
) -> RunStats {
    let mut verify_executes = 0u8;
    loop {
        if ctx.cancel.load(Ordering::Relaxed) {
            emit_cancelled_once(&mut stats, ctx.tx).await;
            return stats;
        }

        let _ = ctx.tx.send(CleanerEvent::Verifying).await;
        let _ = ctx
            .tx
            .send(CleanerEvent::Log {
                level: LogLevel::Info,
                message: format!(
                    "verify: re-scanning for leftovers (scan {})",
                    verify_executes + 1
                ),
            })
            .await;

        rate_limiter.set_user_throttle(std::time::Duration::from_millis(config.scan_throttle_ms));
        rate_limiter.set_parallelism(config.concurrency);
        let mut leftover = scan::build_plan(ctx, scope, config.limit).await;
        finalize_plan(&mut leftover);
        settled.note_rescan(&leftover);
        drop_already_seen(&mut leftover, &settled.seen);
        rate_limiter.set_user_throttle(std::time::Duration::from_millis(config.throttle_ms));
        rate_limiter.set_parallelism(1);

        if ctx.cancel.load(Ordering::Relaxed) {
            emit_cancelled_once(&mut stats, ctx.tx).await;
            return stats;
        }

        if leftover.entries.is_empty() && leftover.scan_failures > 0 {
            let _ = ctx
                .tx
                .send(CleanerEvent::Warning {
                    message:
                        "verify: part of the re-scan failed, so the deletions are not confirmed"
                            .into(),
                })
                .await;
            return stats;
        }

        if leftover.entries.is_empty() {
            let _ = ctx
                .tx
                .send(CleanerEvent::Log {
                    level: LogLevel::Success,
                    message: if verify_executes == 0 && stats.plan_total == 0 {
                        "plan empty, nothing to delete".into()
                    } else {
                        "verify: no leftovers, deletions confirmed".into()
                    },
                })
                .await;
            return stats;
        }

        if verify_executes >= MAX_VERIFY_PASSES {
            stats.plan_total += settled.count_new(&leftover);
            let _ = ctx
                .tx
                .send(CleanerEvent::Warning {
                    message: format!(
                        "verify: still found {} leftover(s) after {MAX_VERIFY_PASSES} passes, stopping",
                        leftover.entries.len()
                    ),
                })
                .await;
            return stats;
        }

        verify_executes += 1;
        // a retried item is still the one item it was: it must not count as found twice
        stats.plan_total += settled.count_new(&leftover);
        let retries = settled.take_retries(&leftover);
        for retry in &retries {
            stats.messages_failed = stats.messages_failed.saturating_sub(retry.failed);
            stats.messages_skipped = stats.messages_skipped.saturating_sub(retry.skipped);
        }
        let _ = ctx
            .tx
            .send(CleanerEvent::Log {
                level: LogLevel::Warning,
                message: format!(
                    "verify pass {verify_executes}/{MAX_VERIFY_PASSES}: {} leftover item(s) still present, deleting",
                    leftover.entries.len()
                ),
            })
            .await;
        if !retries.is_empty() {
            let _ = ctx
                .tx
                .send(CleanerEvent::Retrying { counts: retries })
                .await;
        }
        let _ = ctx
            .tx
            .send(CleanerEvent::PlanReady {
                total: stats.plan_total,
            })
            .await;

        let history = HistoryStore::with_root(&config.history_dir);
        let (extra, items) = execute::execute_plan(
            ctx.client,
            leftover,
            config.dry_run,
            &history,
            ctx.tx,
            ctx.cancel,
        )
        .await;
        settled.absorb(items);
        merge_execute_stats(&mut stats, extra);
        if stats.was_cancelled || stats.unfollow_blocked {
            return stats;
        }
    }
}

fn finalize_plan(plan: &mut Plan) {
    plan.entries.sort_by(|a, b| {
        a.target_id
            .cmp(&b.target_id)
            .then(a.item_id.cmp(&b.item_id))
            .then(a.kind.as_str().cmp(b.kind.as_str()))
    });
    plan.entries
        .dedup_by(|a, b| a.target_id == b.target_id && a.item_id == b.item_id && a.kind == b.kind);
}

pub(crate) fn drop_already_seen(plan: &mut Plan, seen: &HashSet<ItemKey>) {
    plan.entries.retain(|e| !seen.contains(&e.key()));
}

fn merge_execute_stats(into: &mut RunStats, extra: RunStats) {
    into.messages_deleted += extra.messages_deleted;
    into.dm_messages_deleted += extra.dm_messages_deleted;
    into.messages_failed += extra.messages_failed;
    into.messages_skipped += extra.messages_skipped;
    into.channels_processed = into
        .channels_processed
        .saturating_add(extra.channels_processed);
    if extra.was_cancelled {
        into.was_cancelled = true;
    }
    if extra.unfollow_blocked {
        into.unfollow_blocked = true;
    }
}

async fn emit_dry_run_banner(dry_run: bool, tx: &mpsc::Sender<CleanerEvent>) {
    if dry_run {
        let _ = tx
            .send(CleanerEvent::Log {
                level: LogLevel::Warning,
                message: "DRY RUN: no content will be deleted".into(),
            })
            .await;
    }
}

/// Why this run must not hide any thread, if it has a reason to say so.
fn hide_blocker(config: &CleanerConfig, stats: &RunStats) -> Option<&'static str> {
    let has_dates = config.before.is_some() || config.after.is_some();
    if config.dm_media_only {
        Some("Media only keeps the rest of each thread")
    } else if has_dates {
        Some("date filters keep the rest of each thread")
    } else if config.limit > 0 {
        Some("an item limit can stop partway through a thread")
    } else if stats.unfollow_blocked {
        Some("Instagram is refusing actions, so no more were sent")
    } else if stats.dm_messages_deleted == 0 {
        Some("no DM messages were deleted")
    } else {
        None
    }
}

async fn hide_threads_if_requested(
    client: &InstagramClient,
    config: &CleanerConfig,
    stats: &RunStats,
    threads: &[crate::model::DirectThread],
    settled: &Settled,
    tx: &mpsc::Sender<CleanerEvent>,
) {
    if !should_hide_threads(config) || stats.was_cancelled {
        return;
    }
    if let Some(reason) = hide_blocker(config, stats) {
        let _ = tx
            .send(CleanerEvent::Log {
                level: LogLevel::Info,
                message: format!("hide threads: skipped, {reason}"),
            })
            .await;
        return;
    }
    let (hide, kept) = settled.threads_to_hide(threads);
    if kept > 0 {
        let _ = tx
            .send(CleanerEvent::Log {
                level: LogLevel::Warning,
                message: format!(
                    "hide threads: keeping {kept} thread(s) that may still hold a message of yours"
                ),
            })
            .await;
    }
    if !hide.is_empty() {
        execute::hide_resolved_threads(client, &hide, config.dry_run, tx).await;
    } else if kept == 0 {
        let _ = tx
            .send(CleanerEvent::Log {
                level: LogLevel::Info,
                message: "hide threads: no threads changed".into(),
            })
            .await;
    }
}

fn should_hide_threads(config: &CleanerConfig) -> bool {
    config.hide_threads && resolve::has_dm_scope(&config.targets)
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    #[test]
    fn only_dm_scopes_can_hide_threads() {
        let mut config = CleanerConfig {
            targets: vec![TargetResolution::Posts],
            content: ContentFlags::default(),
            before: None,
            after: None,
            limit: 0,
            dry_run: false,
            throttle_ms: 2_000,
            scan_throttle_ms: 350,
            concurrency: 4,
            hide_threads: true,
            protect_users: Vec::new(),
            dm_kind: DmKind::All,
            dm_media_only: false,
            unsend_dms: true,
            history_dir: String::new(),
        };
        assert!(!should_hide_threads(&config));

        config.targets = vec![TargetResolution::AllDms];
        assert!(should_hide_threads(&config));

        config.hide_threads = false;
        assert!(!should_hide_threads(&config));

        let mut stats = RunStats {
            plan_total: 1,
            ..RunStats::default()
        };
        assert!(should_verify_leftovers(&config, &stats));
        stats.unfollow_blocked = true;
        assert!(!should_verify_leftovers(&config, &stats));
    }
}

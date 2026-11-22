use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::client::DiscordClient;
use crate::config::{ContentFilter, DmKind};
use crate::events::{LogLevel, PrunerEvent, RunStats};
use crate::ratelimit::RateLimiter;

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
    AllDms,
    AllGuilds,
    Channels(Vec<String>),
    ChannelObjects(Vec<crate::model::Channel>),
}

#[derive(Debug, Clone)]
pub struct PrunerConfig {
    pub targets: Vec<TargetResolution>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub limit: usize,
    pub dry_run: bool,
    pub throttle_ms: u64,
    pub scan_throttle_ms: u64,
    pub concurrency: u32,
    pub close_dms: bool,
    pub protect_ids: Vec<String>,
    pub dm_kind: DmKind,
    pub content_filter: ContentFilter,
    pub keep_pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemKind {
    #[default]
    Message,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
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
    pub attachments: Vec<crate::history::AttachmentMeta>,
}

impl PlannedDelete {
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
}

pub async fn run(
    client: DiscordClient,
    config: PrunerConfig,
    rate_limiter: Arc<RateLimiter>,
    tx: mpsc::Sender<PrunerEvent>,
    cancel: Arc<AtomicBool>,
) -> RunStats {
    rate_limiter.set_user_throttle(std::time::Duration::from_millis(config.scan_throttle_ms));
    rate_limiter.set_parallelism(config.concurrency);

    let before = config
        .before
        .as_deref()
        .and_then(|s| dates::parse_to_datetime(s, dates::DateBound::Before));
    let after = config
        .after
        .as_deref()
        .and_then(|s| dates::parse_to_datetime(s, dates::DateBound::After));

    let scope = match resolve_scope(&client, &config, &tx).await {
        Some(s) => s,
        None => return RunStats::default(),
    };

    let target_count = resolve::target_count(&scope);
    let _ = tx
        .send(PrunerEvent::Started {
            channel_count: target_count,
        })
        .await;
    emit_dry_run_banner(config.dry_run, &tx).await;
    let _ = tx
        .send(PrunerEvent::Log {
            level: LogLevel::Info,
            message: format!(
                "CFG   scan {}ms ×{} parallel · delete {}ms serial · own-only full history",
                config.scan_throttle_ms, config.concurrency, config.throttle_ms,
            ),
        })
        .await;

    let scan_ctx = scan::ScanCtx {
        client: &client,
        before,
        after,
        content_filter: config.content_filter,
        keep_pinned: config.keep_pinned,
        limits: scan::ScanLimits::first_pass(config.concurrency),
        cancel: &cancel,
        tx: &tx,
    };
    let mut plan = scan::build_plan(&scan_ctx, &scope, config.limit).await;
    finalize_plan(&mut plan);

    rate_limiter.set_user_throttle(std::time::Duration::from_millis(config.throttle_ms));

    rate_limiter.set_parallelism(1);

    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut changed_items: HashSet<(String, String)> = HashSet::new();

    let mut stats = if plan.entries.is_empty() {
        let was_cancelled = cancel.load(Ordering::Relaxed);
        if was_cancelled {
            let _ = tx
                .send(PrunerEvent::Log {
                    level: LogLevel::Info,
                    message: "scan cancelled, nothing to delete".into(),
                })
                .await;
            let _ = tx.send(PrunerEvent::Cancelled).await;
        }
        RunStats {
            channel_count: target_count,
            channels_processed: plan.channels_processed,
            messages_scanned: plan.messages_scanned,
            messages_planned: 0,
            messages_deleted: 0,
            dm_messages_deleted: 0,
            messages_skipped: 0,
            messages_failed: 0,
            rate_limit_hits: 0,
            plan_total: 0,
            was_cancelled,
        }
    } else {
        let _ = tx
            .send(PrunerEvent::PlanReady {
                total: plan.entries.len(),
            })
            .await;
        let (s, succeeded) =
            execute::execute_plan(&client, plan, config.dry_run, &tx, &cancel).await;
        changed_items.extend(succeeded.iter().cloned());
        seen.extend(succeeded);
        s
    };

    if cancel.load(Ordering::Relaxed) && !stats.was_cancelled {
        stats.was_cancelled = true;
        let _ = tx.send(PrunerEvent::Cancelled).await;
    }

    if should_verify_leftovers(&config, &stats) {
        let verify_ctx = scan::ScanCtx {
            client: &client,
            before,
            after,
            content_filter: config.content_filter,
            keep_pinned: config.keep_pinned,
            limits: scan::ScanLimits::verify_pass(config.concurrency),
            cancel: &cancel,
            tx: &tx,
        };
        stats = verify_leftovers(
            &verify_ctx,
            &scope,
            &config,
            rate_limiter,
            stats,
            &mut seen,
            &mut changed_items,
        )
        .await;
    } else if stats.plan_total == 0 && !stats.was_cancelled {
        let _ = tx
            .send(PrunerEvent::Log {
                level: LogLevel::Info,
                message: "plan empty, nothing to delete".into(),
            })
            .await;
    }

    // Close all targeted 1:1 DMs (type 1) in scope.
    // Never close group DMs (type 3) — that leaves the group.
    // Never close guild channels.
    close_dms_if_requested(&client, &config, &scope.channels, stats.was_cancelled, &tx).await;

    stats
}

async fn resolve_scope(
    client: &DiscordClient,
    config: &PrunerConfig,
    tx: &mpsc::Sender<PrunerEvent>,
) -> Option<resolve::ResolvedTargets> {
    match resolve::resolve_targets(
        client,
        &config.targets,
        &config.protect_ids,
        config.dm_kind,
        tx,
    )
    .await
    {
        Ok(s) => Some(s),
        Err(e) => {
            let _ = tx
                .send(PrunerEvent::Error {
                    message: format!("target resolution failed: {e}"),
                })
                .await;
            None
        }
    }
}

fn should_verify_leftovers(config: &PrunerConfig, stats: &RunStats) -> bool {
    !config.dry_run && config.limit == 0 && !stats.was_cancelled && stats.plan_total > 0
}

async fn emit_cancelled_once(stats: &mut RunStats, tx: &mpsc::Sender<PrunerEvent>) {
    if stats.was_cancelled {
        return;
    }
    stats.was_cancelled = true;
    let _ = tx.send(PrunerEvent::Cancelled).await;
}

async fn verify_leftovers(
    ctx: &scan::ScanCtx<'_>,
    scope: &resolve::ResolvedTargets,
    config: &PrunerConfig,
    rate_limiter: Arc<RateLimiter>,
    mut stats: RunStats,
    seen: &mut HashSet<(String, String)>,
    changed_items: &mut HashSet<(String, String)>,
) -> RunStats {
    let mut verify_executes = 0u8;
    loop {
        if ctx.cancel.load(Ordering::Relaxed) {
            emit_cancelled_once(&mut stats, ctx.tx).await;
            return stats;
        }

        let _ = ctx
            .tx
            .send(PrunerEvent::Log {
                level: LogLevel::Info,
                message: format!(
                    "verify: re-scanning for leftovers (pass {}/{MAX_VERIFY_PASSES})",
                    verify_executes + 1
                ),
            })
            .await;

        rate_limiter.set_user_throttle(std::time::Duration::from_millis(config.scan_throttle_ms));
        rate_limiter.set_parallelism(config.concurrency);
        let mut leftover = scan::build_plan(ctx, scope, config.limit).await;
        finalize_plan(&mut leftover);
        drop_already_seen(&mut leftover, seen);
        stats.messages_scanned += leftover.messages_scanned;
        rate_limiter.set_user_throttle(std::time::Duration::from_millis(config.throttle_ms));
        rate_limiter.set_parallelism(1);

        if ctx.cancel.load(Ordering::Relaxed) {
            emit_cancelled_once(&mut stats, ctx.tx).await;
            return stats;
        }

        if leftover.entries.is_empty() {
            let _ = ctx
                .tx
                .send(PrunerEvent::Log {
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
            let _ = ctx
                .tx
                .send(PrunerEvent::Log {
                    level: LogLevel::Warning,
                    message: format!(
                        "verify: still found {} leftover(s) after {MAX_VERIFY_PASSES} passes, stopping",
                        leftover.entries.len()
                    ),
                })
                .await;
            return stats;
        }

        verify_executes += 1;
        stats.plan_total += leftover.entries.len();
        let _ = ctx
            .tx
            .send(PrunerEvent::Log {
                level: LogLevel::Warning,
                message: format!(
                    "verify pass {verify_executes}: {} leftover item(s) still present, deleting",
                    leftover.entries.len()
                ),
            })
            .await;
        let _ = ctx
            .tx
            .send(PrunerEvent::PlanReady {
                total: stats.plan_total,
            })
            .await;

        let (extra, succeeded) =
            execute::execute_plan(ctx.client, leftover, config.dry_run, ctx.tx, ctx.cancel).await;
        changed_items.extend(succeeded.iter().cloned());
        seen.extend(succeeded);
        merge_execute_stats(&mut stats, extra);
        if stats.was_cancelled {
            return stats;
        }
    }
}

fn finalize_plan(plan: &mut Plan) {
    plan.entries.sort_by(|a, b| {
        a.target_id
            .cmp(&b.target_id)
            .then(a.item_id.cmp(&b.item_id))
    });
    plan.entries
        .dedup_by(|a, b| a.target_id == b.target_id && a.item_id == b.item_id);
}

pub(crate) fn drop_already_seen(plan: &mut Plan, seen: &HashSet<(String, String)>) {
    plan.entries
        .retain(|e| !seen.contains(&(e.target_id.clone(), e.item_id.clone())));
}

fn merge_execute_stats(into: &mut RunStats, extra: RunStats) {
    into.messages_deleted += extra.messages_deleted;
    into.dm_messages_deleted += extra.dm_messages_deleted;
    into.messages_failed += extra.messages_failed;
    into.messages_skipped += extra.messages_skipped;
    into.rate_limit_hits += extra.rate_limit_hits;
    into.channels_processed = into
        .channels_processed
        .saturating_add(extra.channels_processed);
    if extra.was_cancelled {
        into.was_cancelled = true;
    }
}

async fn emit_dry_run_banner(dry_run: bool, tx: &mpsc::Sender<PrunerEvent>) {
    if dry_run {
        let _ = tx
            .send(PrunerEvent::Log {
                level: LogLevel::Warning,
                message: "DRY RUN: no content will be deleted".into(),
            })
            .await;
    }
}

async fn close_dms_if_requested(
    client: &DiscordClient,
    config: &PrunerConfig,
    channels: &[crate::model::Channel],
    was_cancelled: bool,
    tx: &mpsc::Sender<PrunerEvent>,
) {
    if !should_close_dms(config) || was_cancelled {
        return;
    }
    // Only close 1:1 DMs (type 1). Never close group DMs (type 3) or guild channels.
    let target_dms: Vec<_> = channels
        .iter()
        .filter(|channel| channel.is_dm())
        .cloned()
        .collect();
    if target_dms.is_empty() {
        return;
    }
    execute::close_resolved_dms(client, &target_dms, config.dry_run, tx).await;
}

fn should_close_dms(config: &PrunerConfig) -> bool {
    config.close_dms
        && config.targets.iter().any(|target| {
            matches!(
                target,
                TargetResolution::AllDms
                    | TargetResolution::Channels(_)
                    | TargetResolution::ChannelObjects(_)
            )
        })
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    #[test]
    fn only_dm_scopes_can_close_dms() {
        let mut config = PrunerConfig {
            targets: vec![TargetResolution::AllGuilds],
            before: None,
            after: None,
            limit: 0,
            dry_run: false,
            throttle_ms: 2_000,
            scan_throttle_ms: 350,
            concurrency: 4,
            close_dms: true,
            protect_ids: Vec::new(),
            dm_kind: DmKind::All,
            content_filter: ContentFilter::All,
            keep_pinned: false,
        };
        assert!(!should_close_dms(&config));

        config.targets = vec![TargetResolution::AllDms];
        assert!(should_close_dms(&config));

        config.close_dms = false;
        assert!(!should_close_dms(&config));
    }
}

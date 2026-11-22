use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::client::DiscordClient;
use crate::error::{DiscordError, DiscordResult};
use crate::events::{ChannelStats, LogLevel, Phase, PrunerEvent, RunStats};
use crate::history::{DeleteOutcome, HistoryStore};
use crate::model::Channel;
use crate::util::truncate;

use super::{ItemKind, PlannedDelete};

pub(crate) fn map_delete_bool(ok: bool, what: &str) -> DiscordResult<()> {
    if ok {
        Ok(())
    } else {
        Err(DiscordError::Other(format!("{what} rejected (false)")))
    }
}

pub(crate) fn is_retryable_delete_error(err: &DiscordError) -> bool {
    match err {
        DiscordError::Http(_) => true,
        DiscordError::Api { status, .. } if (500..600).contains(status) => true,
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteStep {
    Deleted,
    AlreadyGone,
    Forbidden,
    Failed,
    DryRun,
}

impl DeleteStep {
    fn outcome(self) -> DeleteOutcome {
        match self {
            Self::AlreadyGone => DeleteOutcome::AlreadyGone,
            Self::Forbidden => DeleteOutcome::Forbidden,
            Self::DryRun => DeleteOutcome::DryRun,
            Self::Deleted | Self::Failed => DeleteOutcome::Deleted,
        }
    }
}

pub(crate) fn classify_delete_error(err: &DiscordError) -> DeleteStep {
    match err {
        DiscordError::NotFound { .. } => DeleteStep::AlreadyGone,
        DiscordError::Forbidden { .. } => DeleteStep::Forbidden,
        _ => DeleteStep::Failed,
    }
}

pub(crate) fn record_outcome(
    entry: &PlannedDelete,
    step: DeleteStep,
    channel_stats: &mut ChannelStats,
    stats: &mut RunStats,
    succeeded: &mut HashSet<(String, String)>,
) {
    match step {
        DeleteStep::Deleted | DeleteStep::AlreadyGone => {
            channel_stats.deleted += 1;
            if step == DeleteStep::Deleted && entry.kind == ItemKind::Message {
                // `dm_messages_deleted` counts DM-channel deletions only; the
                // caller distinguishes by target kind via channel metadata.
                // We conservatively count every message here and let the
                // close_dm gate use the DM subset (see execute_plan caller).
                stats.dm_messages_deleted += 1;
            }
            succeeded.insert((entry.target_id.clone(), entry.item_id.clone()));
        }
        DeleteStep::Forbidden => channel_stats.skipped += 1,
        DeleteStep::Failed => channel_stats.failed += 1,
        DeleteStep::DryRun => channel_stats.planned += 1,
    }
}

/// Collapses the plan into one group per target, keeping the plan's order.
/// `finalize_plan` sorts by `target_id`, so equal targets are already adjacent.
pub(crate) fn group_by_target(
    entries: Vec<PlannedDelete>,
) -> Vec<(String, String, Vec<PlannedDelete>)> {
    let mut by_target: Vec<(String, String, Vec<PlannedDelete>)> = Vec::new();
    for entry in entries {
        if let Some(last) = by_target.last_mut() {
            if last.0 == entry.target_id {
                last.2.push(entry);
                continue;
            }
        }
        by_target.push((
            entry.target_id.clone(),
            entry.target_name.clone(),
            vec![entry],
        ));
    }
    by_target
}

pub(crate) async fn execute_plan(
    client: &DiscordClient,
    plan: super::Plan,
    dry_run: bool,
    tx: &mpsc::Sender<PrunerEvent>,
    cancel: &Arc<AtomicBool>,
) -> (RunStats, HashSet<(String, String)>) {
    let history = HistoryStore::from_env_with("PRUNER_HISTORY_DIR", "history");
    let mut succeeded: HashSet<(String, String)> = HashSet::new();

    let total_entries = plan.entries.len();
    let by_target = group_by_target(plan.entries);
    let by_target_len = by_target.len();
    let mut stats = RunStats {
        channel_count: by_target_len,
        plan_total: total_entries,
        messages_scanned: plan.messages_scanned,
        ..Default::default()
    };

    for (target_index, (target_id, target_name, entries)) in by_target.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(PrunerEvent::Cancelled).await;
            stats.was_cancelled = true;
            return (stats, succeeded);
        }

        let _ = tx
            .send(PrunerEvent::ChannelStarted {
                channel_id: target_id.clone(),
                channel_name: target_name.clone(),
                index: target_index,
                total: by_target_len,
                phase: Phase::Delete,
            })
            .await;

        let mut channel_stats = ChannelStats::default();
        let entries_len = entries.len() as u64;
        let mut target_rate_hits = 0u64;

        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                finish_cancelled(
                    tx,
                    &mut stats,
                    &mut channel_stats,
                    entries_len,
                    target_rate_hits,
                    &target_id,
                )
                .await;
                return (stats, succeeded);
            }

            if dry_run {
                record_outcome(
                    &entry,
                    DeleteStep::DryRun,
                    &mut channel_stats,
                    &mut stats,
                    &mut succeeded,
                );
                emit_deleted(tx, &history, &entry, DeleteOutcome::DryRun).await;
                continue;
            }

            match delete_with_retry(client, &entry, cancel, tx, &mut target_rate_hits).await {
                Ok(()) => {
                    record_outcome(
                        &entry,
                        DeleteStep::Deleted,
                        &mut channel_stats,
                        &mut stats,
                        &mut succeeded,
                    );
                    emit_deleted(tx, &history, &entry, DeleteOutcome::Deleted).await;
                }
                Err(DiscordError::Cancelled) => {
                    finish_cancelled(
                        tx,
                        &mut stats,
                        &mut channel_stats,
                        entries_len,
                        target_rate_hits,
                        &target_id,
                    )
                    .await;
                    return (stats, succeeded);
                }
                Err(e) => {
                    let step = classify_delete_error(&e);
                    record_outcome(&entry, step, &mut channel_stats, &mut stats, &mut succeeded);
                    if step == DeleteStep::Failed {
                        let _ = tx
                            .send(PrunerEvent::MessageFailed {
                                message_id: entry.item_id.clone(),
                                error: e.to_string(),
                            })
                            .await;
                    } else {
                        emit_deleted(tx, &history, &entry, step.outcome()).await;
                    }
                }
            }
        }

        let remaining = entries_len
            .saturating_sub(channel_stats.deleted)
            .saturating_sub(channel_stats.failed)
            .saturating_sub(channel_stats.skipped);
        if remaining > 0 {
            channel_stats.skipped += remaining;
        }
        stats.rate_limit_hits += target_rate_hits;
        stats.channels_processed += 1;
        stats.aggregate(channel_stats);
        let _ = tx
            .send(PrunerEvent::ChannelDone {
                channel_id: target_id.clone(),
                stats: channel_stats,
                phase: Phase::Delete,
            })
            .await;
    }

    if cancel.load(Ordering::Relaxed) {
        let _ = tx.send(PrunerEvent::Cancelled).await;
        stats.was_cancelled = true;
    }

    (stats, succeeded)
}

async fn finish_cancelled(
    tx: &mpsc::Sender<PrunerEvent>,
    stats: &mut RunStats,
    channel_stats: &mut ChannelStats,
    entries_len: u64,
    target_rate_hits: u64,
    target_id: &str,
) {
    let _ = tx.send(PrunerEvent::Cancelled).await;
    stats.was_cancelled = true;
    stats.rate_limit_hits += target_rate_hits;
    let remaining = entries_len
        .saturating_sub(channel_stats.deleted)
        .saturating_sub(channel_stats.failed)
        .saturating_sub(channel_stats.skipped);
    if remaining > 0 {
        channel_stats.skipped += remaining;
    }
    stats.channels_processed += 1;
    stats.aggregate(*channel_stats);
    let _ = tx
        .send(PrunerEvent::ChannelDone {
            channel_id: target_id.to_string(),
            stats: *channel_stats,
            phase: Phase::Delete,
        })
        .await;
}

async fn emit_deleted(
    tx: &mpsc::Sender<PrunerEvent>,
    history: &HistoryStore,
    entry: &PlannedDelete,
    outcome: DeleteOutcome,
) {
    let record = entry.to_record(outcome);
    if let Err(e) = history.append(&record) {
        let _ = tx
            .send(PrunerEvent::Log {
                level: LogLevel::Warning,
                message: format!(
                    "history write failed for {}/{}: {e}",
                    entry.target_id, entry.item_id
                ),
            })
            .await;
    }
    let _ = tx
        .send(PrunerEvent::MessageDeleted {
            preview: entry.preview.clone(),
            outcome,
        })
        .await;
}

async fn delete_with_retry(
    client: &DiscordClient,
    entry: &PlannedDelete,
    cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<PrunerEvent>,
    rate_hits: &mut u64,
) -> DiscordResult<()> {
    let mut attempt = 0u8;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DiscordError::Cancelled);
        }
        let result = match client
            .delete_message(&entry.target_id, &entry.item_id)
            .await
        {
            Ok(ok) => map_delete_bool(ok, "message delete"),
            Err(e) => Err(e),
        };
        match result {
            Ok(()) => return Ok(()),
            Err(DiscordError::RateLimited {
                retry_after_ms,
                global,
                ..
            }) => {
                *rate_hits = rate_hits.saturating_add(1);
                attempt += 1;
                if attempt > 2 {
                    return Err(DiscordError::RateLimited {
                        retry_after_ms,
                        global,
                        scope: "exhausted".into(),
                    });
                }
                let retry_after_ms = retry_after_ms.max(500);
                let _ = tx.send(PrunerEvent::RateLimited { retry_after_ms }).await;
                crate::ratelimit::poll_sleep_with_cancel(
                    std::time::Duration::from_millis(retry_after_ms),
                    cancel.clone(),
                )
                .await;
                if cancel.load(Ordering::Relaxed) {
                    return Err(DiscordError::Cancelled);
                }
            }
            Err(e) if is_retryable_delete_error(&e) => {
                attempt += 1;
                if attempt > 2 {
                    return Err(e);
                }
                let _ = tx
                    .send(PrunerEvent::Log {
                        level: LogLevel::Warning,
                        message: format!(
                            "delete {}/{} transient error, retrying ({attempt}/2): {e}",
                            entry.target_id, entry.item_id
                        ),
                    })
                    .await;
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                if cancel.load(Ordering::Relaxed) {
                    return Err(DiscordError::Cancelled);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

pub(crate) async fn close_resolved_dms(
    client: &DiscordClient,
    channels: &[Channel],
    dry_run: bool,
    tx: &mpsc::Sender<PrunerEvent>,
) {
    let mut hidden: u64 = 0;
    let mut failed: u64 = 0;
    let mut skipped_groups: u64 = 0;

    for channel in channels {
        // Never leave group DMs: DELETE /channels/{id} on a type-3 channel
        // removes the account from the group. Only 1:1 DMs (type 1) are
        // safe to close (hides the DM, leaves nothing). Guild channels
        // always 403 here by design.
        if channel.is_group_dm() {
            skipped_groups += 1;
            continue;
        }
        if !channel.is_dm() {
            continue;
        }
        let id = channel.id();
        let name = channel.display_name();
        if dry_run {
            hidden += 1;
            let _ = tx
                .send(PrunerEvent::Log {
                    level: LogLevel::Warning,
                    message: format!("WOULD-CLOSE {id}  {}", truncate(&name, 40)),
                })
                .await;
            continue;
        }
        match client.close_dm(&id).await {
            Ok(true) | Err(DiscordError::NotFound { .. }) => {
                hidden += 1;
                let _ = tx
                    .send(PrunerEvent::Log {
                        level: LogLevel::Success,
                        message: format!("CLOSE {id}  {}", truncate(&name, 40)),
                    })
                    .await;
            }
            Ok(false) => {
                failed += 1;
                let _ = tx
                    .send(PrunerEvent::Log {
                        level: LogLevel::Error,
                        message: format!("CLOSE {id} rejected: Discord returned status != ok"),
                    })
                    .await;
            }
            Err(e) => {
                failed += 1;
                let _ = tx
                    .send(PrunerEvent::Log {
                        level: LogLevel::Error,
                        message: format!("CLOSE {id} failed: {e}"),
                    })
                    .await;
            }
        }
    }

    let mut summary = if dry_run {
        format!("close_dms: would close {hidden} DM(s)")
    } else {
        format!("close_dms: closed {hidden} DM(s), {failed} failed")
    };
    if skipped_groups > 0 {
        summary.push_str(&format!(
            "; skipped {skipped_groups} group DM(s) (never leave groups)"
        ));
    }
    let level = if failed > 0 {
        LogLevel::Warning
    } else {
        LogLevel::Info
    };
    let _ = tx
        .send(PrunerEvent::Log {
            level,
            message: summary,
        })
        .await;
}

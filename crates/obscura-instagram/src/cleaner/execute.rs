use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::client::InstagramClient;
use crate::error::{IgError, IgResult};
use crate::events::{ChannelStats, CleanerEvent, LogLevel, Phase, RunStats};
use crate::history::{DeleteOutcome, HistoryStore};
use crate::model::DirectThread;
use crate::util::truncate;

use super::{ItemKind, PlannedDelete};

pub(crate) fn map_delete_bool(ok: bool, what: &str) -> IgResult<()> {
    if ok {
        Ok(())
    } else {
        Err(IgError::Other(format!("{what} rejected (false)")))
    }
}

pub(crate) fn is_retryable_delete_error(err: &IgError) -> bool {
    match err {
        IgError::Http(_) => true,
        IgError::Api { status, .. } if (500..600).contains(status) => true,
        IgError::FeedbackRequired { .. } => true,
        _ => false,
    }
}

/// Instagram is refusing follow actions: more attempts only risk a longer block.
pub(crate) fn blocks_further_unfollows(err: &IgError) -> bool {
    matches!(
        err,
        IgError::FeedbackRequired { .. }
            | IgError::RateLimited { .. }
            | IgError::ChallengeRequired { .. }
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteStep {
    Deleted,
    AlreadyGone,
    Forbidden,
    Failed,
}

impl DeleteStep {
    fn outcome(self) -> DeleteOutcome {
        match self {
            Self::AlreadyGone => DeleteOutcome::AlreadyGone,
            Self::Forbidden => DeleteOutcome::Forbidden,
            Self::Deleted | Self::Failed => DeleteOutcome::Deleted,
        }
    }
}

pub(crate) fn classify_delete_error(err: &IgError) -> DeleteStep {
    match err {
        IgError::NotFound { .. } => DeleteStep::AlreadyGone,
        IgError::Forbidden { .. } => DeleteStep::Forbidden,
        _ => DeleteStep::Failed,
    }
}

pub(crate) fn record_outcome(
    entry: &PlannedDelete,
    step: DeleteStep,
    channel_stats: &mut ChannelStats,
    stats: &mut RunStats,
    succeeded: &mut HashSet<super::ItemKey>,
) {
    match step {
        DeleteStep::Deleted | DeleteStep::AlreadyGone => {
            channel_stats.deleted += 1;
            if step == DeleteStep::Deleted && entry.kind == ItemKind::Dm {
                stats.dm_messages_deleted += 1;
            }
            succeeded.insert(entry.key());
        }
        DeleteStep::Forbidden => channel_stats.skipped += 1,
        DeleteStep::Failed => channel_stats.failed += 1,
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

/// Where each attempted item ended up. Items a cancel or block never reached are in none.
#[derive(Debug, Default)]
pub(crate) struct ExecutedItems {
    pub succeeded: HashSet<super::ItemKey>,
    pub failed: HashSet<super::ItemKey>,
    pub skipped: HashSet<super::ItemKey>,
}

pub(crate) async fn execute_plan(
    client: &InstagramClient,
    plan: super::Plan,
    dry_run: bool,
    history: &HistoryStore,
    tx: &mpsc::Sender<CleanerEvent>,
    cancel: &Arc<AtomicBool>,
) -> (RunStats, ExecutedItems) {
    let mut items = ExecutedItems::default();

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
            let _ = tx.send(CleanerEvent::Cancelled).await;
            stats.was_cancelled = true;
            return (stats, items);
        }

        let _ = tx
            .send(CleanerEvent::ChannelStarted {
                channel_id: target_id.clone(),
                channel_name: target_name.clone(),
                index: target_index,
                total: by_target_len,
                phase: Phase::Delete,
            })
            .await;

        let entries_len = entries.len() as u64;
        let mut channel_stats = ChannelStats {
            found: entries_len,
            ..ChannelStats::default()
        };

        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                finish_cancelled(tx, &mut stats, channel_stats, &target_id).await;
                return (stats, items);
            }

            if dry_run {
                record_outcome(
                    &entry,
                    DeleteStep::Deleted,
                    &mut channel_stats,
                    &mut stats,
                    &mut items.succeeded,
                );
                emit_deleted(tx, history, &entry, DeleteOutcome::DryRun).await;
                continue;
            }

            match delete_with_retry(client, &entry, cancel, tx).await {
                Ok(()) => {
                    record_outcome(
                        &entry,
                        DeleteStep::Deleted,
                        &mut channel_stats,
                        &mut stats,
                        &mut items.succeeded,
                    );
                    emit_deleted(tx, history, &entry, DeleteOutcome::Deleted).await;
                }
                Err(IgError::Cancelled) => {
                    finish_cancelled(tx, &mut stats, channel_stats, &target_id).await;
                    return (stats, items);
                }
                Err(e) => {
                    let step = classify_delete_error(&e);
                    record_outcome(
                        &entry,
                        step,
                        &mut channel_stats,
                        &mut stats,
                        &mut items.succeeded,
                    );
                    match step {
                        DeleteStep::Failed => {
                            items.failed.insert(entry.key());
                        }
                        DeleteStep::Forbidden => {
                            items.skipped.insert(entry.key());
                        }
                        DeleteStep::Deleted | DeleteStep::AlreadyGone => {}
                    }
                    if step == DeleteStep::Failed {
                        let _ = tx
                            .send(CleanerEvent::MessageFailed {
                                message_id: entry.item_id.clone(),
                                error: e.to_string(),
                            })
                            .await;
                    } else {
                        emit_deleted(tx, history, &entry, step.outcome()).await;
                    }
                    if entry.kind == ItemKind::Unfollow && blocks_further_unfollows(&e) {
                        stats.unfollow_blocked = true;
                        let _ = tx
                            .send(CleanerEvent::Log {
                                level: LogLevel::Error,
                                message: format!(
                                    "unfollow: stopping, Instagram is refusing follow actions: {e}. \
                                     Try again later with a smaller --limit"
                                ),
                            })
                            .await;
                        break;
                    }
                }
            }
        }

        stats.channels_processed += 1;
        stats.aggregate(channel_stats);
        let _ = tx
            .send(CleanerEvent::ChannelDone {
                channel_id: target_id.clone(),
                stats: channel_stats,
                phase: Phase::Delete,
            })
            .await;
    }

    if cancel.load(Ordering::Relaxed) {
        let _ = tx.send(CleanerEvent::Cancelled).await;
        stats.was_cancelled = true;
    }

    (stats, items)
}

async fn finish_cancelled(
    tx: &mpsc::Sender<CleanerEvent>,
    stats: &mut RunStats,
    channel_stats: ChannelStats,
    target_id: &str,
) {
    let _ = tx.send(CleanerEvent::Cancelled).await;
    stats.was_cancelled = true;
    stats.channels_processed += 1;
    stats.aggregate(channel_stats);
    let _ = tx
        .send(CleanerEvent::ChannelDone {
            channel_id: target_id.to_string(),
            stats: channel_stats,
            phase: Phase::Delete,
        })
        .await;
}

async fn emit_deleted(
    tx: &mpsc::Sender<CleanerEvent>,
    history: &HistoryStore,
    entry: &PlannedDelete,
    outcome: DeleteOutcome,
) {
    let record = entry.to_record(outcome);
    if let Err(e) = history.append(&record) {
        let _ = tx
            .send(CleanerEvent::Warning {
                message: format!(
                    "history write failed for {}/{}: {e}",
                    entry.target_id, entry.item_id
                ),
            })
            .await;
    }
    let _ = tx
        .send(CleanerEvent::MessageDeleted {
            preview: entry.preview.clone(),
            outcome,
        })
        .await;
}

async fn delete_with_retry(
    client: &InstagramClient,
    entry: &PlannedDelete,
    cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<CleanerEvent>,
) -> IgResult<()> {
    let mut attempt = 0u8;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(IgError::Cancelled);
        }
        let result = match entry.kind {
            ItemKind::Media => match client.media_delete(&entry.item_id).await {
                Ok(ok) => map_delete_bool(ok, "media delete"),
                Err(e) => Err(e),
            },
            ItemKind::Dm => match client
                .direct_message_unsend(&entry.target_id, &entry.item_id)
                .await
            {
                Ok(ok) => map_delete_bool(ok, "dm unsend"),
                Err(e) => Err(e),
            },
            ItemKind::DmReaction => match client
                .direct_reaction_remove(
                    &entry.target_id,
                    &entry.item_id,
                    &entry.content,
                    entry.media_type.as_deref(),
                    entry.original_message_client_context.as_deref(),
                )
                .await
            {
                Ok(ok) => map_delete_bool(ok, "DM reaction remove"),
                Err(e) => Err(e),
            },
            ItemKind::Like => match client.media_unlike(&entry.item_id).await {
                Ok(ok) => map_delete_bool(ok, "unlike"),
                Err(e) => Err(e),
            },
            ItemKind::Saved => match client.media_unsave(&entry.item_id).await {
                Ok(ok) => map_delete_bool(ok, "unsave"),
                Err(e) => Err(e),
            },
            ItemKind::Archived => match client.media_delete(&entry.item_id).await {
                Ok(ok) => map_delete_bool(ok, "archived delete"),
                Err(e) => Err(e),
            },
            ItemKind::Comment => match client
                .comments_bulk_delete(&entry.target_id, std::slice::from_ref(&entry.item_id))
                .await
            {
                Ok(ok) => map_delete_bool(ok, "comment delete"),
                Err(e) => Err(e),
            },
            ItemKind::Repost => match client.media_note_delete(&entry.item_id, None).await {
                Ok(ok) => map_delete_bool(ok, "repost removal"),
                Err(e) => Err(e),
            },
            ItemKind::Unfollow => match client.user_unfollow(&entry.item_id).await {
                Ok(ok) => map_delete_bool(ok, "unfollow"),
                Err(e) => Err(e),
            },
        };
        match result {
            Ok(()) => return Ok(()),
            // the client has already waited out its cooldowns by the time this surfaces
            Err(e) if entry.kind == ItemKind::Unfollow && blocks_further_unfollows(&e) => {
                return Err(e)
            }
            Err(IgError::RateLimited {
                retry_after_ms,
                global,
                ..
            }) => {
                attempt += 1;
                if attempt > 2 {
                    return Err(IgError::RateLimited {
                        retry_after_ms,
                        global,
                        scope: "exhausted".into(),
                    });
                }
                let retry_after_ms = retry_after_ms.max(500);
                let _ = tx.send(CleanerEvent::RateLimited { retry_after_ms }).await;
                crate::ratelimit::poll_sleep_with_cancel(
                    std::time::Duration::from_millis(retry_after_ms),
                    cancel.clone(),
                )
                .await;
                if cancel.load(Ordering::Relaxed) {
                    return Err(IgError::Cancelled);
                }
            }
            Err(e) if is_retryable_delete_error(&e) => {
                attempt += 1;
                if attempt > 2 {
                    return Err(e);
                }
                let _ = tx
                    .send(CleanerEvent::Log {
                        level: LogLevel::Warning,
                        message: format!(
                            "delete {}/{} transient error, retrying ({attempt}/2): {e}",
                            entry.target_id, entry.item_id
                        ),
                    })
                    .await;
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                if cancel.load(Ordering::Relaxed) {
                    return Err(IgError::Cancelled);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

pub(crate) async fn hide_resolved_threads(
    client: &InstagramClient,
    threads: &[DirectThread],
    dry_run: bool,
    tx: &mpsc::Sender<CleanerEvent>,
) {
    let mut hidden: u64 = 0;
    let mut failed: u64 = 0;

    for thread in threads {
        let id = thread.id();
        let name = thread.display_name();
        if dry_run {
            let _ = tx
                .send(CleanerEvent::Log {
                    level: LogLevel::Warning,
                    message: format!("WOULD-HIDE {id}  {}", truncate(&name, 40)),
                })
                .await;
            hidden += 1;
            continue;
        }
        match client.direct_thread_hide(&id).await {
            Ok(true) | Err(IgError::NotFound { .. }) => {
                hidden += 1;
                let _ = tx
                    .send(CleanerEvent::Log {
                        level: LogLevel::Success,
                        message: format!("HIDE {id}  {}", truncate(&name, 40)),
                    })
                    .await;
            }
            Ok(false) => {
                failed += 1;
                let _ = tx
                    .send(CleanerEvent::Log {
                        level: LogLevel::Error,
                        message: format!("HIDE {id} rejected: Instagram returned status != ok"),
                    })
                    .await;
            }
            Err(IgError::Cancelled) => break,
            Err(e) => {
                failed += 1;
                let _ = tx
                    .send(CleanerEvent::Log {
                        level: LogLevel::Error,
                        message: format!("HIDE {id} failed: {e}"),
                    })
                    .await;
            }
        }
    }

    let summary = if dry_run {
        format!("hide threads: would hide {hidden} thread(s)")
    } else {
        format!("hide threads: hid {hidden} thread(s), {failed} failed")
    };
    let level = if failed > 0 {
        LogLevel::Warning
    } else {
        LogLevel::Info
    };
    let _ = tx
        .send(CleanerEvent::Log {
            level,
            message: summary,
        })
        .await;
}

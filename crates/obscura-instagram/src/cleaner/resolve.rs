use std::collections::HashSet;

use tokio::sync::mpsc;

use crate::client::{self, InstagramClient};
use crate::config::DmKind;
use crate::error::IgResult;
use crate::events::{CleanerEvent, LogLevel};
use crate::model::DirectThread;

use super::filters::{first_protected_user, thread_matches_kind};
use super::TargetResolution;

const MAX_INBOX_THREADS: usize = 2_000;

#[derive(Debug, Default)]
pub(crate) struct Skips {
    pub kind: usize,
    pub protected: usize,
    pub protected_names: Vec<String>,
}

// Single admission gate for every target kind: wrong dm_kind, already requested,
// or guarded by the protect list. `apply_kind` is off for thread IDs typed by hand;
// a pick hidden by the conversation type must not be touched without showing up.
pub(crate) fn admit_thread(
    thread: &DirectThread,
    dm_kind: DmKind,
    apply_kind: bool,
    protect: &HashSet<&str>,
    seen: &mut HashSet<String>,
    skips: &mut Skips,
) -> bool {
    if apply_kind && !thread_matches_kind(thread, dm_kind) {
        skips.kind += 1;
        return false;
    }
    let id = thread.id();
    if id.is_empty() || !seen.insert(id) {
        return false;
    }
    if let Some(name) = first_protected_user(thread, protect) {
        skips.protected += 1;
        skips.protected_names.push(name);
        return false;
    }
    true
}

#[derive(Debug, Default)]
pub(crate) struct ResolvedTargets {
    pub posts: bool,
    pub threads: Vec<DirectThread>,
    pub content: super::ContentFlags,
    /// False when threads are only in scope to have reactions removed.
    pub unsend_dms: bool,
}

pub(crate) async fn resolve_targets(
    client: &InstagramClient,
    targets: &[TargetResolution],
    content: super::ContentFlags,
    protect_users: &[String],
    dm_kind: DmKind,
    unsend_dms: bool,
    tx: &mpsc::Sender<CleanerEvent>,
) -> IgResult<ResolvedTargets> {
    let protect: HashSet<&str> = protect_users.iter().map(|s| s.as_str()).collect();
    let dm_scope = has_dm_scope(targets);
    let mut out = ResolvedTargets {
        content,
        unsend_dms: dm_scope && unsend_dms,
        ..ResolvedTargets::default()
    };
    let mut seen_thread_ids: HashSet<String> = HashSet::new();
    let mut skips = Skips::default();

    if !out.unsend_dms && content.dm_reactions {
        let _ = tx
            .send(CleanerEvent::Log {
                level: LogLevel::Info,
                message: "DM reactions only: your messages will be kept".into(),
            })
            .await;
    }

    for target in targets {
        match target {
            TargetResolution::Posts => {
                out.posts = true;
            }
            TargetResolution::AllDms => {
                let threads = client::list_all_threads(client, MAX_INBOX_THREADS).await?;
                if threads.len() >= MAX_INBOX_THREADS {
                    let _ = tx
                        .send(CleanerEvent::Warning {
                            message: format!(
                                "inbox walk capped at {MAX_INBOX_THREADS} threads, remaining not scanned"
                            ),
                        })
                        .await;
                }
                for t in threads {
                    if admit_thread(
                        &t,
                        dm_kind,
                        true,
                        &protect,
                        &mut seen_thread_ids,
                        &mut skips,
                    ) {
                        out.threads.push(t);
                    }
                }
            }
            TargetResolution::Threads(ids) => {
                for id in ids {
                    let id = id.trim();
                    if id.is_empty() || seen_thread_ids.contains(id) {
                        continue;
                    }
                    let _ = tx
                        .send(CleanerEvent::Log {
                            level: LogLevel::Info,
                            message: format!("loading thread {id}…"),
                        })
                        .await;
                    let thread = match client.direct_thread(id, 50, None).await {
                        Ok(t) => t,
                        Err(e) => {
                            let _ = tx
                                .send(CleanerEvent::Warning {
                                    message: format!("thread {id}: {e}"),
                                })
                                .await;
                            continue;
                        }
                    };
                    if admit_thread(
                        &thread,
                        dm_kind,
                        false,
                        &protect,
                        &mut seen_thread_ids,
                        &mut skips,
                    ) {
                        out.threads.push(thread);
                    }
                }
            }
            TargetResolution::ThreadObjects(threads) => {
                for thread in threads {
                    if admit_thread(
                        thread,
                        dm_kind,
                        true,
                        &protect,
                        &mut seen_thread_ids,
                        &mut skips,
                    ) {
                        // picks were listed a while ago: read them, don't trust that copy
                        let mut thread = thread.clone();
                        thread.items.clear();
                        out.threads.push(thread);
                    }
                }
            }
        }
    }

    if skips.kind > 0 {
        let _ = tx
            .send(CleanerEvent::Log {
                level: LogLevel::Info,
                message: format!(
                    "skipped {} thread(s) that are not {}",
                    skips.kind,
                    dm_kind.label()
                ),
            })
            .await;
    }

    if skips.protected > 0 {
        skips.protected_names.sort();
        skips.protected_names.dedup();
        let _ = tx
            .send(CleanerEvent::Log {
                level: LogLevel::Warning,
                message: format!(
                    "skipped {} protected thread(s): {}",
                    skips.protected,
                    skips.protected_names.join(", ")
                ),
            })
            .await;
    }

    Ok(out)
}

pub(crate) fn has_dm_scope(targets: &[TargetResolution]) -> bool {
    targets.iter().any(|target| {
        matches!(
            target,
            TargetResolution::AllDms
                | TargetResolution::Threads(_)
                | TargetResolution::ThreadObjects(_)
        )
    })
}

pub(crate) fn target_count(resolved: &ResolvedTargets) -> usize {
    // dm-reactions is handled inside the thread scans, so it has no pass of its own
    let content_passes = resolved.content.count() - usize::from(resolved.content.dm_reactions);
    resolved.threads.len() + usize::from(resolved.posts) + content_passes
}

use std::collections::HashSet;

use futures_util::stream::{self, StreamExt};
use tokio::sync::mpsc;

use crate::client::DiscordClient;
use crate::config::DmKind;
use crate::error::DiscordResult;
use crate::events::{LogLevel, PrunerEvent};
use crate::model::Channel;

use super::filters::{channel_matches_kind, first_protected_channel};
use super::TargetResolution;

const GUILD_CHANNEL_CONCURRENCY: usize = 4;

#[derive(Debug, Default)]
pub(crate) struct Skips {
    pub kind: usize,
    pub protected: usize,
    pub protected_names: Vec<String>,
    pub non_text: usize,
}

// Single admission gate for every channel: wrong dm_kind, non-text,
// already seen, or guarded by the protect list. `apply_kind` is off for
// explicitly named channels.
pub(crate) fn admit_channel(
    channel: &Channel,
    dm_kind: DmKind,
    apply_kind: bool,
    protect: &HashSet<&str>,
    seen: &mut HashSet<String>,
    skips: &mut Skips,
) -> bool {
    if !channel.is_dm_like() && !channel.is_guild_text_like() {
        skips.non_text += 1;
        return false;
    }
    if apply_kind && channel.is_dm_like() && !channel_matches_kind(channel, dm_kind) {
        skips.kind += 1;
        return false;
    }
    let id = channel.id();
    if id.is_empty() || !seen.insert(id) {
        return false;
    }
    if let Some(name) = first_protected_channel(channel, protect) {
        skips.protected += 1;
        skips.protected_names.push(name);
        return false;
    }
    true
}

#[derive(Debug, Default)]
pub(crate) struct ResolvedTargets {
    pub channels: Vec<Channel>,
    pub guilds: Vec<(String, String)>,
}

pub(crate) async fn resolve_targets(
    client: &DiscordClient,
    targets: &[TargetResolution],
    protect_ids: &[String],
    dm_kind: DmKind,
    tx: &mpsc::Sender<PrunerEvent>,
) -> DiscordResult<ResolvedTargets> {
    let protect: HashSet<&str> = protect_ids.iter().map(|s| s.as_str()).collect();
    let mut out = ResolvedTargets::default();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut skips = Skips::default();

    for target in targets {
        match target {
            TargetResolution::AllDms => {
                let channels = client.private_channels().await?;
                for c in channels {
                    if admit_channel(&c, dm_kind, true, &protect, &mut seen_ids, &mut skips) {
                        out.channels.push(c);
                    }
                }
            }
            TargetResolution::AllGuilds => {
                let guilds = client.guilds().await?;
                let _ = tx
                    .send(PrunerEvent::Log {
                        level: LogLevel::Info,
                        message: format!("resolving channels in {} guild(s)…", guilds.len()),
                    })
                    .await;
                let guild_ids: Vec<_> = guilds
                    .iter()
                    .map(|g| (g.id.clone(), g.name.clone()))
                    .collect();
                let mut fetched = stream::iter(guilds.into_iter().map(|g| async move {
                    let result = client.guild_channels(&g.id).await;
                    (g.id, result)
                }))
                .buffer_unordered(GUILD_CHANNEL_CONCURRENCY);

                while let Some((guild_id, result)) = fetched.next().await {
                    match result {
                        Ok(channels) => {
                            let parent_ids: Vec<String> = channels
                                .iter()
                                .filter(|channel| channel.is_thread_parent())
                                .map(Channel::id)
                                .filter(|id| !id.is_empty())
                                .collect();
                            let (threads, thread_errors) =
                                collect_guild_threads(client, &guild_id, &parent_ids).await;
                            for error in thread_errors {
                                let _ = tx
                                    .send(PrunerEvent::Log {
                                        level: LogLevel::Warning,
                                        message: error,
                                    })
                                    .await;
                            }
                            let mut channels = channels;
                            channels.extend(threads);
                            for mut c in channels {
                                if c.guild_id.is_none() {
                                    c.guild_id = Some(guild_id.clone());
                                }
                                if admit_channel(
                                    &c,
                                    dm_kind,
                                    false,
                                    &protect,
                                    &mut seen_ids,
                                    &mut skips,
                                ) {
                                    out.channels.push(c);
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send(PrunerEvent::Log {
                                    level: LogLevel::Warning,
                                    message: format!("guild {guild_id} channels failed: {e}"),
                                })
                                .await;
                        }
                    }
                }
                out.guilds.extend(guild_ids);
            }
            TargetResolution::Channels(ids) => {
                for id in ids {
                    let id = id.trim();
                    if id.is_empty() || seen_ids.contains(id) {
                        continue;
                    }
                    let _ = tx
                        .send(PrunerEvent::Log {
                            level: LogLevel::Info,
                            message: format!("loading channel {id}…"),
                        })
                        .await;
                    let channel = match client.get_channel(id).await {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx
                                .send(PrunerEvent::Log {
                                    level: LogLevel::Warning,
                                    message: format!("channel {id}: {e}"),
                                })
                                .await;
                            continue;
                        }
                    };
                    if admit_channel(
                        &channel,
                        dm_kind,
                        false,
                        &protect,
                        &mut seen_ids,
                        &mut skips,
                    ) {
                        out.channels.push(channel);
                    }
                }
            }
            TargetResolution::ChannelObjects(channels) => {
                for channel in channels {
                    if admit_channel(channel, dm_kind, false, &protect, &mut seen_ids, &mut skips) {
                        out.channels.push(channel.clone());
                    }
                }
            }
        }
    }

    if skips.kind > 0 {
        let _ = tx
            .send(PrunerEvent::Log {
                level: LogLevel::Info,
                message: format!(
                    "skipped {} channel(s) not matching dm_kind={}",
                    skips.kind,
                    dm_kind.label()
                ),
            })
            .await;
    }
    if skips.non_text > 0 {
        let _ = tx
            .send(PrunerEvent::Log {
                level: LogLevel::Info,
                message: format!(
                    "skipped {} non-text channel(s) (voice/stage/category)",
                    skips.non_text
                ),
            })
            .await;
    }

    if skips.protected > 0 {
        skips.protected_names.sort();
        skips.protected_names.dedup();
        let _ = tx
            .send(PrunerEvent::Log {
                level: LogLevel::Warning,
                message: format!(
                    "skipped {} protected channel(s): {}",
                    skips.protected,
                    skips.protected_names.join(", ")
                ),
            })
            .await;
    }

    Ok(out)
}

async fn collect_guild_threads(
    client: &DiscordClient,
    guild_id: &str,
    parent_ids: &[String],
) -> (Vec<Channel>, Vec<String>) {
    let mut threads = Vec::new();
    let mut errors = Vec::new();
    match client.active_threads(guild_id).await {
        Ok(mut active) => threads.append(&mut active),
        Err(error) => errors.push(format!("guild {guild_id} active threads failed: {error}")),
    }

    let mut archived = stream::iter(parent_ids.iter().cloned().map(|parent_id| async move {
        let (public, private, joined) = tokio::join!(
            client.public_archived_threads(&parent_id),
            client.private_archived_threads(&parent_id),
            client.joined_private_archived_threads(&parent_id),
        );
        (parent_id, public, private, joined)
    }))
    .buffer_unordered(GUILD_CHANNEL_CONCURRENCY);

    while let Some((parent_id, public, private, joined)) = archived.next().await {
        append_archived(&mut threads, &mut errors, &parent_id, "public", public);
        append_archived(&mut threads, &mut errors, &parent_id, "private", private);
        append_archived(
            &mut threads,
            &mut errors,
            &parent_id,
            "joined private",
            joined,
        );
    }

    let mut seen = HashSet::new();
    threads.retain(|thread| !thread.id.is_empty() && seen.insert(thread.id.clone()));
    (threads, errors)
}

fn append_archived(
    threads: &mut Vec<Channel>,
    errors: &mut Vec<String>,
    parent_id: &str,
    label: &str,
    result: DiscordResult<Vec<Channel>>,
) {
    match result {
        Ok(mut page) => threads.append(&mut page),
        Err(
            crate::error::DiscordError::Forbidden { .. }
            | crate::error::DiscordError::NotFound { .. },
        ) => {}
        Err(error) => errors.push(format!(
            "{label} archived threads for channel {parent_id} failed: {error}"
        )),
    }
}

pub(crate) fn target_count(resolved: &ResolvedTargets) -> usize {
    resolved.channels.len()
}

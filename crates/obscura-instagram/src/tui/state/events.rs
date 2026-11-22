use crate::events::{CleanerEvent, LogLevel, Phase};
use crate::history::DeleteOutcome;

use super::super::widgets::human_duration;
use super::{AppState, ChannelProgress, Connection, RunStatus};

impl AppState {
    fn deleted_word(&self) -> &'static str {
        if self.run.dry_run {
            "would delete"
        } else {
            "deleted"
        }
    }

    pub fn apply_event(&mut self, event: CleanerEvent) {
        match event {
            CleanerEvent::Started { channel_count } => {
                self.run.stats.channel_count = channel_count;
                self.run.push_log(
                    LogLevel::Info,
                    format!("scanning {channel_count} target(s)"),
                );
            }
            CleanerEvent::ChannelStarted {
                channel_id,
                channel_name,
                index,
                total,
                phase,
            } => {
                self.run.phase = phase;
                self.run.start_row(&channel_id, &channel_name, phase);
                self.run.current_progress = Some(ChannelProgress {
                    channel_id,
                    channel_name: channel_name.clone(),
                });
                let position = index + 1;
                self.run.push_log(
                    LogLevel::Info,
                    format!("[{position}/{total}] {channel_name}"),
                );
            }
            CleanerEvent::ScanProgress { scanned, found } => {
                self.run.stats.messages_scanned += scanned;
                self.run.found += found;
            }
            CleanerEvent::MessageDeleted { preview, outcome } => {
                self.run.plan_done += 1;
                let refused = outcome == DeleteOutcome::Forbidden;
                if refused {
                    self.run.stats.messages_skipped += 1;
                } else {
                    self.run.stats.messages_deleted += 1;
                }
                if let Some(row) = self.run.current_row_mut() {
                    if refused {
                        row.stats.skipped += 1;
                    } else {
                        row.stats.deleted += 1;
                    }
                }
                self.run.push_log(
                    LogLevel::Success,
                    format!("{}: {preview}", outcome.log_prefix()),
                );
            }
            CleanerEvent::MessageFailed { message_id, error } => {
                self.run.plan_done += 1;
                self.run.stats.messages_failed += 1;
                if let Some(row) = self.run.current_row_mut() {
                    row.stats.failed += 1;
                }
                self.run
                    .push_log(LogLevel::Error, format!("failed {message_id}: {error}"));
            }
            CleanerEvent::ChannelDone {
                channel_id,
                stats,
                phase,
            } => {
                let deleted_word = self.deleted_word();
                // scans overlap, so the row is found by id rather than by whichever started last
                let summary = self.run.row_mut(&channel_id).map(|row| {
                    match phase {
                        Phase::Scan => {
                            row.stats.scanned = stats.scanned;
                            row.stats.found = stats.found;
                        }
                        Phase::Delete => {
                            let attempted =
                                row.stats.deleted + row.stats.skipped + row.stats.failed;
                            row.stats.found = row.stats.found.max(stats.found).max(attempted);
                        }
                    }
                    row.name.clone()
                });
                let name = summary.unwrap_or_else(|| channel_id.clone());
                let message = match phase {
                    Phase::Scan => format!(
                        "scanned {name}: found {} of {} checked",
                        stats.found, stats.scanned
                    ),
                    Phase::Delete => format!(
                        "finished {name}: {deleted_word} {}, skipped {}, failed {}",
                        stats.deleted, stats.skipped, stats.failed
                    ),
                };
                self.run.push_log(LogLevel::Info, message);
                if self
                    .run
                    .current_progress
                    .as_ref()
                    .is_some_and(|current| current.channel_id == channel_id)
                {
                    self.run.current_progress = None;
                }
            }
            CleanerEvent::Retrying { counts } => {
                let run = &mut self.run;
                for retry in counts {
                    let stats = &mut run.stats;
                    stats.messages_failed = stats.messages_failed.saturating_sub(retry.failed);
                    stats.messages_skipped = stats.messages_skipped.saturating_sub(retry.skipped);
                    run.plan_done = run
                        .plan_done
                        .saturating_sub((retry.failed + retry.skipped) as usize);
                    if let Some(row) = run.channels.iter_mut().find(|r| r.id == retry.target_id) {
                        row.stats.failed = row.stats.failed.saturating_sub(retry.failed);
                        row.stats.skipped = row.stats.skipped.saturating_sub(retry.skipped);
                    }
                }
            }
            CleanerEvent::Verifying => self.run.verifying = true,
            CleanerEvent::PlanReady { total } => {
                // a re-scan only retries a few of them, so this can't read as a new plan
                let message = if self.run.verifying {
                    format!("plan updated: {total} item(s) found in total")
                } else {
                    format!("plan ready: {total} item(s) to delete")
                };
                self.run.verifying = false;
                self.run.found = total as u64;
                self.run.plan_total = total;
                self.run.phase = Phase::Delete;
                self.run.push_log(LogLevel::Success, message);
            }
            CleanerEvent::RateLimited { retry_after_ms } => {
                self.run.push_log(
                    LogLevel::Warning,
                    format!("rate limited: waiting {retry_after_ms}ms"),
                );
            }
            CleanerEvent::Warning { message } => {
                self.run.warnings += 1;
                self.run.push_log(LogLevel::Warning, message);
            }
            CleanerEvent::Log { level, message } => {
                self.run.push_log(level, message);
            }
            CleanerEvent::Done { stats } => {
                self.run.engine_active = false;
                // `Error` already ended the run and these stats would wipe what it counted
                if self.run.status == RunStatus::Failed {
                    return;
                }
                let (status, level, outcome) = if stats.was_cancelled {
                    (RunStatus::Cancelled, LogLevel::Warning, "run cancelled")
                } else {
                    (RunStatus::Completed, LogLevel::Success, "run complete")
                };
                self.run.status = status;
                self.run.finished_at = Some(std::time::Instant::now());
                self.run.stats = stats;
                self.run.found = stats.plan_total as u64;
                self.run.plan_total = stats.plan_total;
                self.run.cancelling = false;
                self.run.verifying = false;
                self.run.rate_limiter.cooldown_remaining_ms = 0;
                self.run.current_progress = None;
                let elapsed = human_duration(self.run.elapsed());
                let deleted_word = self.deleted_word();
                let not_attempted = self.run.not_attempted();
                self.run.push_log(
                    level,
                    format!(
                        "{outcome}: found {}, {deleted_word} {}, skipped {}, failed {}, \
                         not attempted {not_attempted} in {elapsed}",
                        stats.plan_total,
                        stats.messages_deleted,
                        stats.messages_skipped,
                        stats.messages_failed
                    ),
                );
                self.archive.reload();
            }
            // the run stays active until `Done`, so a new one can't start on top of it
            CleanerEvent::Cancelled => {
                self.run.cancelling = true;
                self.run
                    .push_log(LogLevel::Warning, "cancelling: finishing up");
            }
            CleanerEvent::Error { message } => {
                self.run.status = RunStatus::Failed;
                self.run.finished_at = Some(std::time::Instant::now());
                self.run.current_progress = None;
                self.run.cancelling = false;
                self.run.verifying = false;
                self.run.rate_limiter.cooldown_remaining_ms = 0;
                self.run
                    .push_log(LogLevel::Error, format!("error: {message}"));
            }
            CleanerEvent::Identity { username } => {
                self.connection = Connection::Ready;
                self.identity = Some(username);
            }
            CleanerEvent::DMsLoaded { channels } => {
                self.targets.loading = false;
                let before = self.targets.selected_threads.len();
                self.targets
                    .selected_threads
                    .retain(|id| channels.iter().any(|t| &t.thread_id == id));
                let dropped = before - self.targets.selected_threads.len();
                self.targets.threads = channels;
                self.targets.clamp_cursor(self.form.dm_kind);
                if dropped > 0 {
                    self.set_toast(
                        format!("unpicked {dropped} thread(s) no longer in the inbox"),
                        LogLevel::Warning,
                    );
                }
            }
            CleanerEvent::DMsLoadFailed { message } => {
                self.targets.loading = false;
                self.set_toast(
                    format!("could not load threads: {message}"),
                    LogLevel::Error,
                );
            }
            CleanerEvent::ProbeFailed { message } => {
                self.connection = Connection::Failed;
                self.targets.loading = false;
                self.set_toast(format!("connection failed: {message}"), LogLevel::Error);
            }
        }
    }
}

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::events::{ChannelStats, LogEntry, LogLevel, Phase, RunStats};

use super::{LOG_CAP, SPARKLINE_CAP};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelProgress {
    pub channel_id: String,
    pub channel_name: String,
}

/// One target's counts. Scanning and deleting a target both land on the same row.
#[derive(Debug, Clone, Default)]
pub struct TargetRow {
    pub id: String,
    pub name: String,
    pub stats: ChannelStats,
    pub deleting: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RateLimiterView {
    pub cooldown_remaining_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunStatus {
    #[default]
    Idle,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone)]
pub struct RunState {
    pub status: RunStatus,
    pub phase: Phase,
    /// What this run was started as, which the form can no longer change.
    pub dry_run: bool,
    /// Asked to stop, and still finishing what it was doing.
    pub cancelling: bool,
    pub verifying: bool,
    /// A run's task may still send events, so a new run can't start yet.
    pub engine_active: bool,
    pub started_at: Option<Instant>,
    pub finished_at: Option<Instant>,
    pub channels: Vec<TargetRow>,
    pub current_progress: Option<ChannelProgress>,
    pub stats: RunStats,
    /// Items matching the filters: a running count while scanning, then the plan's size.
    pub found: u64,
    pub plan_total: usize,
    pub plan_done: usize,
    pub warnings: u64,
    pub log: VecDeque<LogEntry>,
    pub log_scroll: usize,
    pub rate_limiter: RateLimiterView,
    pub sparkline_data: VecDeque<u64>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            status: RunStatus::Idle,
            phase: Phase::Scan,
            dry_run: false,
            cancelling: false,
            verifying: false,
            engine_active: false,
            started_at: None,
            finished_at: None,
            channels: Vec::new(),
            current_progress: None,
            stats: RunStats::default(),
            found: 0,
            plan_total: 0,
            plan_done: 0,
            warnings: 0,
            log: VecDeque::new(),
            log_scroll: 0,
            rate_limiter: RateLimiterView::default(),
            sparkline_data: VecDeque::new(),
        }
    }
}

impl RunState {
    pub fn is_running(&self) -> bool {
        self.status == RunStatus::Running
    }

    pub fn elapsed(&self) -> Duration {
        match (self.started_at, self.finished_at) {
            (Some(s), Some(f)) => f.duration_since(s),
            (Some(s), None) => Instant::now().duration_since(s),
            _ => Duration::ZERO,
        }
    }

    pub fn phase_label(&self) -> &'static str {
        match (self.cancelling, self.verifying, self.phase) {
            (true, _, _) => "cancelling",
            (_, true, _) => "verifying",
            (_, _, Phase::Scan) => "scanning",
            (_, _, Phase::Delete) => "deleting",
        }
    }

    /// Found items that no delete was tried on, because the run stopped early.
    pub fn not_attempted(&self) -> u64 {
        let stats = &self.stats;
        self.found
            .saturating_sub(stats.messages_deleted)
            .saturating_sub(stats.messages_skipped)
            .saturating_sub(stats.messages_failed)
    }

    pub fn scroll_log(&mut self, older: bool) {
        self.log_scroll = if older {
            (self.log_scroll + 1).min(self.log.len().saturating_sub(1))
        } else {
            self.log_scroll.saturating_sub(1)
        };
    }

    pub fn push_log(&mut self, level: LogLevel, message: impl Into<String>) {
        if self.log.len() >= LOG_CAP {
            self.log.pop_front();
        }
        // keeps the lines being read in place while new ones arrive
        if self.log_scroll > 0 {
            self.log_scroll = (self.log_scroll + 1).min(self.log.len());
        }
        self.log.push_back(LogEntry {
            level,
            message: message.into(),
            at: std::time::SystemTime::now(),
        });
    }

    /// Moves the target to the end of the list, where the newest activity lives.
    pub fn start_row(&mut self, id: &str, name: &str, phase: Phase) {
        let mut row = match self.channels.iter().position(|row| row.id == id) {
            Some(index) => self.channels.remove(index),
            None => TargetRow {
                id: id.to_string(),
                name: name.to_string(),
                ..TargetRow::default()
            },
        };
        row.deleting |= phase == Phase::Delete;
        self.channels.push(row);
    }

    pub fn row_mut(&mut self, id: &str) -> Option<&mut TargetRow> {
        self.channels.iter_mut().find(|row| row.id == id)
    }

    pub fn current_row_mut(&mut self) -> Option<&mut TargetRow> {
        let id = self.current_progress.as_ref()?.channel_id.clone();
        self.row_mut(&id)
    }

    /// Takes the limiter's own numbers, which include waits the client handled itself.
    pub fn sync_rate_limits(&mut self, hits: u64, cooldown: Duration) {
        let cooldown_ms = cooldown.as_millis() as u64;
        if hits > self.stats.rate_limit_hits {
            self.push_rate(cooldown_ms);
        }
        self.stats.rate_limit_hits = hits;
        self.rate_limiter.cooldown_remaining_ms = cooldown_ms;
    }

    pub fn push_rate(&mut self, ms: u64) {
        if self.sparkline_data.len() >= SPARKLINE_CAP {
            self.sparkline_data.pop_front();
        }
        self.sparkline_data.push_back(ms);
    }
}

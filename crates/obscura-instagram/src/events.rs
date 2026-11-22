pub use obscura_core::events::{LogEntry, LogLevel, Phase};

#[derive(Debug, Clone, Copy, Default)]
pub struct ChannelStats {
    pub scanned: u64,
    /// Items in this target that matched the filters and are planned for deletion.
    pub found: u64,
    pub deleted: u64,
    pub skipped: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RunStats {
    pub channel_count: usize,
    pub channels_processed: usize,
    pub messages_scanned: u64,
    pub messages_deleted: u64,
    /// Refused by Instagram. Items never attempted are `plan_total` minus the rest.
    pub messages_skipped: u64,
    pub messages_failed: u64,
    pub rate_limit_hits: u64,
    pub dm_messages_deleted: u64,
    pub plan_total: usize,
    pub was_cancelled: bool,
    /// Instagram refused an unfollow in a way that retrying would only make worse.
    pub unfollow_blocked: bool,
}

impl RunStats {
    pub fn aggregate(&mut self, cs: ChannelStats) {
        self.messages_scanned += cs.scanned;
        self.messages_deleted += cs.deleted;
        self.messages_skipped += cs.skipped;
        self.messages_failed += cs.failed;
    }
}

/// How many items of one target are about to be attempted again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryCount {
    pub target_id: String,
    pub failed: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum CleanerEvent {
    Started {
        channel_count: usize,
    },
    ChannelStarted {
        channel_id: String,
        channel_name: String,
        index: usize,
        total: usize,
        phase: Phase,
    },
    MessageDeleted {
        preview: String,
        outcome: crate::history::DeleteOutcome,
    },
    /// Something the run could not check and left alone.
    Warning {
        message: String,
    },
    MessageFailed {
        message_id: String,
        error: String,
    },
    ChannelDone {
        channel_id: String,
        stats: ChannelStats,
        phase: Phase,
    },
    PlanReady {
        total: usize,
    },
    /// What a scan has newly counted since its last report, across all targets.
    ScanProgress {
        scanned: u64,
        found: u64,
    },
    /// Items that failed or were refused earlier and are about to be attempted again.
    Retrying {
        counts: Vec<RetryCount>,
    },
    /// The deletes are done and the run is scanning again to confirm them.
    Verifying,
    RateLimited {
        retry_after_ms: u64,
    },
    Log {
        level: LogLevel,
        message: String,
    },
    Done {
        stats: RunStats,
    },
    Cancelled,
    Error {
        message: String,
    },
    DMsLoaded {
        channels: Vec<crate::model::DirectThread>,
    },
    DMsLoadFailed {
        message: String,
    },
    Identity {
        username: String,
    },
    ProbeFailed {
        message: String,
    },
}

//! Small event vocabulary shared by every platform.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Scan,
    Delete,
}

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
    pub at: std::time::SystemTime,
}

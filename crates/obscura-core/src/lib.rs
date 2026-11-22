//! Shared building blocks for obscura's platform plugins.
//!
//! Everything here is platform-agnostic: the archive format, the shared
//! rate limiter, date parsing, terminal chrome and small utilities. The
//! Discord and Instagram crates keep their own clients and scan/delete
//! engines, but they write to the same archive and reuse the same UI parts.

pub mod clipboard;
pub mod dates;
pub mod events;
pub mod history;
pub mod ratelimit;
pub mod theme;
pub mod tui;
pub mod util;

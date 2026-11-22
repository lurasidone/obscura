//! Instagram rate limiter: grows the wait on repeated 429s.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

pub use obscura_core::ratelimit::poll_sleep_with_cancel;
pub use obscura_core::ratelimit::Backoff;
use obscura_core::ratelimit::RateLimiter as CoreRateLimiter;

#[derive(Debug, Clone)]
pub struct RateLimiter(CoreRateLimiter);

impl RateLimiter {
    pub fn new(user_throttle: Duration, cancel: Arc<AtomicBool>) -> Self {
        Self(CoreRateLimiter::with_backoff(
            user_throttle,
            cancel,
            Backoff::Exponential,
        ))
    }
}

impl std::ops::Deref for RateLimiter {
    type Target = CoreRateLimiter;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

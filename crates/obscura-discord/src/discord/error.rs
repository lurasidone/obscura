use thiserror::Error;

#[derive(Debug, Error)]
pub enum DiscordError {
    #[error("invalid token (HTTP {status})")]
    InvalidToken { status: u16 },

    #[error("missing permission for {resource} (HTTP {status})")]
    Forbidden { resource: String, status: u16 },

    #[error("resource not found: {resource} (HTTP {status})")]
    NotFound { resource: String, status: u16 },

    #[error("rate limited; retry after {retry_after_ms}ms")]
    RateLimited {
        retry_after_ms: u64,
        global: bool,
        scope: String,
    },

    #[error("captcha required: {message}")]
    CaptchaRequired { message: String },

    #[error("cloudflare ban: {message}")]
    CloudflareBan { message: String },

    #[error("cancelled")]
    Cancelled,

    #[error("Discord API error: HTTP {status}: {message}")]
    Api { status: u16, message: String },

    #[error("HTTP error: {0}")]
    Http(#[from] wreq::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid token")]
    BadToken,

    #[error("{0}")]
    Other(String),
}

pub type DiscordResult<T> = Result<T, DiscordError>;

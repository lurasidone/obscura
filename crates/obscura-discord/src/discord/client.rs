//! REST client modelled after `discord.py-self`'s `HTTPClient::request`
//! (`discord/http.py`): Chrome-on-Windows web-client headers, raw user-token
//! auth, `X-Super-Properties`, and Discord-style 429 / bucket handling.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use wreq::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_TYPE,
};
use wreq_util::{Emulation, Platform, Profile};

use super::error::{DiscordError, DiscordResult};
use super::headers::{DiscordIdentity, API_BASE, API_DOMAIN};
use super::model::{Channel, Guild, Message, ThreadListResponse, User};
use super::ratelimit::RateLimiter;
use super::session::{resolve_session_path, SessionStore};

// ---- Per-route Discord rate-limit bucket tracking ----
// Mirrors discord.py-self's _bucket_hashes + _buckets dicts.

#[derive(Debug, Default)]
struct BucketState {
    /// Requests remaining in the current window (from X-Ratelimit-Remaining).
    remaining: i64,
    /// Duration in seconds until the window resets (from X-Ratelimit-Reset-After).
    reset_after: f64,
    /// Wall-clock instant when the window expires.
    expires_at: Option<std::time::Instant>,
}

impl BucketState {
    fn update(&mut self, remaining: i64, reset_after: f64) {
        self.remaining = remaining;
        self.reset_after = reset_after;
        self.expires_at = Some(std::time::Instant::now() + Duration::from_secs_f64(reset_after));
    }

    fn is_expired(&self) -> bool {
        self.expires_at
            .is_none_or(|t| std::time::Instant::now() >= t)
    }

    /// Returns how long to pre-emptively sleep if remaining == 0.
    fn sleep_duration(&self) -> Option<Duration> {
        if self.remaining > 0 {
            return None;
        }
        self.expires_at
            .and_then(|t| t.checked_duration_since(std::time::Instant::now()))
    }
}

#[derive(Debug, Default)]
struct BucketMap {
    /// Route key ("METHOD /path") → Discord bucket hash.
    hashes: std::collections::HashMap<String, String>,
    /// Bucket key (hash + major params, or route key + major params) → state.
    buckets: std::collections::HashMap<String, BucketState>,
}

impl BucketMap {
    fn route_key(method: &str, path: &str) -> String {
        let parts: Vec<_> = path.split('/').collect();
        let mut normalized = Vec::with_capacity(parts.len());
        for (i, part) in parts.iter().enumerate() {
            let previous = i.checked_sub(1).and_then(|j| parts.get(j)).copied();
            let replacement = match previous {
                Some("channels") => Some("{channel_id}"),
                Some("guilds") => Some("{guild_id}"),
                Some("messages") => Some("{message_id}"),
                _ => None,
            };
            normalized.push(if is_snowflake(part) {
                replacement.unwrap_or(part)
            } else {
                part
            });
        }
        format!("{method} {}", normalized.join("/"))
    }

    fn bucket_key(&self, method: &str, path: &str) -> String {
        let rkey = Self::route_key(method, path);
        let major = major_parameter(path);
        match self.hashes.get(&rkey) {
            Some(hash) => format!("{hash}:{major}"),
            None => format!("{rkey}:{major}"),
        }
    }

    /// Record a new Discord bucket hash for a route, returning the new bucket key.
    fn record_hash(&mut self, method: &str, path: &str, discord_hash: &str) -> String {
        let rkey = Self::route_key(method, path);
        let major = major_parameter(path);
        let old_hash = self.hashes.get(&rkey).cloned();
        if old_hash.as_deref() != Some(discord_hash) {
            // Migrate existing state from old bucket key to new one.
            if let Some(old) = &old_hash {
                let old_key = format!("{old}:{major}");
                let new_key = format!("{discord_hash}:{major}");
                if let Some(state) = self.buckets.remove(&old_key) {
                    self.buckets.insert(new_key, state);
                }
                tracing::debug!("bucket hash change for {rkey}: {old} -> {discord_hash}");
            }
            self.hashes.insert(rkey, discord_hash.to_owned());
        }
        format!("{discord_hash}:{major}")
    }

    fn get_or_default(&mut self, key: &str) -> &mut BucketState {
        self.buckets.entry(key.to_owned()).or_default()
    }

    /// Evict buckets when the map grows large (mirrors discord.py-self's 256-entry cap).
    fn maybe_evict(&mut self) {
        if self.buckets.len() < 256 {
            return;
        }
        self.buckets.retain(|_, v| !v.is_expired());
    }
}

#[derive(Debug)]
struct LiveAuth {
    token: String,
    user_id: String,
    username: String,
}

pub struct DiscordClient {
    http: wreq::Client,
    identity: DiscordIdentity,
    live: Mutex<LiveAuth>,
    rate_limiter: Arc<RateLimiter>,
    buckets: Mutex<BucketMap>,
    base_url: String,
    session_path: PathBuf,
}

impl DiscordClient {
    pub async fn new(
        token: &str,
        base_url: Option<&str>,
        session_path: Option<&str>,
        rate_limiter: Arc<RateLimiter>,
    ) -> DiscordResult<Self> {
        let session_path = resolve_session_path(session_path);
        let provided = token.trim();
        let mut store = SessionStore::load(&session_path).unwrap_or_default();
        let token = if !provided.is_empty() {
            let t = normalize_token(provided)?;
            if store.token != t {
                store.token = t.clone();
                store.user_id.clear();
                store.username.clear();
            }
            t
        } else if !store.token.is_empty() {
            normalize_token(&store.token.clone())?
        } else {
            return Err(DiscordError::BadToken);
        };

        // discord.py-self refreshes its default web HeadersContext at startup.
        let identity = DiscordIdentity::load().await;

        let base_url = base_url
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(API_BASE)
            .trim_end_matches('/')
            .to_string();

        // Match the Chrome TLS profile to the dynamically fetched browser major version.
        let emulation = Emulation::builder()
            .profile(chrome_profile(identity.browser_version))
            .platform(Platform::Windows)
            .headers(false)
            .build();
        let http = wreq::Client::builder()
            .emulation(emulation)
            .timeout(Duration::from_secs(30))
            .build()?;

        let live = LiveAuth {
            token: token.clone(),
            user_id: store.user_id.clone(),
            username: store.username.clone(),
        };
        let client = Self {
            http,
            identity,
            live: Mutex::new(live),
            rate_limiter,
            buckets: Mutex::new(BucketMap::default()),
            base_url,
            session_path,
        };

        let me = client.fetch_me().await?;
        {
            let mut live = client.live.lock().expect("live auth poisoned");
            live.user_id = me.id.clone();
            live.username = me.username.clone();
        }
        client.persist_session();

        Ok(client)
    }

    pub fn user_id(&self) -> String {
        self.live
            .lock()
            .expect("live auth poisoned")
            .user_id
            .clone()
    }

    pub fn username(&self) -> String {
        self.live
            .lock()
            .expect("live auth poisoned")
            .username
            .clone()
    }

    async fn fetch_me(&self) -> DiscordResult<User> {
        let resp = self.get("users/@me", &[]).await?;
        parse_json(resp).await
    }

    // ---- discord.py-self route surface we need ----

    /// `GET /users/@me/channels` — DM + group DM channels.
    pub async fn private_channels(&self) -> DiscordResult<Vec<Channel>> {
        let resp = self.get("users/@me/channels", &[]).await?;
        parse_json(resp).await
    }

    /// `GET /users/@me/guilds` — guilds the account is in.
    pub async fn guilds(&self) -> DiscordResult<Vec<Guild>> {
        const PAGE_SIZE: usize = 200;
        let mut guilds = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let limit = PAGE_SIZE.to_string();
            let mut params = vec![("limit", limit.as_str()), ("with_counts", "1")];
            if let Some(cursor) = after.as_deref() {
                params.push(("after", cursor));
            }
            let resp = self.get("users/@me/guilds", &params).await?;
            let page: Vec<Guild> = parse_json(resp).await?;
            if page.len() < PAGE_SIZE {
                guilds.extend(page);
                break;
            }
            let Some(cursor) = page
                .last()
                .map(|g| g.id.clone())
                .filter(|id| !id.is_empty())
            else {
                guilds.extend(page);
                break;
            };
            if after.as_deref() == Some(cursor.as_str()) {
                guilds.extend(page);
                break;
            }
            guilds.extend(page);
            after = Some(cursor);
        }
        Ok(guilds)
    }

    /// `GET /guilds/{guild_id}/channels` — text-ish channels of one guild.
    pub async fn guild_channels(&self, guild_id: &str) -> DiscordResult<Vec<Channel>> {
        let resp = self
            .get(&format!("guilds/{guild_id}/channels"), &[])
            .await?;
        parse_json(resp).await
    }

    /// `GET /guilds/{guild_id}/threads/active`.
    pub async fn active_threads(&self, guild_id: &str) -> DiscordResult<Vec<Channel>> {
        let resp = self
            .get(&format!("guilds/{guild_id}/threads/active"), &[])
            .await?;
        let body: ThreadListResponse = parse_json(resp).await?;
        Ok(body.threads)
    }

    /// Fetch every archived thread visible through a channel's archive route.
    pub async fn public_archived_threads(&self, channel_id: &str) -> DiscordResult<Vec<Channel>> {
        self.archived_threads(channel_id, "public").await
    }

    /// Fetch private archived threads for accounts with Manage Threads.
    pub async fn private_archived_threads(&self, channel_id: &str) -> DiscordResult<Vec<Channel>> {
        self.archived_threads(channel_id, "private").await
    }

    /// Fetch private archived threads the current account has joined.
    pub async fn joined_private_archived_threads(
        &self,
        channel_id: &str,
    ) -> DiscordResult<Vec<Channel>> {
        self.archived_threads(channel_id, "users/@me/threads/archived/private")
            .await
    }

    async fn archived_threads(
        &self,
        channel_id: &str,
        archive_route: &str,
    ) -> DiscordResult<Vec<Channel>> {
        const PAGE_SIZE: &str = "50";
        let path = if archive_route.starts_with("users/") {
            format!("channels/{channel_id}/{archive_route}")
        } else {
            format!("channels/{channel_id}/threads/archived/{archive_route}")
        };
        // Discord's public/private archive endpoints page by archive timestamp;
        // the joined-private endpoint pages by thread snowflake.
        let joined_private = archive_route.starts_with("users/");
        let mut threads = Vec::new();
        let mut seen = HashSet::new();
        let mut before: Option<String> = None;
        loop {
            let mut params = vec![("limit", PAGE_SIZE)];
            if let Some(cursor) = before.as_deref() {
                params.push(("before", cursor));
            }
            let resp = self.get(&path, &params).await?;
            let page: ThreadListResponse = parse_json(resp).await?;
            let next = page.threads.last().and_then(|thread| {
                if joined_private {
                    (!thread.id.is_empty()).then(|| thread.id.clone())
                } else {
                    thread
                        .thread_metadata
                        .as_ref()
                        .and_then(|metadata| metadata.archive_timestamp.clone())
                        .filter(|timestamp| !timestamp.is_empty())
                }
            });
            for thread in page.threads {
                if !thread.id.is_empty() && seen.insert(thread.id.clone()) {
                    threads.push(thread);
                }
            }
            if !page.has_more || next.is_none() || next == before {
                break;
            }
            before = next;
        }
        Ok(threads)
    }

    /// `GET /channels/{channel_id}` — single channel (DM or guild).
    pub async fn get_channel(&self, channel_id: &str) -> DiscordResult<Channel> {
        let resp = self.get(&format!("channels/{channel_id}"), &[]).await?;
        parse_json(resp).await
    }

    /// `GET /channels/{channel_id}/messages` — discord.py-self `logs_from`.
    /// `limit` clamped to 1-100 like the real API.
    pub async fn channel_messages(
        &self,
        channel_id: &str,
        limit: u32,
        before: Option<&str>,
        after: Option<&str>,
        around: Option<&str>,
    ) -> DiscordResult<Vec<Message>> {
        let limit_s = limit.clamp(1, 100).to_string();
        let mut params: Vec<(&str, &str)> = vec![("limit", limit_s.as_str())];
        if let Some(c) = before.filter(|s| !s.is_empty()) {
            params.push(("before", c));
        }
        if let Some(c) = after.filter(|s| !s.is_empty()) {
            params.push(("after", c));
        }
        if let Some(c) = around.filter(|s| !s.is_empty()) {
            params.push(("around", c));
        }
        let resp = self
            .get(&format!("channels/{channel_id}/messages"), &params)
            .await?;
        parse_json(resp).await
    }

    /// `GET /guilds/{guild_id}/messages/search` — indexed guild search.
    /// Retries 202 Accepted (indexing code 11xxxx) up to 5 times with `attempts` parameter,
    /// mirroring discord.py-self's `HTTPClient.request`.
    pub async fn search_guild_messages(
        &self,
        guild_id: &str,
        params: &[(&str, &str)],
    ) -> DiscordResult<Value> {
        let path = format!("guilds/{guild_id}/messages/search");
        let mut owned_params: Vec<(String, String)> = params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        for attempt in 1..=5 {
            let p_refs: Vec<(&str, &str)> = owned_params
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let resp = self.get(&path, &p_refs).await?;
            let status = resp.status().as_u16();
            let val: Value = parse_json(resp).await?;
            let is_indexing = status == 202
                || val
                    .get("code")
                    .and_then(Value::as_i64)
                    .is_some_and(|c| (110000..120000).contains(&c));
            if is_indexing && attempt < 5 {
                if self.rate_limiter.is_cancelled() {
                    return Err(DiscordError::Cancelled);
                }
                let retry_after = val
                    .get("retry_after")
                    .and_then(Value::as_f64)
                    .unwrap_or(5.0);
                tracing::info!(
                    "guild {guild_id} search indexing (code 110000); retrying in {retry_after}s (attempt {attempt}/5)"
                );
                tokio::time::sleep(Duration::from_secs_f64(retry_after.max(1.0))).await;
                if self.rate_limiter.is_cancelled() {
                    return Err(DiscordError::Cancelled);
                }
                if let Some(pos) = owned_params.iter().position(|(k, _)| k == "attempts") {
                    owned_params[pos].1 = attempt.to_string();
                } else {
                    owned_params.push(("attempts".to_string(), attempt.to_string()));
                }
                continue;
            }
            return Ok(val);
        }
        Err(DiscordError::Other(format!(
            "guild {guild_id} search indexing timed out"
        )))
    }

    /// `DELETE /channels/{channel_id}/messages/{message_id}` —
    /// discord.py-self `delete_message` (bucket metadata for <10s / >14d kept
    /// conceptually; Discord buckets server-side, we just call through).
    pub async fn delete_message(&self, channel_id: &str, message_id: &str) -> DiscordResult<bool> {
        let resp = self
            .delete(&format!("channels/{channel_id}/messages/{message_id}"))
            .await?;
        let status = resp.status().as_u16();
        if status == 204 || (200..300).contains(&status) {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Close a 1:1 DM: discord.py-self closes DMs with
    /// `DELETE /channels/{channel_id}`. Guild channels 403 here by design.
    /// Never call this on group DMs (type 3) — it leaves the group.
    /// Callers must filter to `Channel::is_dm()` first.
    pub async fn close_dm(&self, channel_id: &str) -> DiscordResult<bool> {
        let resp = self.delete(&format!("channels/{channel_id}")).await?;
        let status = resp.status().as_u16();
        Ok((200..300).contains(&status))
    }

    // ---- HTTP plumbing (discord.py-self `HTTPClient::request`) ----

    async fn get(&self, path: &str, params: &[(&str, &str)]) -> DiscordResult<wreq::Response> {
        self.request("GET", path, params, None).await
    }

    async fn delete(&self, path: &str) -> DiscordResult<wreq::Response> {
        self.request("DELETE", path, &[], None).await
    }

    #[allow(clippy::too_many_lines)]
    async fn request(
        &self,
        method: &str,
        path: &str,
        params: &[(&str, &str)],
        body: Option<&Value>,
    ) -> DiscordResult<wreq::Response> {
        for attempt in 0..5u32 {
            if self.rate_limiter.is_cancelled() {
                return Err(DiscordError::Cancelled);
            }
            self.rate_limiter.wait().await;
            if self.rate_limiter.is_cancelled() {
                return Err(DiscordError::Cancelled);
            }

            let url = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
            let mut builder = match method {
                "GET" => self.http.get(&url),
                "POST" => self.http.post(&url),
                "DELETE" => self.http.delete(&url),
                "PATCH" => self.http.patch(&url),
                "PUT" => self.http.put(&url),
                other => return Err(DiscordError::Other(format!("unsupported method {other}"))),
            };
            builder = builder.headers(self.base_headers(body.is_some()));
            if !params.is_empty() {
                builder = builder.query(params);
            }
            if let Some(data) = body {
                builder = builder.body(serde_json::to_string(data).unwrap_or_else(|_| "{}".into()));
            }

            // Pre-emptive bucket sleep — extract duration then drop the lock before
            // awaiting so the MutexGuard does not cross the await point (not Send).
            let preempt_sleep: Option<Duration> = {
                let mut bmap = self.buckets.lock().expect("bucket map poisoned");
                let bkey = bmap.bucket_key(method, path);
                let state = bmap.get_or_default(&bkey);
                state
                    .sleep_duration()
                    .map(|d| d + Duration::from_millis(50))
            }; // lock released here
            if let Some(sleep_dur) = preempt_sleep {
                tracing::debug!("pre-emptive bucket sleep {sleep_dur:?} for {method} {path}");
                tokio::time::sleep(sleep_dur).await;
            }

            let resp = match builder.send().await {
                Ok(r) => r,
                Err(e) if attempt < 4 && !self.rate_limiter.is_cancelled() => {
                    tracing::warn!("network error on {path}: {e}; retrying (attempt {attempt})");
                    tokio::time::sleep(Duration::from_secs(1 + u64::from(attempt) * 2)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let status = resp.status().as_u16();

            // Update per-route bucket state from response headers.
            let response_bucket_hash = {
                let headers = resp.headers();
                let discord_hash = headers
                    .get("x-ratelimit-bucket")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let remaining = headers
                    .get("x-ratelimit-remaining")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<i64>().ok());
                let reset_after = headers
                    .get("x-ratelimit-reset-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<f64>().ok());
                if let (Some(remaining), Some(reset_after)) = (remaining, reset_after) {
                    let mut bmap = self.buckets.lock().expect("bucket map poisoned");
                    bmap.maybe_evict();
                    let bkey = if let Some(hash) = &discord_hash {
                        bmap.record_hash(method, path, hash)
                    } else {
                        bmap.bucket_key(method, path)
                    };
                    if status != 429 {
                        bmap.get_or_default(&bkey).update(remaining, reset_after);
                        if remaining == 0 {
                            tracing::debug!(
                                "bucket exhausted for {method} {path} (reset in {reset_after:.2}s)"
                            );
                        }
                    }
                }
                discord_hash
            };

            if matches!(status, 502 | 504 | 507 | 522 | 523 | 524)
                && attempt < 4
                && !self.rate_limiter.is_cancelled()
            {
                // discord.py-self retries 502/504/507/522/523/524 unconditionally.
                tracing::warn!(
                    "HTTP {status} server error on {path}; retrying (attempt {attempt})"
                );
                tokio::time::sleep(Duration::from_secs(1 + u64::from(attempt) * 2)).await;
                continue;
            }

            if status == 429 {
                let (retry_ms, global) = parse_429(resp).await.unwrap_or((60_000, false));
                let scope = if global {
                    self.rate_limiter.observe_rate_limit(retry_ms);
                    "global"
                } else {
                    let mut bmap = self.buckets.lock().expect("bucket map poisoned");
                    let bkey = if let Some(hash) = response_bucket_hash.as_deref() {
                        bmap.record_hash(method, path, hash)
                    } else {
                        bmap.bucket_key(method, path)
                    };
                    bmap.get_or_default(&bkey)
                        .update(0, retry_ms as f64 / 1000.0);
                    "bucket"
                };
                tracing::warn!(
                    "HTTP 429 on {path}; cooldown {retry_ms}ms ({scope}, attempt {attempt})"
                );
                if attempt + 1 == 5 {
                    return Err(DiscordError::RateLimited {
                        retry_after_ms: retry_ms,
                        global,
                        scope: scope.into(),
                    });
                }
                continue;
            }

            if status == 404 {
                return Err(DiscordError::NotFound {
                    resource: path.to_string(),
                    status,
                });
            }

            if !(200..300).contains(&status) {
                let text = resp.text().await.unwrap_or_default();
                let (message, code) = api_message_code(&text);

                if status == 401 {
                    return Err(DiscordError::InvalidToken { status });
                }
                if let Some(msg) = &message {
                    let lower = msg.to_ascii_lowercase();
                    if lower.contains("captcha") {
                        return Err(DiscordError::CaptchaRequired {
                            message: msg.clone(),
                        });
                    }
                    if lower.contains("cloudflare") || code == Some(0) && status == 403 {
                        return Err(DiscordError::CloudflareBan {
                            message: msg.clone(),
                        });
                    }
                }

                return match status {
                    403 => Err(DiscordError::Forbidden {
                        resource: path.to_string(),
                        status,
                    }),
                    _ => Err(DiscordError::Api {
                        status,
                        message: message.unwrap_or_else(|| truncate_err(&text)),
                    }),
                };
            }

            return Ok(resp);
        }
        Err(DiscordError::Other("exhausted retries".into()))
    }

    /// Header set copied from discord.py-self `HTTPClient.request` +
    /// `HeadersContext.client_hints`.
    fn base_headers(&self, is_json: bool) -> HeaderMap {
        let live = self.live.lock().expect("live auth poisoned");
        let mut headers = HeaderMap::new();

        set(&mut headers, "Sec-CH-UA", &self.identity.sec_ch_ua());
        set(&mut headers, "Sec-CH-UA-Mobile", "?0");
        set(&mut headers, "Sec-CH-UA-Platform", "\"Windows\"");
        set(&mut headers, "Priority", "u=0, i");
        set(&mut headers, "Accept", "*/*");
        set(&mut headers, "Accept-Language", "en-US,en;q=0.9");
        set(&mut headers, ACCEPT_ENCODING, "gzip, deflate, br, zstd");
        set(&mut headers, "Origin", "https://discord.com");
        set(&mut headers, "Referer", "https://discord.com/channels/@me");
        set(&mut headers, "Sec-Fetch-Dest", "empty");
        set(&mut headers, "Sec-Fetch-Mode", "cors");
        set(&mut headers, "Sec-Fetch-Site", "same-origin");
        set(&mut headers, "User-Agent", &self.identity.user_agent);
        set(&mut headers, "X-Discord-Locale", &self.identity.locale);
        set(
            &mut headers,
            "X-Super-Properties",
            &self.identity.super_properties_b64,
        );
        set(&mut headers, "X-Discord-Timezone", &self.identity.timezone);
        // discord.py-self sends Authorization as the raw user token.
        set(&mut headers, AUTHORIZATION, &live.token);
        if is_json {
            set(&mut headers, CONTENT_TYPE, "application/json");
        }
        set(&mut headers, "Host", API_DOMAIN);

        headers
    }

    fn persist_session(&self) {
        let live = self.live.lock().expect("live auth poisoned");
        let store = SessionStore {
            token: live.token.clone(),
            user_id: live.user_id.clone(),
            username: live.username.clone(),
        };
        if let Err(e) = store.save(&self.session_path) {
            tracing::warn!(
                "failed to persist session to {}: {e}",
                self.session_path.display()
            );
        }
    }
}

fn is_snowflake(value: &str) -> bool {
    value.len() >= 15 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn major_parameter(path: &str) -> &str {
    let parts: Vec<_> = path.split('/').collect();
    parts
        .windows(2)
        .find(|pair| matches!(pair[0], "channels" | "guilds") && is_snowflake(pair[1]))
        .map(|pair| pair[1])
        .unwrap_or("")
}

fn chrome_profile(version: u32) -> Profile {
    match version {
        149.. => Profile::Chrome149,
        148 => Profile::Chrome148,
        147 => Profile::Chrome147,
        146 => Profile::Chrome146,
        145 => Profile::Chrome145,
        144 => Profile::Chrome144,
        143 => Profile::Chrome143,
        142 => Profile::Chrome142,
        141 => Profile::Chrome141,
        140 => Profile::Chrome140,
        139 => Profile::Chrome139,
        138 => Profile::Chrome138,
        137 => Profile::Chrome137,
        _ => Profile::Chrome136,
    }
}

impl Drop for DiscordClient {
    fn drop(&mut self) {
        self.persist_session();
    }
}

fn set(map: &mut HeaderMap, key: impl AsHeaderName, value: &str) {
    if let Ok(val) = HeaderValue::from_str(value) {
        key.insert(map, val);
    }
}

trait AsHeaderName {
    fn insert(self, map: &mut HeaderMap, val: HeaderValue);
}

impl AsHeaderName for HeaderName {
    fn insert(self, map: &mut HeaderMap, val: HeaderValue) {
        map.insert(self, val);
    }
}

impl AsHeaderName for &'static str {
    fn insert(self, map: &mut HeaderMap, val: HeaderValue) {
        if let Ok(name) = HeaderName::from_bytes(self.as_bytes()) {
            map.insert(name, val);
        }
    }
}

fn normalize_token(raw: &str) -> DiscordResult<String> {
    let s = raw.trim().trim_matches('"');
    // Accept raw token or a pasted header/value pair; never log it.
    let s = if let Some(idx) = s.find("Authorization:") {
        s[idx + "Authorization:".len()..].trim()
    } else {
        s
    };
    let s = s.split_whitespace().next().unwrap_or(s).trim();
    if s.len() < 20 {
        return Err(DiscordError::BadToken);
    }
    Ok(s.to_string())
}

/// Parse a 429 the way discord.py-self does: prefer the JSON body's
/// `retry_after` (seconds, float) + `global`, fall back to `Retry-After`.
async fn parse_429(resp: wreq::Response) -> Option<(u64, bool)> {
    let retry_header_ms = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|secs| (secs * 1000.0) as u64);
    let body = resp.text().await.ok();
    if let Some(body) = body {
        if let Ok(json) = serde_json::from_str::<Value>(&body) {
            let mut retry_ms = json
                .get("retry_after")
                .and_then(Value::as_f64)
                .map(|seconds| (seconds * 1000.0) as u64)
                .or(retry_header_ms)
                .unwrap_or(0);
            // Discord's edge sometimes reports Retry-After in seconds while
            // the JSON retry_after value is milliseconds (and vice versa).
            if retry_header_ms == Some(retry_ms / 1000) {
                retry_ms /= 1000;
            }
            return Some((
                retry_ms,
                json.get("global").and_then(Value::as_bool).unwrap_or(false),
            ));
        }
    }
    retry_header_ms.map(|ms| (ms, false))
}

fn api_message_code(text: &str) -> (Option<String>, Option<i64>) {
    let Some(v) = serde_json::from_str::<Value>(text).ok() else {
        return (None, None);
    };
    let message = v.get("message").and_then(Value::as_str).map(str::to_string);
    let code = v.get("code").and_then(Value::as_i64);
    (message, code)
}

async fn parse_json<T: serde::de::DeserializeOwned>(resp: wreq::Response) -> DiscordResult<T> {
    let bytes = resp.bytes().await?;
    match serde_json::from_slice(&bytes) {
        Ok(val) => Ok(val),
        Err(e) => {
            let preview = String::from_utf8_lossy(&bytes);
            tracing::warn!(
                "JSON parse error ({e}) on response: {}",
                truncate_err(&preview)
            );
            Err(DiscordError::Json(e))
        }
    }
}

fn truncate_err(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.trim().chars();
    for c in chars.by_ref().take(240) {
        out.push(c);
    }
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_raw_and_header_forms() {
        let s = normalize_token("  abcdefghijklmnopqrstuvwx  ").unwrap();
        assert!(s.starts_with("abc"));
        assert!(normalize_token("short").is_err());
    }
}

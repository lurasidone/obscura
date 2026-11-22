//! Client fingerprint modelled after `discord.py-self` (`discord/tracking.py`
//! `HeadersContext::default()` + `HTTPClient::request`).
//!
//! discord.py-self sends, on every REST call:
//! - `Authorization: <user token>` (raw, no `Bot` prefix)
//! - `User-Agent: <browser_user_agent from super properties>`
//! - `X-Super-Properties: base64(json)` describing the Chrome/Windows web client
//! - `Origin: https://discord.com`, `Referer: .../channels/@me`
//! - `Sec-Fetch-*` + `Sec-CH-UA*` client hints, `X-Discord-Locale`, `X-Discord-Timezone`
//!
//! The default web properties are fetched from the same properties service used
//! by discord.py-self. Static values are only a fallback when that service is
//! unavailable.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::json;

pub const API_DOMAIN: &str = "discord.com";
pub const API_BASE: &str = "https://discord.com/api/v9";
pub const DEFAULT_SESSION_PATH: &str = "data/discord/session.json";

/// Recent-ish Chrome major version; must match the TLS profile (`Chrome136`).
pub const BROWSER_MAJOR_VERSION: u32 = 136;
/// Plausible web-client build number (discord.py-self scrapes this live;
/// we keep a constant + env override instead).
pub const CLIENT_BUILD_NUMBER: u32 = 9_999;

#[derive(Debug, Clone)]
pub struct DiscordIdentity {
    pub browser_version: u32,
    pub user_agent: String,
    pub super_properties_b64: String,
    pub timezone: String,
    pub locale: String,
}

impl DiscordIdentity {
    pub async fn load() -> Self {
        let bootstrap = match wreq::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
        {
            Ok(client) => client,
            Err(_) => return Self::fresh(),
        };
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            bootstrap
                .post("https://cordapi.dolfi.es/api/v2/properties/web")
                .query(&[("release_channel", "stable")])
                .header("User-Agent", "discord.py-self")
                .send(),
        )
        .await;
        let Ok(Ok(response)) = response else {
            return load_fallback(&bootstrap).await;
        };
        if !response.status().is_success() {
            return load_fallback(&bootstrap).await;
        }
        let Ok(body) = response.text().await else {
            return load_fallback(&bootstrap).await;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
            return load_fallback(&bootstrap).await;
        };
        let Some(properties) = value.get("properties").filter(|v| v.is_object()).cloned() else {
            return load_fallback(&bootstrap).await;
        };
        let browser_version = properties
            .get("browser_version")
            .and_then(serde_json::Value::as_str)
            .and_then(|v| v.split('.').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(BROWSER_MAJOR_VERSION);
        let user_agent = properties
            .get("browser_user_agent")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format_chrome_ua(browser_version));
        let super_properties_b64 = value
            .get("encoded")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| BASE64.encode(serde_json::to_vec(&properties).unwrap_or_default()));
        let timezone = std::env::var("DISCORD_TIMEZONE")
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(detect_system_timezone);
        let locale = properties
            .get("system_locale")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("en-US")
            .to_owned();
        Self {
            browser_version,
            user_agent,
            super_properties_b64,
            timezone,
            locale,
        }
    }

    pub fn fresh() -> Self {
        let browser_version = std::env::var("DISCORD_BROWSER_VERSION")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(BROWSER_MAJOR_VERSION);
        let build_number = std::env::var("DISCORD_BUILD_NUMBER")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(CLIENT_BUILD_NUMBER);
        Self::with_versions(browser_version, build_number)
    }

    pub fn with_versions(browser_version: u32, build_number: u32) -> Self {
        let user_agent = format_chrome_ua(browser_version);
        let super_properties = json!({
            "os": "Windows",
            "browser": "Chrome",
            "device": "",
            "system_locale": "en-US",
            "browser_user_agent": user_agent,
            "browser_version": format!("{browser_version}.0.0.0"),
            "os_version": "10",
            "referrer": "",
            "referring_domain": "",
            "referrer_current": "",
            "referring_domain_current": "",
            "release_channel": "stable",
            "client_build_number": build_number,
            "client_event_source": null,
            "has_client_mods": false,
            "client_launch_id": uuid::Uuid::new_v4().to_string(),
            "client_app_state": "unfocused",
            "client_heartbeat_session_id": uuid::Uuid::new_v4().to_string(),
            "launch_signature": uuid::Uuid::new_v4().to_string(),
        });
        let super_properties_b64 =
            BASE64.encode(serde_json::to_vec(&super_properties).unwrap_or_default());
        let timezone = std::env::var("DISCORD_TIMEZONE")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(detect_system_timezone);
        Self {
            browser_version,
            user_agent,
            super_properties_b64,
            timezone,
            locale: "en-US".to_string(),
        }
    }

    /// `Sec-CH-UA` value matching what discord.py-self's `client_hints` sends
    /// for Chrome on Windows.
    pub fn sec_ch_ua(&self) -> String {
        let v = self.browser_version;
        let chars = [' ', '(', ':', '-', '.', '/', ')', ';', '=', '?', '_'];
        let versions = ["8", "99", "24"];
        let grease = format!(
            "Not{}A{}Brand",
            chars[v as usize % chars.len()],
            chars[(v as usize + 1) % chars.len()]
        );
        let grease_version = versions[v as usize % versions.len()];
        let brands = [
            ("Not", grease.as_str(), grease_version),
            ("Chromium", "Chromium", ""),
            ("Google Chrome", "Google Chrome", ""),
        ];
        let orders = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        let order = orders[v as usize % orders.len()];
        order
            .iter()
            .map(|i| {
                let (_, name, version) = brands[*i];
                if *i == 0 {
                    format!("\"{name}\";v=\"{version}\"")
                } else {
                    format!("\"{name}\";v=\"{v}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

async fn load_fallback(http: &wreq::Client) -> DiscordIdentity {
    let build = fetch_discord_build(http)
        .await
        .unwrap_or(CLIENT_BUILD_NUMBER);
    let browser = fetch_chrome_version(http)
        .await
        .unwrap_or(BROWSER_MAJOR_VERSION);
    DiscordIdentity::with_versions(browser, build)
}

async fn fetch_discord_build(http: &wreq::Client) -> Option<u32> {
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        http.get("https://discord.com/login").send(),
    )
    .await
    .ok()?
    .ok()?;
    let page = response.text().await.ok()?;
    let marker = page.find("\"BUILD_NUMBER\"")?;
    let tail = page.get(marker + "\"BUILD_NUMBER\"".len()..)?;
    let start = tail.find(|c: char| c.is_ascii_digit())?;
    let digits: String = tail[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

async fn fetch_chrome_version(http: &wreq::Client) -> Option<u32> {
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        http.get("https://versionhistory.googleapis.com/v1/chrome/platforms/win/channels/stable/versions").send(),
    ).await.ok()?.ok()?;
    let body = response.text().await.ok()?;
    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    value
        .get("versions")?
        .as_array()?
        .first()?
        .get("version")?
        .as_str()?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Detect the local IANA timezone name, mirroring what `tzlocal.get_localzone_name()`
/// does in discord.py-self.
///
/// Strategy (Linux/macOS):
/// 1. `$TZ` env var (stripped of any leading `:`)
/// 2. `/etc/localtime` → resolve symlink → strip the zoneinfo prefix → IANA name
/// 3. `/etc/timezone` plain text file (Debian/Ubuntu)
/// 4. Fallback: `"UTC"`
fn detect_system_timezone() -> String {
    // 1. $TZ env var
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim().trim_start_matches(':').to_owned();
        if !tz.is_empty() && !tz.starts_with('/') {
            return tz;
        }
    }

    // 2. /etc/localtime symlink  →  …/zoneinfo/<Region/City>
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        for marker in &["zoneinfo/", "zoneinfo\\"] {
            if let Some(pos) = s.find(marker) {
                let name = &s[pos + marker.len()..];
                if !name.is_empty() {
                    return name.to_owned();
                }
            }
        }
    }

    // 3. /etc/timezone  (plain text, e.g. "America/New_York\n")
    if let Ok(content) = std::fs::read_to_string("/etc/timezone") {
        let name = content.trim().to_owned();
        if !name.is_empty() {
            return name;
        }
    }

    "UTC".to_owned()
}

fn format_chrome_ua(browser_version: u32) -> String {
    format!(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{browser_version}.0.0.0 Safari/537.36"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn super_properties_encode_roundtrip() {
        let id = DiscordIdentity::with_versions(136, 12345);
        assert!(id.user_agent.contains("Chrome/136"));
        let decoded = BASE64.decode(&id.super_properties_b64).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(v["client_build_number"], 12345);
        assert_eq!(v["os"], "Windows");
    }
}

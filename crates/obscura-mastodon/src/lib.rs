use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wreq::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, LINK};
use wreq::Client;

pub mod config {
    #[derive(Clone, Debug)]
    pub struct Config {
        pub instance_url: String,
        pub token: String,
        pub before: String,
        pub after: String,
        pub include: Vec<String>,
        pub exclude: Vec<String>,
        pub limit: String,
        pub history_path: String,
        pub dry_run: bool,
    }

    impl Config {
        pub fn from_env() -> Self {
            Self {
                instance_url: env("MASTODON_INSTANCE_URL", "https://mastodon.social"),
                token: env("MASTODON_TOKEN", ""),
                before: env("MASTODON_BEFORE", ""),
                after: env("MASTODON_AFTER", ""),
                include: list("MASTODON_INCLUDE"),
                exclude: list("MASTODON_EXCLUDE"),
                limit: env("MASTODON_LIMIT", "0"),
                history_path: env("MASTODON_HISTORY_PATH", "data/mastodon/history.jsonl"),
                dry_run: env("MASTODON_DRY_RUN", "true").parse().unwrap_or(true),
            }
        }
    }
    fn env(key: &str, default: &str) -> String {
        std::env::var(key).unwrap_or_else(|_| default.to_owned())
    }
    fn list(key: &str) -> Vec<String> {
        env(key, "")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect()
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "obscura-mastodon",
    about = "Review and delete your own Mastodon statuses"
)]
struct Args {
    /// Instance origin, for example https://mastodon.social
    #[arg(long)]
    instance_url: Option<String>,
    /// OAuth access token (or MASTODON_TOKEN)
    #[arg(long)]
    token: Option<String>,
    #[arg(long)]
    before: Option<String>,
    #[arg(long)]
    after: Option<String>,
    /// Comma-separated terms; status must contain any term
    #[arg(long)]
    include: Option<String>,
    /// Comma-separated terms; statuses containing any are skipped
    #[arg(long)]
    exclude: Option<String>,
    /// Maximum planned statuses; zero means unlimited
    #[arg(long)]
    limit: Option<usize>,
    /// Append successful-delete records here
    #[arg(long)]
    history: Option<String>,
    /// Explicitly disable the default dry run; requires typing DELETE
    #[arg(long, conflicts_with = "dry_run")]
    live: bool,
    /// Force a non-destructive dry run
    #[arg(long, conflicts_with = "live")]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
struct Account {
    id: String,
    username: String,
    acct: String,
}
#[derive(Debug, Deserialize)]
struct Reblog {
    content: String,
}
#[derive(Debug, Deserialize)]
struct Status {
    id: String,
    created_at: DateTime<Utc>,
    content: String,
    url: Option<String>,
    reblog: Option<Reblog>,
}
#[derive(Serialize)]
struct HistoryRecord<'a> {
    platform: &'static str,
    status_id: &'a str,
    url: &'a Option<String>,
    created_at: DateTime<Utc>,
    deleted_at: DateTime<Utc>,
    account: &'a str,
}

struct Api {
    client: Client,
    base: String,
}
impl Api {
    fn new(base: &str, token: &str) -> Result<Self> {
        let parsed = url::Url::parse(base).context("invalid Mastodon instance URL")?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            bail!("instance URL must be an https origin or base path without credentials, query, or fragment");
        }
        let base = base.trim_end_matches('/').to_owned();
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
        Ok(Self {
            client: Client::builder().default_headers(headers).build()?,
            base,
        })
    }
    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
    async fn json(&self, url: &str) -> Result<(Value, Option<String>)> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Mastodon API request failed")?;
        let status = response.status();
        let link = response
            .headers()
            .get(LINK)
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        let body = response
            .text()
            .await
            .context("reading Mastodon API response")?;
        if !status.is_success() {
            bail!("Mastodon API returned {status}: {}", truncate(&body));
        }
        Ok((
            serde_json::from_str(&body).context("invalid Mastodon API JSON")?,
            link,
        ))
    }
    async fn delete(&self, id: &str) -> Result<()> {
        let response = self
            .client
            .delete(self.endpoint(&format!("/api/v1/statuses/{id}")))
            .send()
            .await
            .context("deleting Mastodon status")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Mastodon API returned {status}: {}", truncate(&body));
        }
        Ok(())
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(300).collect()
}
fn plain_content(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.to_lowercase()
}
fn terms(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}
fn next_url(link: Option<&str>) -> Option<String> {
    link?
        .split(',')
        .find(|part| part.contains("rel=\"next\""))?
        .split(';')
        .next()?
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_owned()
        .into()
}
fn status_text(status: &Status) -> String {
    plain_content(
        status
            .reblog
            .as_ref()
            .map_or(&status.content, |r| &r.content),
    )
}
fn matches(
    status: &Status,
    before: Option<&DateTime<Utc>>,
    after: Option<&DateTime<Utc>>,
    include: &[String],
    exclude: &[String],
) -> bool {
    if before.is_some_and(|b| &status.created_at >= b)
        || after.is_some_and(|a| &status.created_at <= a)
    {
        return false;
    }
    let text = status_text(status);
    (include.is_empty() || include.iter().any(|t| text.contains(t)))
        && !exclude.iter().any(|t| text.contains(t))
}

pub fn cli_main() -> Result<()> {
    cli_main_from(std::env::args_os())
}
pub fn cli_main_from<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let _ = dotenvy::dotenv();
    let args = Args::parse_from(args);
    let mut cfg = config::Config::from_env();
    if let Some(v) = args.instance_url {
        cfg.instance_url = v;
    }
    if let Some(v) = args.token {
        cfg.token = v;
    }
    if let Some(v) = args.before {
        cfg.before = v;
    }
    if let Some(v) = args.after {
        cfg.after = v;
    }
    if let Some(v) = args.include {
        cfg.include = terms(&v);
    }
    if let Some(v) = args.exclude {
        cfg.exclude = terms(&v);
    }
    if let Some(v) = args.limit {
        cfg.limit = v.to_string();
    }
    if let Some(v) = args.history {
        cfg.history_path = v;
    }
    if args.dry_run {
        cfg.dry_run = true;
    }
    if args.live {
        cfg.dry_run = false;
    }
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(run(cfg))
}

pub async fn run(cfg: config::Config) -> Result<()> {
    if cfg.token.trim().is_empty() {
        bail!("MASTODON_TOKEN is empty; create an OAuth access token and set it in .env or pass --token");
    }
    let limit = cfg
        .limit
        .parse::<usize>()
        .context("MASTODON_LIMIT/--limit must be a non-negative integer")?;
    let before = obscura_core::dates::validate_date_input(
        &cfg.before,
        obscura_core::dates::BoundStyle::ExcludeDay,
    )
    .map_err(anyhow::Error::msg)?;
    let after = obscura_core::dates::validate_date_input(
        &cfg.after,
        obscura_core::dates::BoundStyle::ExcludeDay,
    )
    .map_err(anyhow::Error::msg)?;
    if let (Some(before), Some(after)) = (before.as_ref(), after.as_ref()) {
        if after >= before {
            bail!("--after must be earlier than --before");
        }
    }
    let api = Api::new(&cfg.instance_url, cfg.token.trim())?;
    let (acct, _) = api
        .json(&api.endpoint("/api/v1/accounts/verify_credentials"))
        .await
        .context("verify Mastodon credentials")?;
    let account: Account =
        serde_json::from_value(acct).context("unexpected verify_credentials response")?;
    println!(
        "Connected as @{} ({}) on {}",
        account.acct, account.username, cfg.instance_url
    );
    let account_url = api.endpoint(&format!(
        "/api/v1/accounts/{}/statuses?limit=40&exclude_replies=false&exclude_reblogs=false",
        account.id
    ));
    let mut page = Some(account_url);
    let mut seen_pages = std::collections::HashSet::new();
    let mut selected = Vec::new();
    while let Some(url) = page {
        if !seen_pages.insert(url.clone()) {
            bail!("Mastodon pagination loop detected; aborting without deletion");
        }
        let (value, link) = api.json(&url).await.context("fetch own statuses")?;
        let statuses: Vec<Status> =
            serde_json::from_value(value).context("unexpected statuses response")?;
        for status in statuses {
            // Boosts are authored wrapper statuses; filters inspect the boosted content.
            if matches(
                &status,
                before.as_ref(),
                after.as_ref(),
                &cfg.include,
                &cfg.exclude,
            ) {
                selected.push(status);
                if limit > 0 && selected.len() >= limit {
                    break;
                }
            }
        }
        if limit > 0 && selected.len() >= limit {
            break;
        }
        page = next_url(link.as_deref());
        if let Some(next) = &page {
            let next_url = url::Url::parse(next).context("invalid Mastodon pagination URL")?;
            let instance_url =
                url::Url::parse(&cfg.instance_url).context("invalid Mastodon instance URL")?;
            if next_url.origin() != instance_url.origin()
                || !next_url
                    .path()
                    .starts_with(instance_url.path().trim_end_matches('/'))
            {
                bail!(
                    "Mastodon returned a pagination URL outside the configured instance; aborting"
                );
            }
        }
    }
    println!(
        "Plan: {} status(es); {}",
        selected.len(),
        if cfg.dry_run {
            "dry run"
        } else {
            "live deletion"
        }
    );
    for s in &selected {
        println!("{}  {}  {}", s.id, s.created_at, truncate(&status_text(s)));
    }
    if cfg.dry_run {
        return Ok(());
    }
    if selected.is_empty() {
        println!("Nothing to delete.");
        return Ok(());
    }
    eprint!("Type DELETE to delete these {} statuses: ", selected.len());
    io::stderr().flush()?;
    let mut confirmation = String::new();
    io::stdin().read_line(&mut confirmation)?;
    if confirmation.trim() != "DELETE" {
        bail!("confirmation did not match; nothing deleted");
    }
    for status in &selected {
        api.delete(&status.id)
            .await
            .with_context(|| format!("failed deleting status {}; stopping", status.id))?;
        append_history(&cfg.history_path, status, &account.acct)?;
        println!("Deleted {}", status.id);
    }
    Ok(())
}

fn append_history(path: &str, status: &Status, account: &str) -> Result<()> {
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("create history directory {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open history {}", path.display()))?;
    let record = HistoryRecord {
        platform: "mastodon",
        status_id: &status.id,
        url: &status.url,
        created_at: status.created_at.clone(),
        deleted_at: Utc::now(),
        account,
    };
    serde_json::to_writer(&mut file, &record)?;
    file.write_all(b"\n")?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn status(created_at: &str, content: &str) -> Status {
        Status {
            id: "123".into(),
            created_at: DateTime::parse_from_rfc3339(created_at)
                .unwrap()
                .with_timezone(&Utc),
            content: content.into(),
            url: None,
            reblog: None,
        }
    }

    #[test]
    fn include_terms_are_case_insensitive_and_any_match() {
        let status = status("2024-01-02T12:00:00Z", "<p>Evening walk</p>");
        assert!(matches(&status, None, None, &terms("morning,WALK"), &[]));
    }

    #[test]
    fn exclude_terms_override_include_matches() {
        let status = status("2024-01-02T12:00:00Z", "<p>Evening walk</p>");
        assert!(!matches(
            &status,
            None,
            None,
            &["walk".into()],
            &["evening".into()]
        ));
    }

    #[test]
    fn date_bounds_exclude_the_bound_itself() {
        let status = status("2024-01-02T12:00:00Z", "status");
        let before = DateTime::parse_from_rfc3339("2024-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let after = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(matches(&status, Some(&before), Some(&after), &[], &[]));
        assert!(!matches(&status, Some(&status.created_at), None, &[], &[]));
        assert!(!matches(&status, None, Some(&status.created_at), &[], &[]));
    }
}

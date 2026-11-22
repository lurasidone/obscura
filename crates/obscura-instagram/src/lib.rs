mod cleaner;
pub mod config;
mod events;
mod instagram;
pub mod tui;

pub use instagram::client::InstagramClient;
pub use instagram::error::{IgError, IgResult};
pub use instagram::ratelimit::RateLimiter;
pub use obscura_core::{clipboard, history, util};

pub(crate) use instagram::{client, device, error, model, ratelimit};

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::Parser;

use crate::config::Config;

#[derive(Parser, Debug)]
#[command(
    name = "instagram-cleaner",
    version,
    about = "Delete your own Instagram content from the terminal",
    after_help = "Options other than diagnostics can also be set in .env. See the README."
)]
struct CliArgs {
    /// What to delete: posts, dms, reactions, likes, saved, comments, archived, reposts or all;
    /// add unfollow, which all leaves out, to unfollow accounts that don't follow you back
    #[arg(long, value_name = "LIST", help_heading = "What to delete")]
    content: Option<String>,

    /// Limit dms and reactions to these thread IDs
    #[arg(long, value_name = "IDS", help_heading = "DMs and reactions")]
    threads: Option<String>,

    /// Limit the inbox and your picks to all, 1on1 or group conversations
    #[arg(long, value_name = "KIND", help_heading = "DMs and reactions")]
    dm_kind: Option<String>,

    /// Keep text messages, Reels and shared posts; unsend your other messages
    #[arg(long, help_heading = "DMs and reactions")]
    dm_media_only: bool,

    /// After a live run, hide threads left with none of your messages
    #[arg(long, help_heading = "DMs and reactions")]
    hide_threads: bool,

    /// Never touch DMs with, or unfollow, these usernames, user IDs or thread IDs (a list or a file)
    #[arg(long, value_name = "LIST|FILE", help_heading = "DMs and reactions")]
    protect: Option<String>,

    /// Only items newer than this date: YYYY-MM-DD or RFC 3339, UTC
    #[arg(long, value_name = "DATE", help_heading = "Filters")]
    after: Option<String>,

    /// Only items older than this date: YYYY-MM-DD or RFC 3339, UTC
    #[arg(long, value_name = "DATE", help_heading = "Filters")]
    before: Option<String>,

    /// Stop after planning this many items; 0 means no limit
    #[arg(long, value_name = "N", help_heading = "Filters")]
    limit: Option<String>,

    /// Force a dry run, even if INSTAGRAM_DRY_RUN is false
    #[arg(long, help_heading = "Run")]
    dry_run: bool,

    /// Minimum wait between delete requests
    #[arg(long, value_name = "MS", help_heading = "Run")]
    delete_delay_ms: Option<String>,

    /// Minimum wait between read requests
    #[arg(long, value_name = "MS", help_heading = "Run")]
    scan_delay_ms: Option<String>,

    /// Parallel read requests, 1 to 8
    #[arg(long, value_name = "N", help_heading = "Run")]
    scan_concurrency: Option<String>,

    /// The sessionid cookie; prefer .env, since flags show up in process lists
    #[arg(long, value_name = "COOKIE", help_heading = "Account")]
    sessionid: Option<String>,

    /// Log in, list the first posts and DM threads, then exit without deleting
    #[arg(long, help_heading = "Diagnostics")]
    check: bool,

    /// Print the items of one DM thread, then exit without deleting
    #[arg(long, value_name = "THREAD_ID", help_heading = "Diagnostics")]
    probe_thread: Option<String>,

    /// Print the shape of the activity feeds, then exit without deleting
    #[arg(long, help_heading = "Diagnostics")]
    probe_activity: bool,

    /// Dry run of unfollow without the app: print who would go and every warning, then exit
    #[arg(long, help_heading = "Diagnostics")]
    probe_unfollow: bool,
}

enum Mode {
    Tui,
    Check,
    ProbeThread(String),
    ProbeActivity,
    ProbeUnfollow,
}

/// Entry point of the standalone `obscura-instagram` binary.
pub fn cli_main() -> Result<()> {
    cli_main_from(std::env::args_os())
}

/// Parse the Instagram arguments (program name first) and run.
pub fn cli_main_from<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    let env_file = dotenvy::dotenv();
    init_tracing();

    let args = CliArgs::parse_from(args);
    // loading stops at a broken line, which would silently drop what comes after it,
    // such as the protect list; a missing file is fine, since settings can come from elsewhere
    if let Err(error) = env_file {
        if !error.not_found() {
            bail!("could not read .env: {}", env_file_problem(&error));
        }
    }
    config::check_replaced_vars().map_err(anyhow::Error::msg)?;
    let mut cfg = Config::from_env();
    cfg.absorb_cli(&args)?;

    let check = args.check;
    let probe_activity = args.probe_activity;
    let probe_unfollow = args.probe_unfollow;
    let mode = if check {
        Mode::Check
    } else if let Some(thread_id) = args.probe_thread {
        Mode::ProbeThread(thread_id)
    } else if probe_activity {
        Mode::ProbeActivity
    } else if probe_unfollow {
        Mode::ProbeUnfollow
    } else {
        Mode::Tui
    };

    let runtime = tokio::runtime::Runtime::new()?;
    match mode {
        Mode::Tui => runtime.block_on(tui::run(cfg)),
        Mode::Check => runtime.block_on(run_check(&cfg)),
        Mode::ProbeThread(thread_id) => runtime.block_on(run_probe_thread(&cfg, &thread_id)),
        Mode::ProbeActivity => runtime.block_on(run_probe_activity(&cfg)),
        Mode::ProbeUnfollow => runtime.block_on(run_probe_unfollow(&cfg)),
    }
}

impl Config {
    fn absorb_cli(&mut self, args: &CliArgs) -> Result<()> {
        if let Some(v) = &args.sessionid {
            self.sessionid = v.clone();
        }
        if let Some(v) = &args.content {
            self.content = config::parse_content_value(v).map_err(anyhow::Error::msg)?;
        }
        if let Some(v) = &args.threads {
            self.threads = config::thread_id_list(v).join(",");
        }
        if let Some(v) = &args.before {
            self.before = v.clone();
        }
        if let Some(v) = &args.after {
            self.after = v.clone();
        }
        if let Some(v) = &args.limit {
            self.limit = v.clone();
        }
        if let Some(v) = &args.delete_delay_ms {
            self.throttle_ms = v.clone();
        }
        if let Some(v) = &args.scan_delay_ms {
            self.scan_throttle_ms = v.clone();
        }
        if let Some(v) = &args.scan_concurrency {
            self.concurrency = v.clone();
        }
        if let Some(v) = &args.dm_kind {
            self.dm_kind = config::parse_dm_kind_value(v).map_err(anyhow::Error::msg)?;
        }
        if args.dm_media_only {
            self.dm_media_only = true;
        }
        if args.dry_run {
            self.dry_run = true;
        }
        if args.hide_threads {
            self.hide_threads = true;
        }
        let protect = args
            .protect
            .clone()
            .or_else(|| config::trimmed_env("INSTAGRAM_PROTECT"));
        if let Some(spec) = &protect {
            self.protect_users = parse_protect(spec)?;
        }
        Ok(())
    }
}

/// dotenvy's own message quotes the broken line, which can be the session cookie.
fn env_file_problem(error: &dotenvy::Error) -> String {
    let dotenvy::Error::LineParse(line, _) = error else {
        return error.to_string();
    };
    match line.split_once('=').map(|(key, _)| key.trim()) {
        Some(key) if !key.is_empty() && !key.contains(char::is_whitespace) => {
            format!("the line that sets {key} can't be parsed")
        }
        _ => "a line can't be parsed; each one should look like NAME=value".to_string(),
    }
}

fn init_tracing() {
    let Ok(spec) = std::env::var("RUST_LOG") else {
        return;
    };
    if spec.trim().is_empty() {
        return;
    }
    let filter = tracing_subscriber::EnvFilter::try_new(&spec)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

/// Accepts a comma/whitespace separated list, a path, or `@path`.
fn parse_protect(spec: &str) -> Result<Vec<String>> {
    let spec = spec.trim();
    if spec.is_empty() {
        bail!("--protect is empty");
    }

    let raw = match spec.strip_prefix('@') {
        Some(path) if Path::new(path).is_file() => std::fs::read_to_string(path)
            .with_context(|| format!("--protect: cannot read file {path:?}"))?,
        _ if Path::new(spec).is_file() => std::fs::read_to_string(spec)
            .with_context(|| format!("--protect: cannot read {spec:?}"))?,
        _ => spec.to_string(),
    };

    let mut ids: Vec<String> = Vec::new();
    for id in config::thread_id_list(&raw) {
        let id = id.trim_start_matches('@');
        if !id.is_empty() && !ids.iter().any(|kept| kept == id) {
            ids.push(id.to_string());
        }
    }
    if ids.is_empty() {
        bail!("--protect: no usernames or IDs found");
    }
    Ok(ids)
}

/// Read-only helpers share the conservative 350ms scan pacing.
async fn probe_client(cfg: &Config) -> Result<InstagramClient> {
    let cancel = Arc::new(AtomicBool::new(false));
    let limiter = Arc::new(ratelimit::RateLimiter::new(
        std::time::Duration::from_millis(350),
        cancel,
    ));
    InstagramClient::new(
        &cfg.sessionid,
        Some(&cfg.base_url),
        Some(&cfg.session_path),
        limiter,
    )
    .await
    .context("build instagram client")
}

fn require_session(cfg: &Config) -> Result<()> {
    if cfg.sessionid.trim().is_empty() {
        bail!("no session: set INSTAGRAM_SESSIONID in .env or pass --sessionid");
    }
    Ok(())
}

async fn run_check(cfg: &Config) -> Result<()> {
    require_session(cfg)?;
    println!("[check] base url: {}", cfg.base_url);
    println!("[check] session file: {}", cfg.session_path);

    let client = probe_client(cfg).await?;

    println!("[check] logging in...");
    let user_id = client.user_id();
    let username = client.username();
    let full_name = client
        .fetch_user_info(&user_id)
        .await
        .map(|u| u.full_name)
        .unwrap_or_default();
    println!(
        "[check] logged in as @{} (pk={}, name={:?})",
        username, user_id, full_name
    );

    println!("[check] fetching your first posts...");
    match client.user_medias_page(&user_id, None, 12).await {
        Ok((items, next)) => {
            println!(
                "[check] found {} posts (more_available={})",
                items.len(),
                next.is_some()
            );
            for (i, post) in items.iter().take(5).enumerate() {
                let caption = util::truncate(&post.caption_text(), 50);
                println!("  post [{i}]: id={} caption={caption:?}", post.media_id());
            }
        }
        Err(e) => println!("[check] warning: could not fetch posts: {e}"),
    }

    println!("[check] fetching your first DM threads...");
    match client.direct_inbox_page(None, 20).await {
        Ok((threads, _)) => {
            println!("[check] found {} threads", threads.len());
            for (i, thread) in threads.iter().take(5).enumerate() {
                let title = thread.display_name();
                let last_msg = thread
                    .items
                    .first()
                    .and_then(|it| it.text.as_ref())
                    .map(|s| util::truncate(s, 40))
                    .unwrap_or_else(|| "(non-text item)".to_string());
                let protected = if cfg.protect_users.iter().any(|p| {
                    p == &thread.thread_id
                        || thread
                            .users
                            .iter()
                            .any(|u| &u.pk == p || u.username.eq_ignore_ascii_case(p))
                }) {
                    " [PROTECTED]"
                } else {
                    ""
                };
                println!(
                    "  thread [{i}]: id={} title={:?} last_msg={:?}{protected}",
                    thread.thread_id, title, last_msg
                );
            }
        }
        Err(e) => println!("[check] warning: could not fetch the inbox: {e}"),
    }

    if !cfg.protect_users.is_empty() {
        println!(
            "[check] protecting {} usernames or IDs",
            cfg.protect_users.len()
        );
    }
    println!("[check] done (nothing deleted)");
    Ok(())
}

async fn run_probe_thread(cfg: &Config, thread_id: &str) -> Result<()> {
    require_session(cfg)?;
    println!("[probe-thread] reading thread: {thread_id}");
    let client = probe_client(cfg).await?;

    let thread = client
        .direct_thread(thread_id, 20, None)
        .await
        .context("direct_thread")?;
    println!("[probe-thread] thread_id: {}", thread.thread_id);
    println!("[probe-thread] title: {}", thread.display_name());
    println!("[probe-thread] is_group: {}", thread.is_group);
    println!(
        "[probe-thread] users: {}",
        thread
            .users
            .iter()
            .map(|u| format!("@{} (pk={})", u.username, u.pk))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "[probe-thread] retrieved {} items in page",
        thread.items.len()
    );
    for (i, item) in thread.items.iter().take(10).enumerate() {
        let txt = item.text.as_deref().unwrap_or("(non-text)");
        println!(
            "  item [{i}]: id={} user={:?} type={:?} text={:?}",
            item.item_id, item.user_id, item.item_type, txt
        );
    }
    Ok(())
}

async fn run_probe_unfollow(cfg: &Config) -> Result<()> {
    use crate::events::CleanerEvent;

    require_session(cfg)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let limiter = Arc::new(ratelimit::RateLimiter::new(
        std::time::Duration::from_millis(350),
        cancel.clone(),
    ));
    let client = InstagramClient::new(
        &cfg.sessionid,
        Some(&cfg.base_url),
        Some(&cfg.session_path),
        limiter.clone(),
    )
    .await
    .context("build instagram client")?;
    println!("[probe-unfollow] dry run as @{}", client.username());

    let config = cleaner::CleanerConfig {
        targets: Vec::new(),
        content: cleaner::ContentFlags {
            unfollow: true,
            ..Default::default()
        },
        before: None,
        after: None,
        limit: cfg.limit.trim().parse().context("invalid limit")?,
        dry_run: true,
        throttle_ms: 200,
        scan_throttle_ms: 350,
        concurrency: 2,
        hide_threads: false,
        protect_users: cfg.protect_users.clone(),
        dm_kind: config::DmKind::All,
        dm_media_only: false,
        unsend_dms: false,
        history_dir: cfg.history_dir.clone(),
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let run = tokio::spawn(cleaner::run(client, config, limiter, tx, cancel));
    while let Some(event) = rx.recv().await {
        match event {
            CleanerEvent::Warning { message } | CleanerEvent::Log { message, .. } => {
                println!("{message}");
            }
            CleanerEvent::MessageDeleted { preview, outcome } => {
                println!("{}: {preview}", outcome.log_prefix());
            }
            CleanerEvent::Error { message } => println!("[probe-unfollow] error: {message}"),
            _ => {}
        }
    }
    run.await.context("unfollow probe stopped")?;
    println!("[probe-unfollow] done, nothing was changed; saved in history/dry-run");
    Ok(())
}

async fn run_probe_activity(cfg: &Config) -> Result<()> {
    require_session(cfg)?;
    let client = probe_client(cfg).await?;
    println!(
        "[probe-activity] authenticated as @{} (pk={})",
        client.username(),
        client.user_id()
    );

    let types = r#"["ALL_MEDIA_AUTO_COLLECTION","PRODUCT_AUTO_COLLECTION","MEDIA"]"#;
    match client
        .raw_get_json("collections/list/", &[("collection_types", types)])
        .await
    {
        Ok(v) => {
            println!("\n[probe-activity] collections/list/");
            match v.get("items").and_then(|i| i.as_array()) {
                Some(items) => {
                    for item in items {
                        println!(
                            "  id={:?} name={:?} type={:?} count={:?}",
                            item.get("collection_id"),
                            item.get("collection_name"),
                            item.get("collection_type"),
                            item.get("collection_media_count"),
                        );
                    }
                }
                None => println!("  (no items array) {}", util::truncate(&v.to_string(), 400)),
            }
        }
        Err(e) => println!("\n[probe-activity] collections/list/ failed: {e}"),
    }

    for (label, path) in [
        ("liked", "feed/liked/"),
        ("saved", "feed/saved/posts/"),
        ("archived", "feed/only_me_feed/"),
    ] {
        match client
            .raw_get_json(path, &[("include_igtv_preview", "false")])
            .await
        {
            Ok(v) => {
                let items = v.get("items").and_then(|i| i.as_array());
                let n = items.map_or(0, |a| a.len());
                println!("\n[probe-activity] {path} → {n} item(s)");
                if let Some(first) = items.and_then(|a| a.first()) {
                    println!(
                        "  first item wrapped in `media` key: {}",
                        first.get("media").is_some()
                    );
                    println!(
                        "  keys: {:?}",
                        first.as_object().map(|o| o.keys().collect::<Vec<_>>())
                    );
                }
                println!(
                    "  cursors: next_max_id={:?} max_id={:?} more_available={:?}",
                    v.get("next_max_id"),
                    v.get("max_id"),
                    v.get("more_available")
                );
            }
            Err(e) => println!("\n[probe-activity] {label} ({path}) failed: {e}"),
        }
    }

    let own = client
        .user_medias_page(&client.user_id(), None, 3)
        .await
        .map(|(items, _)| items)
        .unwrap_or_default();
    let probe_media = match own.first() {
        Some(media) => Some((media.media_id(), "own newest post")),
        None => match client.saved_medias_page(None).await {
            Ok((items, _)) => items.first().map(|m| (m.media_id(), "first saved post")),
            Err(_) => None,
        },
    };

    match probe_media {
        Some((media_id, source)) => {
            println!("\n[probe-activity] media/{media_id}/comments/ ({source})");
            match client.media_comments_page(&media_id, None).await {
                Ok((comments, next)) => {
                    println!(
                        "  parsed OK: {} comment(s), next={next:?}, {} authored by me",
                        comments.len(),
                        comments
                            .iter()
                            .filter(|c| c.author_id() == client.user_id())
                            .count()
                    );
                    for c in comments.iter().take(3) {
                        println!(
                            "    pk={} by @{} at={} text={:?}",
                            c.pk,
                            c.author_username(),
                            c.created_at,
                            util::truncate(&c.text, 40)
                        );
                    }
                }
                Err(e) => println!("  failed: {e}"),
            }
        }
        None => println!("\n[probe-activity] no media available to probe comments on"),
    }

    println!("\n[probe-activity] done (nothing deleted)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::env_file_problem;

    fn problem(line: &str) -> String {
        env_file_problem(&dotenvy::Error::LineParse(line.to_string(), 0))
    }

    #[test]
    fn a_broken_env_line_never_shows_its_value() {
        let shown = problem("INSTAGRAM_SESSIONID=123%3Asecret\"oops");
        assert!(shown.contains("INSTAGRAM_SESSIONID"), "{shown}");
        assert!(!shown.contains("secret"), "{shown}");

        let shown = problem("123%3Asecret");
        assert!(!shown.contains("secret"), "{shown}");
    }
}

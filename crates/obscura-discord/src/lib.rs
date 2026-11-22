pub use obscura_core::{clipboard, history, util};

pub mod config;
mod discord;
mod events;
mod pruner;
pub mod tui;

pub(crate) use discord::headers as device;
pub(crate) use discord::{client, error, model, ratelimit};

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use tokio::sync::mpsc;

use crate::client::DiscordClient;
use crate::config::{Config, ScopeMode};
use crate::events::{LogLevel, PrunerEvent};
use crate::ratelimit::RateLimiter;

#[derive(Parser, Debug)]
#[command(
    name = "discord-pruner",
    version,
    about = "Prune your own Discord messages from selected DMs and channels"
)]
struct CliArgs {
    /// User token (or set DISCORD_TOKEN)
    #[arg(long)]
    token: Option<String>,
    /// all, dms, guilds, none, or channels:<id>,<id>
    #[arg(long)]
    scope: Option<String>,
    #[arg(long)]
    before: Option<String>,
    #[arg(long)]
    after: Option<String>,
    #[arg(long)]
    limit: Option<String>,
    #[arg(long)]
    throttle_ms: Option<String>,
    #[arg(long)]
    scan_throttle_ms: Option<String>,
    /// all, only-attachments, has-attachments, has-links, text-only
    #[arg(long)]
    content_filter: Option<String>,
    #[arg(long)]
    keep_pinned: bool,
    #[arg(long)]
    dry_run: bool,
    /// Connect and print account plus selected scan scope; delete nothing
    #[arg(long)]
    smoke: bool,
}

/// Entry point of the standalone `obscura-discord` binary.
pub fn cli_main() -> Result<()> {
    cli_main_from(std::env::args_os())
}

/// Parse the Discord arguments (program name first) and run.
pub fn cli_main_from<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    let has_cli_args = args.len() > 1;

    let _ = dotenvy::dotenv();
    let args = CliArgs::parse_from(args);
    let mut cfg = Config::from_env();
    if let Some(token) = args.token {
        cfg.token = token;
    }
    if let Some(scope) = args.scope {
        cfg.scope = config::parse_scope(&scope).map_err(anyhow::Error::msg)?;
    }
    if let Some(v) = args.before {
        cfg.before = v;
    }
    if let Some(v) = args.after {
        cfg.after = v;
    }
    if let Some(v) = args.limit {
        cfg.limit = v;
    }
    if let Some(v) = args.throttle_ms {
        cfg.throttle_ms = v;
    }
    if let Some(v) = args.scan_throttle_ms {
        cfg.scan_throttle_ms = v;
    }
    if let Some(v) = args.content_filter {
        cfg.content_filter = v.parse::<config::ContentFilter>().unwrap_or_else(|_| {
            eprintln!("[discord-pruner] --content-filter={v:?}: unrecognised; using 'all'");
            config::ContentFilter::All
        });
    }
    if args.keep_pinned {
        cfg.keep_pinned = true;
    }
    if args.dry_run {
        cfg.dry_run = true;
    }
    let runtime = tokio::runtime::Runtime::new()?;
    if !has_cli_args {
        runtime.block_on(tui::run(cfg))
    } else {
        runtime.block_on(run(cfg, args.smoke))
    }
}

async fn run(cfg: Config, smoke: bool) -> Result<()> {
    if cfg.token.trim().is_empty() {
        bail!("DISCORD_TOKEN is empty; set it in .env or pass --token");
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let limiter = Arc::new(RateLimiter::new(
        Duration::from_millis(2000),
        cancel.clone(),
    ));
    let client = DiscordClient::new(
        &cfg.token,
        Some(&cfg.base_url),
        Some(&cfg.session_path),
        limiter.clone(),
    )
    .await
    .context("connect to Discord")?;
    println!("Connected as {} ({})", client.username(), client.user_id());
    println!(
        "Scope: {:?}; mode: {}",
        cfg.scope,
        if smoke || cfg.dry_run {
            "dry run"
        } else {
            "live delete"
        }
    );
    if smoke {
        return Ok(());
    }
    let targets = match &cfg.scope {
        ScopeMode::None => Vec::new(),
        ScopeMode::Dms => vec![pruner::TargetResolution::AllDms],
        ScopeMode::Guilds => vec![pruner::TargetResolution::AllGuilds],
        ScopeMode::All => vec![
            pruner::TargetResolution::AllDms,
            pruner::TargetResolution::AllGuilds,
        ],
        ScopeMode::Channels(raw) => vec![pruner::TargetResolution::Channels(
            config::channel_id_list(raw),
        )],
    };
    let parse_num = |s: &str, fallback: u64| s.parse::<u64>().unwrap_or(fallback);
    let (tx, mut rx) = mpsc::channel(256);
    let cfg_run = pruner::PrunerConfig {
        targets,
        before: (!cfg.before.is_empty()).then_some(cfg.before),
        after: (!cfg.after.is_empty()).then_some(cfg.after),
        limit: parse_num(&cfg.limit, 0) as usize,
        dry_run: cfg.dry_run,
        throttle_ms: parse_num(&cfg.throttle_ms, 2000),
        scan_throttle_ms: parse_num(&cfg.scan_throttle_ms, 350),
        concurrency: parse_num(&cfg.concurrency, 4) as u32,
        close_dms: cfg.close_dms,
        protect_ids: cfg.protect_ids,
        dm_kind: cfg.dm_kind,
        content_filter: cfg.content_filter,
        keep_pinned: cfg.keep_pinned,
    };
    let task = tokio::spawn(pruner::run(client, cfg_run, limiter, tx, cancel));
    while let Some(event) = rx.recv().await {
        match event {
            PrunerEvent::Log { level, message } => match level {
                LogLevel::Warning | LogLevel::Error => eprintln!("{message}"),
                _ => println!("{message}"),
            },
            PrunerEvent::PlanReady { total } => println!("Plan ready: {total} message(s)"),
            PrunerEvent::MessageDeleted { preview, outcome } => {
                println!("{} {preview}", outcome.log_prefix())
            }
            PrunerEvent::MessageFailed { message_id, error } => {
                eprintln!("Failed {message_id}: {error}")
            }
            PrunerEvent::Error { message } => eprintln!("Error: {message}"),
            PrunerEvent::Done { stats } => println!(
                "Finished: scanned {}, deleted {}, skipped {}, failed {}",
                stats.messages_scanned,
                stats.messages_deleted,
                stats.messages_skipped,
                stats.messages_failed
            ),
            _ => {}
        }
    }
    let _ = task.await?;
    Ok(())
}

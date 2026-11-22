//! obscura: one terminal for archiving and deleting your own content.
//!
//! With no arguments it opens the platform picker, then hands the terminal to
//! the chosen platform's full TUI. Subcommands forward to the standalone
//! binaries, so every existing flag keeps working.

mod selector;

use std::ffi::OsString;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();

    match args.get(1).and_then(|arg| arg.to_str()) {
        None => run_picker(),
        Some("discord") => forward(
            obscura_discord::cli_main_from,
            &OsString::from("discord-pruner"),
            &args[1..],
        ),
        Some("instagram") => forward(
            obscura_instagram::cli_main_from,
            &OsString::from("instagram-cleaner"),
            &args[1..],
        ),
        Some("mastodon") => forward(
            obscura_mastodon::cli_main_from,
            &OsString::from("obscura-mastodon"),
            &args[1..],
        ),
        Some("-h" | "--help" | "help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            println!("obscura {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("obscura: unknown command {other:?}\n");
            print_help();
            ExitCode::from(2)
        }
    }
}

fn run_picker() -> ExitCode {
    let _ = dotenvy::dotenv();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("obscura: could not start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(selector::run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("obscura: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn forward<F>(run: F, program: &OsString, args: &[OsString]) -> ExitCode
where
    F: FnOnce(Vec<OsString>) -> anyhow::Result<()>,
{
    let mut forwarded = Vec::with_capacity(args.len() + 1);
    forwarded.push(program.clone());
    forwarded.extend(args.iter().skip(1).cloned());
    match run(forwarded) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("obscura: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "\
obscura {version}
Mass archiver and deleter for your own content.

Usage:
  obscura                 Open the platform picker
  obscura discord ...     Run the Discord pruner (all its flags apply)
  obscura instagram ...   Run the Instagram cleaner (all its flags apply)
  obscura mastodon ...    Run the Mastodon status cleaner (all its flags apply)
  obscura --help          Show this help
  obscura --version       Show the version

Configuration:
  Copy .env.example to .env at the repository root and fill in platform settings.
  Discord reads DISCORD_*; Instagram reads INSTAGRAM_*; Mastodon reads MASTODON_*.

  Each platform keeps its own dry-run defaults and safety checks. Read
  crates/*/README.md before turning dry run off.",
        version = env!("CARGO_PKG_VERSION")
    );
}

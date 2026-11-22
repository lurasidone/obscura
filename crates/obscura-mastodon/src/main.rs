use std::process::ExitCode;

fn main() -> ExitCode {
    match obscura_mastodon::cli_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("obscura-mastodon: {error:#}");
            ExitCode::FAILURE
        }
    }
}

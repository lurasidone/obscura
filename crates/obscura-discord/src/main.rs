fn main() {
    if let Err(error) = obscura_discord::cli_main() {
        eprintln!("obscura-discord: {error:#}");
        std::process::exit(1);
    }
}

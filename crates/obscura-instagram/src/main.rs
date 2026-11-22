fn main() {
    if let Err(error) = obscura_instagram::cli_main() {
        eprintln!("obscura-instagram: {error:#}");
        std::process::exit(1);
    }
}

# obscura

`obscura` is a terminal app for reviewing and removing your own content on
Discord and Instagram. It plans each run before making changes and keeps a local
JSONL record of the plan and its results.

| Platform | Supported items |
| --- | --- |
| Discord | Your messages in DMs, group DMs, and guild channels |
| Instagram | Posts, DM messages and reactions, likes, saved posts, comments, archived posts, and reposts; optional unfollowing |

Automating personal accounts may violate platform terms and can lead to
account restrictions. Deletions are permanent. Review a dry run first; the
default delays are intentional.

## Run

Install Rust 1.98 or newer, then:

```sh
cp .env.example .env
chmod 600 .env
# Set the credentials for the platform you plan to use.
cargo run --release
```

The app opens a platform picker and then the platform's interface. Its screens
are **Settings**, **Targets**, **Run**, and **Archive**. The archive and session
files are kept separately for each platform.

To start a platform directly or run a one-off command:

```sh
cargo run --release -- discord --scope dms --before 2024-01-01 --dry-run
cargo run --release -- instagram --content posts,likes --dry-run
cargo run --release -- instagram --check
```

See the platform guides for options and limitations:

- [Discord](crates/obscura-discord/README.md)
- [Instagram](crates/obscura-instagram/README.md)

## Local data

```text
data/
  discord/session.json
  discord/history/
  instagram/session.json
  instagram/history/
```

The session files contain credentials and client state. Keep them private.
`data/`, `.env`, and `session.json` files are ignored by Git; do not force-add
them. Session files are created with `0600` permissions. Set
`DISCORD_SESSION_PATH`, `PRUNER_HISTORY_DIR`, `INSTAGRAM_SESSION_PATH`, or
`INSTAGRAM_HISTORY_DIR` to use different locations.

## Workspace

```text
crates/
  obscura/              unified command and platform picker
  obscura-core/         shared archive, rate limiter, date parsing, and TUI
  obscura-discord/      Discord client, message pruner, and TUI
  obscura-instagram/    Instagram client, content cleaner, and TUI
```

The platform crates own their API clients and deletion behavior. Shared archive
and terminal components live in `obscura-core`.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## License

The workspace is licensed under the [MIT license](LICENSE).

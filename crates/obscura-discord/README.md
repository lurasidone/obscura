# obscura-discord

Discord support for [obscura](../../README.md). Run it from the workspace root
with `cargo run -p obscura-discord`, or use the unified app with
`cargo run -- discord`.

This tool scans your own messages in DMs, group DMs, and guild channels, then
can delete the messages selected by your filters. It uses Discord's private
user-account API, including raw user-token authentication. Automating a user
account may violate Discord's Terms of Service and can result in account
restrictions. Use at your own risk.

## Start

Copy `.env.example` to `.env` and set `DISCORD_TOKEN`. Keep the file private:
the token grants access to your account.

```sh
cargo run -p obscura-discord
cargo run -p obscura-discord -- --smoke
```

With no arguments, the app opens the terminal interface. Runs start in dry-run
mode. Review the plan in the Run or Archive screens before disabling dry run.

The command-line interface accepts the same settings:

```sh
cargo run -p obscura-discord -- --scope dms --before 2024-01-01 --limit 100 --dry-run
cargo run -p obscura-discord -- --scope channels:123456789012345678,987654321098765432 --dry-run
```

## Scanning and filters

`--scope` accepts `all`, `dms`, `guilds`, `none`, or
`channels:<id>,<id>`. Guild scans use Discord's guild-wide search and also walk
resolved channel histories, including accessible active and archived threads.
Duplicate message IDs are collapsed. Scans continue until exhausted or
cancelled; `DISCORD_LIMIT` is the only configured cap on planned messages.
Only messages authored by the authenticated account are eligible. System
messages are skipped.

`--before` and `--after` accept `YYYY-MM-DD` or RFC 3339 timestamps. Date bounds
are exclusive. `DISCORD_DM_KIND` selects all DMs, one-to-one DMs, or group DMs.
`DISCORD_CONTENT_FILTER` accepts `all`, `only-attachments`,
`has-attachments`, `has-links`, or `text-only`. Pinned messages are included
unless `DISCORD_KEEP_PINNED=true`.

Deleted-message records are written to `data/discord/history/`.

## Settings

| Variable | Default | Purpose |
| --- | --- | --- |
| `DISCORD_TOKEN` | — | Discord user token |
| `DISCORD_BASE_URL` | `https://discord.com/api/v9` | REST API base URL |
| `DISCORD_SESSION_PATH` | `data/discord/session.json` | Session and client identity |
| `DISCORD_SCOPE` | `all` | `all`, `dms`, `guilds`, `none`, or `channels:<id>,<id>` |
| `DISCORD_DM_KIND` | `all` | `all`, `1on1`, or `group` |
| `DISCORD_CONTENT_FILTER` | `all` | Message content filter |
| `DISCORD_KEEP_PINNED` | `false` | Keep pinned messages when `true` |
| `DISCORD_BEFORE` / `DISCORD_AFTER` | — | Exclusive date bounds |
| `DISCORD_LIMIT` | `0` | Planned-message limit; `0` means unlimited |
| `DISCORD_THROTTLE_MS` | `2000` | Delay between delete requests |
| `DISCORD_SCAN_THROTTLE_MS` | `350` | Delay between scan requests |
| `DISCORD_SCAN_CONCURRENCY` | `4` | Concurrent channel scans, clamped to 1–8 |
| `DISCORD_DRY_RUN` | `true` | Record a plan without deleting |
| `DISCORD_CLOSE_DMS` | `true` when unset | Close one-to-one DMs after a completed run; the example `.env` sets it to `false` |
| `PRUNER_HISTORY_DIR` | `data/discord/history` | Archive directory |

Session and archive files are private account data. They are kept under
`data/discord/` by default; see the root [data and privacy notes](../../README.md#local-data).

# obscura-instagram

Instagram support for [obscura](../../README.md). Run it from the workspace
root with `cargo run -p obscura-instagram`, or select Instagram in the unified
app with `cargo run -- instagram`.

The cleaner uses Instagram's private mobile API and your existing browser
session; it does not use browser automation. Automated requests can trigger
account checks, and the API may change without notice. Deletions are permanent.
Start with a dry run and keep the default delays.

## Start

You need Rust 1.98 or newer, a C/C++ toolchain, and `cmake`.

1. Copy the example settings file and restrict its permissions:

   ```sh
   cp .env.example .env
   chmod 600 .env
   ```

2. Put the `sessionid` cookie from a logged-in browser in `INSTAGRAM_SESSIONID`.
   Password login is not supported.
3. Start the app:

   ```sh
   cargo run --release
   ```

4. Select content in **Settings**. Nothing is selected by default.
5. Press `F5` to create a dry-run plan. Check it in **Run** and **Archive**.
6. Only after reviewing the plan, press `d` to disable dry run and `F5` again.
   The app shows the run scope and asks for confirmation.

Press `?` in the app to see the keyboard shortcuts.

## Content types

Use the **Settings** screen or pass a comma-separated list to `--content`:

| Value | Action |
| --- | --- |
| `posts` | Delete your posts |
| `dms` | Unsend your messages in DMs; other people's messages stay |
| `reactions` | Remove your emoji reactions in DMs; messages stay unless `dms` is also selected |
| `likes` | Unlike posts in your liked feed |
| `saved` | Remove posts from your saved items |
| `comments` | Delete your comments on your own posts |
| `archived` | Delete archived posts |
| `reposts` | Remove a native repost by its media-note ID |
| `all` | Select the content above, but not `unfollow` |
| `unfollow` | Unfollow accounts that do not follow you back |

Examples:

```sh
cargo run --release -- --content posts,likes --before 2024-01-01 --dry-run
cargo run --release -- --content reactions --threads 340282366841710300949128100000000000000
```

## Narrowing a run

### Dates and limits

`--after` and `--before` accept `YYYY-MM-DD` or RFC 3339 timestamps. They are
interpreted as UTC and exclude the specified bound. Reactions use the reaction
timestamp when Instagram provides one. `--limit` stops after planning that many
items.

### DMs and reactions

By default, `dms` and `reactions` scan the inbox, up to 2,000 threads. Narrow
the run with:

- `--threads`: a comma-separated list of thread IDs. Explicit IDs take
  precedence over picks in the Targets screen.
- Targets screen picks: used when no thread IDs are supplied.
- `--dm-kind`: `all`, `1on1`, or `group`. This filters the inbox and screen
  picks, but not explicitly supplied thread IDs.
- `--protect`: usernames, user IDs, or thread IDs to leave untouched. It also
  accepts a path to a file containing protected values.

`--dm-media-only` applies to messages in `dms`. It unsends photos, videos,
carousels, voice notes, GIFs, view-once media, story shares, profile shares,
and locations. It keeps text, Reels, shared posts, links, hearts, placeholders,
untyped messages, and unrecognized shares.

`--hide-threads` hides a thread after a live run only when at least one of your
messages was unsent there, none of yours failed or was refused, and a follow-up
read finds none of yours remaining. Each thread is checked independently.
Removing a reaction does not hide a thread. Media-only mode, date filters, and
an item limit disable this step because they can leave messages behind.

### Unfollowing

```sh
cargo run --release -- --content unfollow --protect friend1,friend2 --dry-run
cargo run --release -- --content unfollow --protect friend1,friend2 --limit 20
```

The app compares your followers with the accounts you follow, then checks each
account not found in the follower list. It keeps an account when the result is
uncertain:

- Accounts confirmed to follow you back are kept.
- If Instagram will not return an account's status, it is kept and shown as a
  warning.
- If either list cannot be read in full, the run does not unfollow anyone.
- Protected accounts and pending requests to private accounts are left alone.
- Accounts that do not follow back are eligible unless protected.
- Date filters disable unfollowing because follows have no date in this data.
  Other selected content can still run.

Use small batches with `--limit`. Instagram applies stricter limits to follow
actions than to deletions. If it refuses an unfollow, the run stops that part
without retrying. Warnings and totals appear in the archive under **Not
following back**.

## Run results

A live run re-scans up to three times for remaining items. Threads where the
first scan found no messages of yours are not scanned again. Setting `--limit`
disables these re-scans. Each item is counted once, even if retried.

| Counter | Meaning |
| --- | --- |
| **Scanned** | Items checked in the first scan; re-scans are excluded |
| **Found** | Items matching the filters |
| **Deleted** / **Would delete** | Deleted, or already gone; dry runs show the latter |
| **Skipped** | Refused by Instagram |
| **Failed** | Still failing after retries |
| **Not attempted** | Found, but the run stopped before trying them |
| **Warnings** | A check or part of a scan failed |
| **Rate limited** | Every rate limit encountered, including those handled automatically |

## Settings

Settings come from `.env` and the process environment. Command-line flags
override them. In the app, settings can also be changed except for paths and the
API endpoint.

| Flag | Variable | Default | Purpose |
| --- | --- | --- | --- |
| `--content LIST` | `INSTAGRAM_CONTENT` | empty | Content types to process |
| `--threads IDS` | `INSTAGRAM_THREADS` | whole inbox | Threads for DMs and reactions |
| `--dm-kind KIND` | `INSTAGRAM_DM_KIND` | `all` | `all`, `1on1`, or `group` |
| `--dm-media-only` | `INSTAGRAM_DM_MEDIA_ONLY` | `false` | Keep text, Reels, and shared posts |
| `--hide-threads` | `INSTAGRAM_HIDE_THREADS` | `false` | Hide eligible threads after a live run |
| `--protect LIST\|FILE` | `INSTAGRAM_PROTECT` | empty | Usernames, user IDs, or thread IDs to protect |
| `--after DATE` | `INSTAGRAM_AFTER` | empty | Exclude items at or before this bound |
| `--before DATE` | `INSTAGRAM_BEFORE` | empty | Exclude items at or after this bound |
| `--limit N` | `INSTAGRAM_LIMIT` | `0` | Planned-item limit; `0` means unlimited |
| `--dry-run` | `INSTAGRAM_DRY_RUN` | `true` | Record the plan without deleting |
| `--delete-delay-ms MS` | `INSTAGRAM_DELETE_DELAY_MS` | `2000` | Minimum delay between delete requests; at least 200 |
| `--scan-delay-ms MS` | `INSTAGRAM_SCAN_DELAY_MS` | `350` | Minimum delay between read requests; at least 100 |
| `--scan-concurrency N` | `INSTAGRAM_SCAN_CONCURRENCY` | `4` | Parallel reads, 1–8; divides the scan delay |
| `--sessionid COOKIE` | `INSTAGRAM_SESSIONID` | empty | Browser cookie; `.env` is safer than a command-line flag |
| — | `INSTAGRAM_SESSION_PATH` | `data/instagram/session.json` | Device identity and session tokens |
| — | `INSTAGRAM_HISTORY_DIR` | `./data/instagram/history` | Run archive |
| — | `INSTAGRAM_BASE_URL` | `https://i.instagram.com/api/v1` | API base URL |

Deletes run one at a time. `RUST_LOG` enables tracing. The HTTP client honors
proxy variables such as `HTTPS_PROXY`. An unreadable `.env` setting stops
startup rather than silently changing the run.

Older settings are rejected when detected: `--scope` and
`--dm-reactions-only` were replaced by `--content`, thread IDs moved to
`--threads`, and several environment variables were renamed. Update `.env`
using the error message before running again.

## Diagnostics

These commands only inspect data; they do not delete:

```sh
cargo run --release -- --check
cargo run --release -- --probe-thread <THREAD_ID>
cargo run --release -- --probe-activity
cargo run --release -- --probe-unfollow
```

`--check` lists the first posts and DM threads. The probe commands print the
response shape for one thread, activity feeds, or an unfollow dry run. The
unfollow probe also writes its results to the dry-run archive.

## Data and limitations

`data/instagram/session.json` contains private session data. The
`data/instagram/history/` directory stores a JSONL record of plans and results,
split by day and target. Both paths and `.env` are git-ignored. The session
file is created with `0600` permissions. The archive is an audit log, not a
backup; it cannot restore deleted content.

Some operations have not been verified against a disposable account:

- **Comments:** uses a private bulk-delete endpoint. Leave comments unselected
  unless you can verify the behavior safely.
- **Unfollow:** based on instagrapi endpoints and tested against a fake server,
  not a real account. Start with a dry run and a small limit.
- **Reposts:** removal uses the media-note ID. Instagram does not reliably
  expose IDs for discovering existing reposts, so scans report this limitation
  rather than treating saved items as reposts.
- **TLS profile:** this project uses the Android-compatible OkHttp 4.12 profile;
  the OkHttp 5 profile in `wreq-util` identifies as an unrelated app in this
  alpha build.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## License

[MIT](LICENSE)

# obscura-mastodon

A standalone CLI for reviewing and deleting statuses authored by the authenticated Mastodon account, including replies and boost statuses returned by that account's statuses endpoint. It uses the public Mastodon REST API; it does not use browser sessions or ask for passwords.

## Setup

Create an OAuth access token from the settings/developer interface of your own Mastodon instance or an OAuth application you control. Grant `read:accounts`, `read:statuses`, and `write:statuses`. Keep the token private: `write:statuses` permits deletion of your statuses. Do not put it in source control.

Set these in the repository-root `.env` (or export them):

```dotenv
MASTODON_INSTANCE_URL=https://mastodon.social
MASTODON_TOKEN=
MASTODON_BEFORE=
MASTODON_AFTER=
MASTODON_INCLUDE=
MASTODON_EXCLUDE=
MASTODON_LIMIT=0
MASTODON_DRY_RUN=true
MASTODON_HISTORY_PATH=data/mastodon/history.jsonl
```

`MASTODON_INSTANCE_URL` is your instance origin, e.g. `https://mastodon.social`; self-hosted instances are supported. Do not include credentials, query, or fragment. Dates accept `YYYY-MM-DD` or RFC 3339. Date-bound days are excluded, consistent with the shared date helper. Include terms are comma-separated and ORed; when present a status must contain at least one. Exclude terms are comma-separated and any match skips the status. Matching is case-insensitive against rendered status text (HTML tags removed). Limit `0` means no limit. The planner follows Mastodon's returned pagination links and preserves the instance's newest-first order.

## Commands

```sh
cargo run -p obscura-mastodon -- --help
cargo run -p obscura-mastodon -- --before 2024-01-01 --include "old,unused" --limit 100
```

The default is dry run. Deletion requires both `--live` (or `MASTODON_DRY_RUN=false`) and typing `DELETE` at the prompt. `--dry-run` forces a plan only. A partial failure stops further deletions; successful deletions are appended as JSONL records to the configured local history file. Protect `.env` and history files as private data.

Through the unified binary, use `obscura mastodon ...`; the platform picker also launches its Mastodon runner.

## Risks and limitations

Deletion is irreversible. Review the full dry-run output and narrow the date/filter/limit before enabling live mode. Instance policies, API limits, account visibility, or a revoked/under-scoped token can prevent retrieval or deletion; API errors abort the run. The API only returns statuses the account endpoint exposes; this tool cannot promise access to statuses unavailable from that endpoint. Boosts are deleted as the authenticated account's wrapper status ID, not by targeting the boosted author's original post. Content matching strips HTML tags but does not decode every HTML entity. There is no TUI yet; the picker runs the same CLI workflow with default settings, so configure filters in `.env` before launching it.

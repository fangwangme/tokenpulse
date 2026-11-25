# Usage Module

The usage module turns local agent history into the SQLite ledger every surface
reads. Binding behavior is in [`../specs/usage-sources.md`](../specs/usage-sources.md),
[`../specs/model-ids.md`](../specs/model-ids.md) and
[`../specs/antigravity-model-aliases.md`](../specs/antigravity-model-aliases.md).

## Pipeline

1. `internal/scan` discovers and parses every selected source.
2. Parsers return `provider.UnifiedMessage` rows with source, client, model,
   session, message key, timestamp and token breakdown.
3. `internal/usage.Store` upserts the rows into `usage_messages`, de-duplicates
   by ledger key, then rebuilds `daily_model_usage` for affected dates.
4. `internal/report` answers the usage report, day detail and model detail from
   the same ledger.
5. `internal/summary` keeps the 0.6.x CLI summary shape.

## Sources

| Agent | Implementation |
| :--- | :--- |
| Claude Code | `claude.go` — `projects` and `transcripts` JSONL |
| Codex | `codex.go` — rollout JSONL, cumulative counters and forks |
| Copilot | `copilot.go` — OTEL events and session-state summaries |
| OpenCode | `opencode.go` — 1.x/2.x SQLite databases |
| Gemini CLI | `gemini.go` — legacy JSON and streamed JSONL |
| Pi | `pi.go` — session JSONL |
| Antigravity | `antigravity*.go` — local conversation SQLite/protobuf, language-server RPC and learned aliases |

File-backed sources replace rows by changed session; OpenCode upserts by
message id; Antigravity reads its cache and re-resolves raw model identities on
every read.

## Store

`usage.db` keeps:

- `usage_messages` — one row per ledger key with token counts, stored cost and
  the `unpriced` flag;
- `daily_model_usage` — the rollups the CLI summary reads;
- `source_refresh_state` — the incremental refresh anchor;
- `model_price_periods` / `model_releases` — see [`pricing.md`](pricing.md);
- `app_state` — `scanned_at` and reprice time.

Parser versions are unchanged from 0.6.16, so an upgrade never triggers a
stale-parser rebuild. A 0.6.x database migrates in place with a
`*.pre-0.7.<timestamp>.bak` backup (0600).

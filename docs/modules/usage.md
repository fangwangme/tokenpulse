# Usage Module

## Overview

The usage module scans local agent history, normalizes token events into a stable ledger, and renders historical usage views from ledger-backed aggregates.

Current goals:

- ingest local session history from supported agents
- persist normalized messages in SQLite
- derive daily, weekly, and monthly usage from stored aggregates
- estimate historical cost using pricing snapshots captured at ingest time
- power both CLI summaries and the usage TUI from the same aggregate layer

## Provider Status

Current provider maturity:

- `Claude Code`: usable for daily token tracking
- `Codex`: usable for daily token tracking
- `Copilot`: usable for daily token tracking (OTEL events)
- `OpenCode`: usable for daily token tracking
- `Gemini CLI`: historical parser only
- `PI`: parser retained, secondary product scope
- `Antigravity`: usable for daily token tracking and LS sync

## Architecture

```
usage/
├── mod.rs          # aggregate contracts and summary builders
├── store.rs        # SQLite ledger and aggregate queries
├── scanner.rs      # local file discovery
├── claude.rs       # Claude Code JSONL parser
├── codex.rs        # Codex JSONL parser
├── copilot.rs      # GitHub Copilot OTEL JSONL parser
├── opencode.rs     # OpenCode SQLite parser
├── gemini.rs       # Gemini CLI JSON parser
└── pi.rs           # PI JSONL parser
```

The usage pipeline is:

1. scan local session sources
2. parse provider-specific history into `UnifiedMessage`
3. write messages into `usage_messages`
4. rebuild `daily_model_usage` for affected dates
5. derive `DashboardDay`, weekly rollups, monthly rollups, agent summaries, and normalized model summaries from the ledger

For file-backed agents, incremental runs treat a modified session file as the authoritative copy for that session: changed files are parsed in parallel, then the ledger rows for those changed sessions are replaced before daily aggregates are rebuilt. This keeps rewritten or compacted session files from leaving stale messages behind. OpenCode remains row-incremental through its own SQLite timestamp filter, and Antigravity keeps its own cache database, now filled from the conversation SQLite files themselves rather than only from the language server.

## Core Data Model

### Parsed Messages

`UnifiedMessage` is the parser output contract.

```rust
pub struct UnifiedMessage {
    pub client: String,
    pub client_detail: Option<String>,
    pub model_id: String,
    pub provider_id: String,
    pub session_id: String,
    pub message_key: String,
    pub timestamp: i64,
    pub date: String,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub pricing_day: String,
    pub parser_version: String,
}
```

### Dashboard Aggregates

All usage views should read from daily aggregates rather than raw files.

```rust
pub struct DashboardDay {
    pub date: String,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub message_count: i64,
    pub session_count: i64,
    pub intensity_tokens: u8,
    pub intensity_cost: u8,
}

pub struct UsageRollup {
    pub label: String,
    pub start_date: String,
    pub end_date: String,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub message_count: i64,
    pub session_count: i64,
    pub active_days: i64,
}
```

## Ledger Storage

The usage ledger lives in the user cache directory:

- default path: `~/.local/share/tokenpulse/usage.db`

Main tables:

- `usage_messages`: normalized message ledger
- `daily_model_usage`: day/provider/model aggregates
- `daily_pricing_snapshots`: captured pricing used for historical cost

Important rule:

- historical cost should come from stored snapshots and stored rows, not from recomputing against the latest remote pricing on every view

## Parser Notes

### Claude Code

- source paths:
  - `~/.claude/projects`
  - `~/.claude/transcripts`
- parses `assistant` entries with `message.usage`
- deduplicates with `message.id + requestId` when available

### Codex

- source paths:
  - `~/.codex/sessions`
  - `~/.codex/archived_sessions`
  - `$CODEX_HOME/sessions`
- primary token source is `last_token_usage`
- supports fallback delta computation from `total_token_usage`
- includes cumulative-regression guards

### OpenCode

- source path:
  - `~/.local/share/opencode/opencode.db`
- reads assistant messages from SQLite
- uses stored tokens and pricing-based cost estimation when available

### Gemini CLI (Historical Only)

> [!NOTE]
> Gemini support has been deprecated/removed. The Gemini CLI parser is retained for historical data analytics only.

- source path:
  - `~/.gemini/tmp/**/session-*.json`
  - `~/.gemini/tmp/**/session-*.jsonl`
- current parser is still lower-confidence than Claude/Codex/OpenCode/Copilot, but now handles two Gemini-specific quirks:
  - streamed JSONL records are deduplicated by message `id`, keeping the last chunk per response
  - cache-inclusive `input` values are normalized when `total` indicates that `cached` tokens are already included in the prompt count
- timestamp fallback behavior still needs broader validation against real samples
- parser version changes can invalidate previously ingested Gemini rows; `tokenpulse` will automatically clear and rebuild stale Gemini ledger entries on the next run

### PI

- source path:
  - `~/.pi/agent/sessions/**/*.jsonl`
- retained but not a primary dashboard target
- provider detection now follows the same model-family mapping as Copilot

### GitHub Copilot

- source path:
  - `~/.local/share/github-copilot/events.jsonl`
  - `~/.copilot/session-state/**/events.jsonl`
- parses OTEL JSONL events (OpenTelemetry format)
- event name filter: `gen_ai.client.inference.operation.details`
- deduplication by `response_id` within each parse run
- uses official cache fields when present:
  - `gen_ai.usage.cache_read.input_tokens`
  - `gen_ai.usage.cache_creation.input_tokens`
- OTEL fallback for older files: estimates cache by same-session input growth
- Copilot CLI session-state fallback can read `session.shutdown` summaries when OTEL is unavailable
- session-state summaries are aggregate-at-shutdown data, so cross-day daily attribution is approximate
- provider detection from model name is shared across agents
- `codex/gpt/o*` → `openai`
- `claude*` → `anthropic`
- `gemini*` → `google`
- unsupported or miscellaneous model families default to `other`
- quota reset is treated as month-start (`day 1, 00:00`) when GitHub does not return an explicit reset timestamp
- requires VS Code setting: `"github.copilot.chat.otel.enabled": true`

## CLI Behavior

Current command shape:

```bash
tokenpulse
tokenpulse --no-tui
tokenpulse --since 2026-03-01
tokenpulse --refresh-days 2026-03-01:2026-03-07
tokenpulse --refresh-pricing
tokenpulse --rebuild-all
tokenpulse --log
```

`tokenpulse` now opens the interactive dashboard automatically when both stdin/stdout are attached to a terminal. Use `--no-tui` to force the plain-text summary for scripts, pipes, or quick dumps.

`--log` writes startup timing for the current run to a new timestamped file under `~/.local/share/tokenpulse/log/`. The log records provider parsing, ingest, aggregate queries, and the point where the TUI starts, so slow dashboard startup can be traced without enabling logs by default.

Antigravity usage sync maintains a local cache database in `~/.local/share/tokenpulse/antigravity-cache/cache.db`. Regular runs rebuild sessions whose Antigravity or Antigravity CLI conversation files were modified in the last two days. Running with `--rebuild-all` clears the database of parsed messages and fully rebuilds the local SQLite cache database by querying all discoverable sessions from a running Antigravity language server.

Token usage comes from the conversation databases themselves, so it no longer depends on a running language server. Both `~/.gemini/antigravity-cli/conversations/` (CLI) and `~/.gemini/antigravity/conversations/` (Desktop) are scanned; `antigravity-ide/` and `antigravity-backup/` are not, because they hold only encrypted `.pb` files and `backup` duplicates `ide`. For each `.db` conversation the parser decodes the protobuf blobs in the `gen_metadata` table and writes one `session_usage` row per generation, tagged with the current `parser_version` (`antigravity-v3`):

| `gen_metadata` field | language-server field | column |
|---|---|---|
| `1.4.2` | `inputTokens` | `input_tokens` |
| `1.4.5` | `cacheReadTokens` | `cache_read_tokens` |
| `1.4.9` | `thinkingOutputTokens` | `reasoning_tokens` |
| `1.4.10` | `responseOutputTokens` | `output_tokens` |
| `1.4.11` | `responseId` | `response_id` |
| `1.19`, else the `model_enum` pair in `1.20` | `responseModel` | `model_id` |

`output_tokens` deliberately comes from `1.4.10` rather than `1.4.3` (`outputTokens`): `1.4.3` already contains the thinking tokens, and cost calculation adds reasoning on top of output, so `1.4.3` would double-count. Both the local parser and the language-server path funnel through one `normalize_antigravity_tokens` function that applies this rule and verifies `thinking + response == total`, so the two sources cannot drift apart; a record failing that check is logged and skipped rather than stored. When a source reports only the total, the disjoint output is recovered as `total - thinking`. Antigravity reports no cache-write tokens on either path, so `cache_write_tokens` is always 0. Wall-clock times are joined from `steps.metadata` through the request UUID the two tables share; a `Timestamp` whose nanoseconds fall outside `[0, 1e9)` is not one and is skipped.

Opening the cache migrates any `antigravity-v2` usage row in place, splitting its total output into `output_tokens` and `reasoning_tokens`. That both corrects the old double-count and guarantees no row lingers at an outdated `parser_version`: because `parse_sessions` reports each row's stored version to the ledger, one row the local parser can never re-read — an encrypted `.pb` session — would otherwise mark the source stale on every launch.

Encrypted `.pb` conversations carry no readable usage and remain the language server's job. Both paths write the same `client:session_id:responseId` key with `INSERT OR REPLACE`, so a session read locally and over RPC overwrites in place instead of duplicating. A conversation is re-read when its file (or its `-wal`/`-shm` sidecar) is newer than the cached copy, or when the cache holds no usage row for it at the current `parser_version` — which is what backfills a cache built by an earlier release, and what makes a future `parser_version` bump re-read every conversation. Because that rescan touches every session at once, the local upsert only fills the columns a conversation database actually holds and leaves language-server-supplied metadata (title, workspace path, git root, branch) untouched.

Antigravity CLI and Desktop are treated as sub-clients of the same `antigravity` source. The parser stores the concrete runtime in `client_detail` (`antigravity-cli` or `antigravity-desktop`) and uses a storage key shaped like `client:session_id:message_id`, while usage aggregates deduplicate on the logical `antigravity + session_id + message_id` key. This allows the same message to exist in both CLI and Desktop cache paths without counting its tokens twice.

Claude Code, Codex, Copilot, Gemini CLI, and PI do not maintain separate raw cache databases. Their normal incremental path discovers session files by mtime, parses matching files concurrently, and replaces only the sessions represented by those files. Range refreshes and full rebuilds still use the broader source/date clearing paths.

Non-TUI output includes:

- overall totals
- by-provider totals
- by-model totals (all models)
- recent daily totals (up to the last 365 days)
- weekly totals
- monthly totals

The daily CSV export (`--csv`) emits separate `cache_read_tokens` and `cache_write_tokens` columns rather than a single combined cache figure.

The plain-text and `--json` outputs also append quota snapshots for enabled
providers. Both honor `display.refresh_quota`: when it is off they build no
quota fetcher and contact no quota API, showing only unexpired cached
snapshots. This matches the TUI, where the same setting gates startup,
auto-refresh, and manual `r`. `--csv` never fetches quota.

## TUI Model

The usage TUI is organized into six tabs:

- `Overview` - 60-day stacked bar chart by model company + scrollable top models table
- `Models` - Full searchable/sortable model table with company-colored model names, sort-aware share percentage, and colored numeric columns
- `Daily` - Daily summary bar and table with sorting and 7-day token trends on wide terminals
- `Activity` - GitHub-style calendar heatmap with range stats and selected-day drill-down
- `Quota` - Live quota monitoring dashboard with expected-progress markers, reset times, and credits/balance tracking
- `Settings` - Interactive settings panel to toggle refresh intervals, theme preference, and configure provider visibility

### Source Filtering

All tabs support runtime source filtering:
- Press `s` to open filter overlay
- Toggle individual providers on/off
- Data in all views updates immediately
- Config file (`~/.local/share/tokenpulse/config.toml`) controls which providers are loaded

### `Overview`

- chart shows the last 60 days of token usage
- press `t` or `c` to switch the chart between tokens and cost
- stacked bars are grouped by model company (`OpenAI`, `Google`, `Anthropic`, `Others`)
- top models are normalized before ranking
- top models use row selection; the visible window only moves when the selected row reaches an edge
- model and agent columns are intentionally wider so long names are still legible
- each row shows cost share using the actual filtered total cost

Primary historical dashboard view:

- 60-day stacked bar chart (tokens by company)
- Scrollable top models by cost
- Company-colored legend

### `Models`

Model attribution view:

- Sortable table (cost, tokens, date)
- Quick filter with `/`
- Company-colored model names
- Wider agent column for multi-agent attribution strings
- `%` column reflects the active sort basis for the filtered model total: cost share for cost/date sort, token share for token sort
- Semantic numeric colors: tokens=green, cost=gold, messages=blue
- Filtered by enabled sources

### `Daily`

Daily operations view:

- Summary bar with Today, This Week, This Month, period cost, tokens, messages, and sessions
- Daily table with today highlighted
- 7-day token sparkline on wide terminals
- Semantic numeric colors by column
- Sortable by date/cost/tokens

### `Activity`

Activity calendar heatmap:

- 2 switchable metrics: total tokens and cost
- 3 window modes (past 26 weeks, past 52 weeks, past 365 days)
- GitHub-style calendar layout; intensity uses visible-window peak scaling, not equal-count quantiles
- `<= 0` renders as empty, and positive values use five buckets at 20/40/60/80% of the visible window max
- Cost uses a GitHub-green palette, tokens use a Kaggle-blue palette, and the heatmap surface is theme-invariant (with soft gray background and cell border colors) for consistent low-level readability across both light and dark themes
- Narrow terminals clip to the most recent visible weeks instead of merging multiple dates into one cell
- Clickable legend levels show the current token/cost range for that intensity bucket
- Range overview includes Today, This Week, This Month, and all-time cost
- Day drill-down with:
  - Agent totals with per-agent cost
  - Token summary (total/input/output/cache/reasoning/messages/sessions)
  - Per-agent model list with per-model cost
  - Scrollable selected-day detail panel when content exceeds the viewport, with the scroll hint rendered separately so the last model token line stays visible
### `Quota`

Quota usage monitoring view:

- Displays rate limits (e.g. Session 5h, Weekly 7d) with progress gauges, expected progress indicators, and time to reset/limit.
- Displays remaining balance or used credits depending on the active display mode.

### `Settings`

Settings and configuration view:

- Toggle quota display mode (`used` or `remaining` credit balance)
- Enable/disable individual providers
- Set and save `auto_refresh_interval` (0, 1, 2, 5, 10, 15 min) — shared by quota + usage
- Cycle through theme preference (`auto`, `dark`, `light`)
- Toggle active Antigravity session scanning and alias synchronization (`scan_antigravity`: true / false)
- Enable / disable quota balance refresh (`refresh_quota`: true / false, default true)
- Space or Enter keys cycle or toggle the active setting, and Up/Down (`j`/`k`) keys move selection.


## Known Limits

Current limits worth keeping in mind:

- durable append-only scan-state is not complete yet
- weekly/monthly `session_count` should not be treated as audit-grade unique-session counts yet
- Gemini historical accuracy needs more fixtures
- Antigravity `.pb` conversations (encrypted) still require the active language server process; `.db` conversations are parsed directly
- cost accuracy depends on model pricing matching or source-provided cost

## Working Rules

- parse once, normalize once, aggregate many times
- keep quota status and historical usage separate
- treat daily rows as the dashboard source of truth
- avoid dashboard business logic in the TUI layer

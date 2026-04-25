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
- `Gemini CLI`: provisional parser, needs more real-world validation
- `PI`: parser retained, secondary product scope
- `Antigravity`: quota support exists, historical usage support is not complete

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

## Core Data Model

### Parsed Messages

`UnifiedMessage` is the parser output contract.

```rust
pub struct UnifiedMessage {
    pub client: String,
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

- default path: `~/.cache/tokenpulse/usage.sqlite3`

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

### Gemini CLI

- source path:
  - `~/.gemini/tmp/session-*.json`
- current parser is provisional
- timestamp fallback behavior still needs validation against real samples

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
tokenpulse usage
tokenpulse usage --tui
tokenpulse usage --no-tui
tokenpulse usage --since 2026-03-01
tokenpulse usage -p claude,codex,opencode
tokenpulse usage --refresh-days 2026-03-01:2026-03-07
tokenpulse usage --refresh-pricing
tokenpulse usage --rebuild-all
```

`tokenpulse usage` now opens the interactive dashboard automatically when both stdin/stdout are attached to a terminal. Use `--no-tui` to force the plain-text summary for scripts, pipes, or quick dumps.

Non-TUI output includes:

- overall totals
- by-provider totals
- by-model totals
- recent daily totals
- weekly totals
- monthly totals

## TUI Model

The usage TUI is organized into four tabs:

- `Overview` - summary cards + 60-day stacked bar chart by model company + scrollable top models table
- `Models` - Full searchable/sortable model table with company-colored model names and colored numeric columns
- `Daily` - Daily summary table with sorting and 7-day token trends on wide terminals
- `Activity` - GitHub-style contribution graph with drill-down

### Source Filtering

All tabs support runtime source filtering:
- Press `s` to open filter overlay
- Toggle individual providers on/off
- Data in all views updates immediately
- Config file (`~/.config/tokenpulse/config.toml`) controls which providers are loaded

### `Overview`

- chart shows the last 60 days of token usage
- press `t` or `c` to switch the chart between tokens and cost
- stacked bars are grouped by model company (`OpenAI`, `Google`, `Anthropic`, `Others`)
- top models are normalized before ranking
- top models use row selection; the visible window only moves when the selected row reaches an edge
- model and agent columns are intentionally wider so long names are still legible

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
- Semantic numeric colors: tokens=green, cost=gold, messages=blue
- Filtered by enabled sources

### `Daily`

Daily operations view:

- Summary cards (cost, tokens, messages, sessions)
- Daily table with today highlighted
- 7-day token sparkline on wide terminals
- Semantic numeric colors by column
- Sortable by date/cost/tokens

### `Activity`

GitHub-style contribution graph:

- 7 switchable metrics
- 3 window modes (past 26 weeks, past 52 weeks, past 365 days)
- Day drill-down with:
  - Agent totals with per-agent cost
  - Token summary (total/input/output/cache/reasoning/messages/sessions)
  - Per-agent model list with per-model cost
  - Scrollable selected-day detail panel when content exceeds the viewport
- Streak tracking

## Known Limits

Current limits worth keeping in mind:

- durable append-only scan-state is not complete yet
- weekly/monthly `session_count` should not be treated as audit-grade unique-session counts yet
- Gemini historical accuracy needs more fixtures
- Antigravity historical usage is not complete
- cost accuracy depends on model pricing matching or source-provided cost

## Working Rules

- parse once, normalize once, aggregate many times
- keep quota status and historical usage separate
- treat daily rows as the dashboard source of truth
- avoid dashboard business logic in the TUI layer

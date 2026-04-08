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

Current provider maturity as of 2026-03-24:

- `Claude Code`: usable for daily token tracking
- `Codex`: usable for daily token tracking
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
├── opencode.rs     # OpenCode SQLite parser
├── gemini.rs       # Gemini CLI JSON parser
└── pi.rs           # PI JSONL parser
```

The usage pipeline is:

1. scan local session sources
2. parse provider-specific history into `UnifiedMessage`
3. write messages into `usage_messages`
4. rebuild `daily_model_usage` for affected dates
5. derive `DashboardDay`, weekly rollups, monthly rollups, provider summaries, and model summaries from the ledger

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

The usage TUI is centered on three tabs:

- `GitHub`
- `By Day`
- `By Model`

### `GitHub`

Primary historical dashboard view:

- contribution heatmap
- metric switching
- range switching
- selected-day detail
- day-level source breakdown

### `By Day`

Daily operations view:

- daily totals table
- latest week and month summary cards
- token, cost, message, and cache totals

### `By Model`

Consumption attribution view:

- provider ranking
- model ranking
- provider/model token and cost splits

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

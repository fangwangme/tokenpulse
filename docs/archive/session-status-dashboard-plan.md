# Archived: TokenPulse Session Status And Dashboard Plan

This was a planning document for the usage dashboard work completed after 2026-03-22.
It is retained for historical context; current behavior is documented in `docs/DESIGN.md`
and `docs/modules/usage.md`.

## Goal

Expand TokenPulse so it can:

- scan local history from major coding agents
- normalize token usage into a stable ledger
- compute daily, weekly, and monthly usage
- estimate cost with historical pricing semantics
- render a GitHub-style dashboard and supporting summaries

Initial target provider scope:

- Claude Code
- Codex
- OpenCode
- Gemini CLI
- Antigravity

Supporting or retained scope:

- PI, if we keep it in the product

Required output scope:

- daily token totals
- weekly token totals
- monthly token totals
- daily estimated cost
- weekly estimated cost
- monthly estimated cost

---

## Current Project Status

Status as of 2026-03-22:

### Already in place

- parser coverage already exists for Claude, Codex, OpenCode, PI, and an early Gemini path
- normalized `UnifiedMessage` output already exists
- usage data can already be ingested into a local SQLite ledger
- day-level aggregate storage already exists in `daily_model_usage`
- the usage TUI already has multiple pages plus early `heatmap` and `trend` widgets
- the current test suite is passing

### In progress on the branch

- pricing snapshot persistence
- ledger-backed usage ingestion flow
- expanded quota support
- a richer usage dashboard UI
- the current branch keeps a 4-tab usage layout: `Overview`, `Models`, `Daily`, `Heatmap`

### Still missing or not settled

- a final dashboard data contract that all views agree on
- a stable tab model centered on the three views we actually want
- durable scan-state persistence for append-only file sources
- weekly and monthly rollups as first-class outputs
- a finalized GitHub-style interaction model

### Assessment

The project foundation is meaningful already.

The main problem is not “can TokenPulse parse anything?”.
The main problem is “what exact dashboard product are we building on top of the parser and ledger that already exist?”.

---

## Product Boundaries

TokenPulse has two separate products:

### Live provider status

This is quota-oriented.

Examples:

- plan
- session usage %
- weekly usage %
- pool/model window usage %
- reset times
- credits or spend cap

Source:

- provider APIs
- local auth/session material

### Historical usage dashboard

This is ledger-oriented.

Examples:

- today
- yesterday
- last 7 days
- last 30 days
- current month
- year heatmap
- weekly and monthly totals

Source:

- local session history only

Rule:

- keep `quota` and `usage` separate in code and data flow

---

## Data Model Plan

### Layer 1: normalized parsed messages

Use `UnifiedMessage` as the canonical parser output.

Keep current fields:

- `client`
- `provider_id`
- `model_id`
- `session_id`
- `message_key`
- `timestamp`
- `date`
- `tokens`
- `cost`
- `pricing_day`
- `parser_version`

Add optional source metadata in a backward-compatible way later:

- `source_path`
- `source_format`
- `source_kind`
- `message_id`
- `request_id`

### Layer 2: ledger storage

Persist normalized messages in the usage ledger.

The ledger must be the long-lived source of truth for historical usage.

### Layer 3: daily aggregates

Add a stable day-level dashboard row.

Recommended shape:

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
```

All dashboard views should derive from daily rows, not raw messages.

### Layer 4: weekly and monthly aggregates

Weekly and monthly summaries should derive from daily rows.

Do not rescan raw logs to answer weekly/monthly views.

---

## Parser Plan

### Claude

- keep current JSONL parser and dedup logic
- preserve `message.id + requestId` dedup
- later add headless/event-stream support if real samples require it

### Codex

- keep `last_token_usage` as the primary delta source
- keep cumulative regression detection
- keep archived session support

### OpenCode

- keep SQLite-first parsing
- improve time-window filtering if scan cost becomes material

### Gemini

- validate against real local samples before considering parser complete
- treat current parser as provisional until sample coverage improves

### Antigravity

- do not force it into the Gemini path
- first solve:
  - local source discovery
  - session identity
  - stable timestamps
- only then add token accounting

---

## Scan-State Plan

TokenPulse should add durable scan-state persistence similar to `CodexBar`.

Store per file:

- file path
- mtime
- size
- parsed bytes
- continuation state
- discovered session id
- days contributed by that file

Primary goals:

- skip unchanged files
- continue parsing appended files from the last offset
- subtract old per-file contributions before replacing them

Initial priority:

1. Codex
2. Claude
3. Gemini or other file-based providers

OpenCode can remain ledger/database-first because its source is SQLite, not append-only JSONL.

---

## Pricing Plan

Historical cost should not drift.

Rule:

- historical rows use the pricing snapshot for that message day
- refreshing pricing should affect new ingests, not silently rewrite old history
- explicit rebuild can recalculate if the user asks for it

This means:

- pricing snapshots are part of the ledger contract
- dashboard totals should read stored cost, not recompute from latest remote pricing on every view

---

## Dashboard Plan

### Primary dashboard tabs

The current branch dashboard is organized around four views:

#### 1. `Overview`

Purpose:

- summary-first landing page
- quick ranking of the most expensive models

Contents:

- 60-day stacked usage chart
- company-colored legend (`OpenAI`, `Google`, `Anthropic`, `Others`)
- scrollable top-models table

#### 2. `Models`

Purpose:

- answer “which agents and models are consuming usage”

Contents:

- sortable model ranking
- agent attribution per model
- cost/token/message columns with semantic colors

#### 3. `Daily`

Purpose:

- answer “what happened each day”

Contents:

- daily totals table
- per-column token/cost breakdown
- sortable daily view

#### 4. `Heatmap`

Purpose:

- a GitHub-style contribution calendar for tokens or cost
- the drill-down view for selected days

Contents:

- contribution heatmap
- range selector
- metric selector
- token mode and cost mode
- selected-day detail grouped by agent, then model, with cost totals
- summary strip

#### Optional 4. `Sessions`

Purpose:

- debugging and deep inspection

Contents:

- session-level breakdown
- span, message count, token count, cost

This is useful, but secondary to the three core tabs above.

### Recommended naming

Prefer stable user-facing names that match the shipped branch:

- `Overview`
- `Models`
- `Daily`
- `Heatmap`
- `Sessions` (if added later)

Branch note:

- the current TUI implementation intentionally keeps `Overview` as a separate summary tab
- `Heatmap` is still the GitHub-style contribution view and carries the day-level drill-down

### Heatmap tab

Make `GitHub` the canonical historical dashboard view.

It should support:

- year or rolling range selection
- metric switching
  - total tokens
  - cost
  - input
  - output
  - sessions
- provider filtering
- selected-day detail panel

### Calendar logic

Implement:

1. fill missing days
2. stable week grouping
3. metric-based intensity calculation
4. detail lookup for selected day

Default behavior:

- Sunday-based week layout to match GitHub
- relative intensity buckets `0..4`

Later enhancement:

- absolute thresholds
- palette switching
- month labels and streak stats

---

## CLI And JSON Plan

Extend usage outputs to include:

- daily dashboard rows
- weekly totals
- monthly totals
- provider and model splits for selected ranges

This makes the TUI and machine-readable outputs use the same backend contract.

---

## Implementation Sequence

### Phase 1

Finalize the dashboard-facing daily/weekly/monthly schema in `tokenpulse-core`.

Deliverables:

- stable daily dashboard contract
- explicit weekly and monthly rollup shapes
- agreement that all dashboard views derive from daily aggregates

### Phase 2

Add durable scan-state persistence for Codex and Claude.

Deliverables:

- per-file scan-state schema
- append-only continuation rules
- invalidate/rebuild rules

### Phase 3

Finalize the usage tab model.

Deliverables:

- exact tab list
- exact content per tab
- exact keyboard and navigation model

### Phase 4

Move the `GitHub` tab to ledger-derived daily aggregates instead of recomputing from raw messages in-memory.

Deliverables:

- daily aggregate read path
- selected-day detail contract
- shared data path for all dashboard views

### Phase 5

Add intensity calculation by metric and range.

Deliverables:

- relative intensity buckets
- stable empty-day fill behavior
- range-aware summary metrics

### Phase 6

Add weekly and monthly summaries to CLI JSON and TUI views.

Deliverables:

- daily, weekly, monthly JSON output
- `Daily`, `Models`, and `Heatmap` views backed by the same aggregate layer

### Phase 7

Improve Gemini coverage with real fixtures.

### Phase 8

Design Antigravity historical support around discovery and identity before token accounting.

---

## Working Rules

1. Parse once, normalize once, aggregate many times.
2. Do not compute dashboard business logic from raw files in the view layer.
3. Keep quota status and historical usage separate.
4. Treat daily rows as the dashboard source of truth.
5. Treat Antigravity as a staged integration, not a quick parser addition.

---

## Immediate Next Steps

1. Add dashboard aggregate types in `tokenpulse-core`.
2. Add scan-state persistence for Codex and Claude.
3. Keep the current 4-tab layout stable while continuing to treat `Heatmap` as the GitHub-style view.
4. Route the current GitHub-style tab through stored daily aggregates.
5. Add weekly and monthly rollups to usage commands.
6. Add anonymized fixture tests for real-world parser inputs.

---

## Immediate Planning Decision

Before any more UI implementation, these decisions should be treated as fixed on this branch:

1. The current usage tabs are `Overview`, `Models`, `Daily`, and `Heatmap`.
2. `Heatmap` is the GitHub-style historical activity view.
3. All usage tabs read from the same ledger-derived daily aggregate layer.
4. Session-level views remain diagnostic, not the main product.

Once these are stable, the implementation path becomes much less likely to thrash.

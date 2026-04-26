# TokenPulse - Design Document v1

## Overview

A Rust CLI tool with two core features:
1. **Quota** - On-demand check of remaining usage quota for coding agents
2. **Usage** - Ledger-backed historical usage dashboard with cost estimation

**Current Usage Scope:** Claude Code, Codex, OpenCode, Gemini CLI, PI, Copilot CLI
**Current Quota Scope:** Claude Code, Codex, Gemini CLI, GitHub Copilot, Antigravity
**Maturity Note:** Historical usage is strongest today for Claude Code, Codex, and OpenCode. Gemini CLI is provisional. Antigravity historical usage is not complete yet.

**Language:** Rust
**Key Principle:** On-demand only. No auto-refresh, no polling. Run command → see results → exit.

---

## Current State

As of 2026-04-25:

- usage parsing writes normalized messages into a local SQLite ledger
- the dashboard reads daily aggregates from the ledger, not from raw files in the TUI layer
- the usage TUI is organized around `Overview`, `Models`, `Daily`, and `Activity`
- CLI usage output includes daily, weekly, and monthly summaries
- pricing snapshots are stored per day/model so historical cost does not silently drift
- quota view shows top 3 windows per provider in Overview tab; all windows in per-provider detail tabs
- each quota gauge shows an expected-progress marker (`▏`) and ETA to limit
- activity heatmap uses block characters (`░▒▓█`) scaled to value intensity for colorblind accessibility

Known gaps:

- durable scan-state persistence for append-only sources is not finished
- Gemini CLI historical coverage still needs more sample validation
- Antigravity historical usage is still staged work
- weekly/monthly session counts should not yet be treated as fully deduplicated unique-session metrics

---

## Project Structure

```
tokenpulse/
├── Cargo.toml                    # workspace root
├── AGENTS.md
├── docs/
│   └── DESIGN.md                 # this file
│
├── tokenpulse-core/              # library crate
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── provider.rs           # UnifiedMessage, TokenBreakdown, provider traits
│       ├── auth/                 # credential loading and token refresh
│       ├── quota/                # API-based quota fetching and cache
│       ├── usage/
│       │   ├── mod.rs            # dashboard contracts and summary builders
│       │   ├── store.rs          # SQLite usage ledger
│       │   ├── scanner.rs        # local discovery
│       │   ├── claude.rs         # Claude Code parser
│       │   ├── codex.rs          # Codex parser
│       │   ├── copilot.rs        # GitHub Copilot OTEL parser
│       │   ├── opencode.rs       # OpenCode parser
│       │   ├── gemini.rs         # Gemini CLI parser
│       │   ├── pi.rs             # PI parser
│       │   └── utils.rs          # model/provider normalization helpers
│       ├── quota/
│       │   ├── mod.rs
│       │   ├── claude.rs
│       │   ├── codex.rs
│       │   ├── copilot.rs
│       │   ├── gemini.rs
│       │   ├── antigravity.rs
│       │   └── cache.rs
│       └── pricing/              # model pricing and cost calculation
│
└── tokenpulse-cli/               # binary crate
    ├── Cargo.toml
    └── src/
        ├── main.rs
        ├── commands/
        │   ├── mod.rs
        │   ├── quota.rs
        │   └── usage.rs
        └── tui/
            ├── mod.rs
            ├── theme.rs
            ├── widgets/
            │   ├── mod.rs
            │   ├── gauge.rs
            │   ├── heatmap.rs
            │   ├── trend.rs
            │   └── barchart.rs
            └── views/
                ├── mod.rs
                ├── quota.rs
                └── usage.rs
```

---

## Dependencies

| Crate | Purpose |
|---|---|
| `clap` (derive) | CLI argument parsing |
| `reqwest` (json, rustls-tls) | HTTP for quota APIs + pricing |
| `tokio` (rt-multi-thread) | async runtime |
| `serde` + `serde_json` | JSON serialization |
| `chrono` | timestamps |
| `ratatui` + `crossterm` | TUI framework for fancy dashboards |
| `rusqlite` (bundled) | OpenCode SQLite parsing |
| `walkdir` | directory traversal |
| `rayon` | parallel file parsing |
| `dirs` | home directory paths |
| `anyhow` | error handling |
| `humantime` | "3h 12m" style formatting |

---

## CLI Interface

```bash
# Quota checking - one-shot, pretty output
tokenpulse quota                          # all providers
tokenpulse quota -p claude                # single provider
tokenpulse quota --json                   # JSON output for scripting

# Usage summary / dashboard
tokenpulse usage                          # interactive TUI on a terminal, plain text when piped
tokenpulse usage --tui                    # force the interactive TUI dashboard
tokenpulse usage --no-tui                 # plain-text summary
tokenpulse usage --json                   # JSON summary for scripts
tokenpulse usage --since 2026-03-01       # filter by date
tokenpulse usage -p claude,codex          # filter by provider
tokenpulse usage --refresh-days 2026-03-01:2026-03-07
tokenpulse usage --refresh-pricing
tokenpulse usage --rebuild-all
```

---

## TUI Dashboard Design

### Quota View (`tokenpulse quota`)

The quota TUI has two modes:
- **Overview tab** shows only the top 3 most-used windows per provider for a compact summary
- **Detail tabs** (per provider) show all available rate windows

Each gauge includes:
- A gradient color progress bar
- An expected-progress marker (`▏`) showing where theoretical usage should be at this point in time
- Pace ETA: when ahead of pace, shows estimated time to limit; when behind, shows "under pace"
- Fixed-width label columns for proper alignment (especially for Gemini CLI's multiple models)
- GitHub Copilot uses dynamic calendar-month billing period calculation

```
╭─────────────────────────────────────────────────────────────────────╮
│                    ⚡ TokenPulse - Quota Overview                    │
╰─────────────────────────────────────────────────────────────────────╯

  ╭─ CLAUDE CODE ───────────────────────────────────────────────────╮
  │  Plan: Pro                                                      │
  │                                                                 │
  │  Session (5h)   ████████████▏░░░░░░░░░░░░░░░░░  42%  ⏳ 3h 12m │
  │  Weekly (7d)    █████▏░░░░░░░░░░░░░░░░░░░░░░░░  18%  ⏳ 4d 6h  │
  ╰─────────────────────────────────────────────────────────────────╯

  ╭─ GITHUB COPILOT ────────────────────────────────────────────────╮
  │  Plan: Pro                                                      │
  │                                                                 │
  │  Completions    ██████████████████▏░░░░░░░░░░░  67%  ⏳ 12d    │
  ╰─────────────────────────────────────────────────────────────────╯
```

### Usage Dashboard (`tokenpulse usage`)

```
╭─────────────────────────────────────────────────────────────────────╮
│                   📊 TokenPulse - Usage Dashboard                    │
╰─────────────────────────────────────────────────────────────────────╯

  ╭─ Token Usage (60 days) ───────────────────────────────────────────╮
  │                                                                   │
  │  $12 ┤                              ╭─╮                           │
  │  $10 ┤          ╭─╮        ╭─╮      │ │                           │
  │   $8 ┤    ╭─╮   │ │  ╭─╮  │ │ ╭─╮  │ │  ╭─╮                     │
  │   $6 ┤ ╭─╤ │╭╮  │ │  │ │  │ │ │ │  │ │  │ │                     │
  │   $4 ┤ │ ││ ││╭╮ │ │╭╮│ │╭╮│ │ │ │╭╮│ │╭╮│ │╭╮                  │
  │   $2 ┤ │ ││ ││││ │ ││││ │││├─┤ │ ││││ ││││ │││                  │
  │   $0 ┼─┴─┴┴─┴┴┴┴─┴─┴┴┴┴─┴┴┴┴─┴─┴─┴┴┴┴─┴┴┴┴─┴┴┴─               │
  │       03/01  03   05   07   09   11   13   15                     │
  │                                                                   │
  │  Legend: ██ Claude  ██ Codex  ██ OpenCode  ██ PI                  │
  ╰───────────────────────────────────────────────────────────────────╯

  ╭─ Provider Breakdown ──────────────────────────────────────────────╮
  │                                                                   │
  │  Claude     ██████████████████████████████░░░░░░  62%   $48.30   │
  │  Codex      █████████████░░░░░░░░░░░░░░░░░░░░░░  28%   $21.70   │
  │  OpenCode   ████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░   8%    $6.20   │
  │  PI         █░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░   2%    $1.50   │
  │                                                                   │
  │  Total: $77.70                                                    │
  ╰───────────────────────────────────────────────────────────────────╯

  ╭─ Token Details ───────────────────────────────────────────────────╮
  │                                                                   │
  │  Provider   │ Model           │ Input    │ Output  │ Cache  │ $   │
  │  ───────────┼─────────────────┼──────────┼─────────┼────────┼──── │
  │  Claude     │ opus-4          │ 2.1M     │ 560K    │ 1.4M   │ 32  │
  │  Claude     │ sonnet-4        │ 890K     │ 230K    │ 670K   │ 16  │
  │  Codex      │ o3              │ 1.2M     │ 340K    │ 0      │ 14  │
  │  Codex      │ gpt-4.1        │ 450K     │ 120K    │ 0      │  8  │
  │  OpenCode   │ sonnet-4        │ 340K     │ 90K     │ 200K   │  6  │
  │  PI         │ claude-4-sonnet │ 120K     │ 45K     │ 80K    │  2  │
  ╰───────────────────────────────────────────────────────────────────╯

  ╭─ Model Cost Distribution ─────────────────────────────────────────╮
  │                                                                   │
  │  opus-4         ████████████████████░░░░░░  41%   $31.90         │
  │  sonnet-4       ████████████████░░░░░░░░░░  28%   $21.80         │
  │  o3             ████████░░░░░░░░░░░░░░░░░░  18%   $14.00         │
  │  gpt-4.1        █████░░░░░░░░░░░░░░░░░░░░░  10%    $7.70         │
  │  other          █░░░░░░░░░░░░░░░░░░░░░░░░░   3%    $2.30         │
  ╰───────────────────────────────────────────────────────────────────╯

  Tab: [Overview] [Models] [Daily] [Activity]
  Press q to quit │ ←/→ switch tabs │ ↑/↓ move selected row/day
```

Current usage TUI notes:

- `Overview` shows a 60-day stacked chart switchable between token and cost views, plus a scrollable `Top Models` table
- `Overview` top models use their own visible scroll hint, cost percentage, and wider model/agent columns so long model IDs and multi-agent attribution fit better
- `Models` shows a searchable (`/`), sortable table with cost percentage and per-column semantic colors (`Model`=company color, `Tokens`=green, `Cost`=gold, `Msgs`=blue)
- `Daily` shows Today/This Week/This Month cost, period totals, and daily rows as a colored table (`Tokens`, `Cost`, `Input`, `Output`, `Cache`, `Msgs`) with a 7-day token trend column on wide terminals
- `Activity` shows range cost stats, a GitHub-style contribution heatmap with block-character intensity (`░▒▓█`) scaled to value level for accessibility, mouse-clickable cells, and selected-day drill-down grouped by agent first, then model, with agent/model cost totals
- `Activity` selected-day panel includes total/input/output/cache/reasoning/message/session summary and supports detail scrolling when the agent/model list exceeds the viewport
- Press `s` on any tab to open a source filter overlay (toggle providers on/off)

**Company vs Agent Distinction:**
- **Company color** = model family owner (`OpenAI`, `Google`, `Anthropic`, `Others`)
- **Agent** = client tool (`Claude Code`, `Codex`, `OpenCode`, `Gemini CLI`, `Copilot CLI`, `Pi`, `Antigravity`)

The TUI uses company color for model names and chart segments, while agent/source labels remain textual attribution. In data model terms, `UnifiedMessage.client` = agent and `UnifiedMessage.provider_id` = provider/backend identifier.

---

## Data Models

### Quota

```rust
pub struct QuotaSnapshot {
    pub provider: String,           // "claude", "codex"
    pub plan: Option<String>,       // "Pro", "Plus"
    pub windows: Vec<RateWindow>,
    pub credits: Option<CreditInfo>,
    pub fetched_at: DateTime<Utc>,
}

pub struct RateWindow {
    pub label: String,              // "Session (5h)", "Weekly"
    pub used_percent: f64,          // 0.0 - 100.0
    pub resets_at: Option<DateTime<Utc>>,
}

pub struct CreditInfo {
    pub used: f64,
    pub limit: Option<f64>,         // None = unlimited
    pub currency: String,           // "USD"
}
```

### Usage

```rust
pub struct TokenBreakdown {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
}

pub struct UnifiedMessage {
    pub client: String,             // "claude", "codex", "opencode", "pi"
    pub model_id: String,           // "claude-opus-4", "o3"
    pub provider_id: String,        // "anthropic", "openai"
    pub session_id: String,
    pub timestamp: i64,             // Unix ms
    pub date: String,               // "YYYY-MM-DD"
    pub tokens: TokenBreakdown,
    pub cost: f64,                  // calculated USD
}
```

---

## API Details

### Claude Code Quota

```
GET https://api.anthropic.com/api/oauth/usage
Headers:
  Authorization: Bearer <token>
  anthropic-beta: oauth-2025-04-20

Credentials: ~/.claude/.credentials.json
  → claudeAiOauth.accessToken, refreshToken, expiresAt
  → Fallback: macOS Keychain "Claude Code-credentials"

Token refresh:
  POST https://platform.claude.com/v1/oauth/token
  Body: grant_type=refresh_token&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&refresh_token=<token>
```

### Codex Quota

```
GET https://chatgpt.com/backend-api/wham/usage
Headers:
  Authorization: Bearer <token>

Credentials: ~/.config/codex/auth.json or ~/.codex/auth.json
  → tokens.access_token, tokens.refresh_token

Token refresh:
  POST https://auth.openai.com/oauth/token
  Body (form): grant_type=refresh_token&client_id=app_EMoamEEZ73f0CkXaXp7hrann&refresh_token=<token>
```

### Session File Locations

| Agent | Path | Format |
|---|---|---|
| Claude Code | `~/.claude/projects/**/*.jsonl` | JSONL with type=assistant, message.usage |
| Codex | `~/.codex/sessions/*.jsonl` | JSONL with model, token deltas |
| OpenCode | `~/.local/share/opencode/opencode.db` | SQLite, messages table |
| PI | `~/.pi/agent/sessions/**/*.jsonl` | JSONL with header + entries |
| GitHub Copilot | `~/.local/share/github-copilot/events.jsonl` | OTEL JSONL events |
| Gemini CLI | `~/.gemini/tmp/session-*.json` | JSON session files |

### Pricing Source

```
GET https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
Cache: ~/.cache/tokenpulse/pricing.json (24h TTL)
```

---

## Implementation Phases

### Phase 1 - MVP
- [x] Cargo workspace setup
- [x] Claude Code: auth + quota fetching
- [x] Codex: auth + quota fetching
- [x] `tokenpulse quota` with TUI gauge display
- [x] Claude Code: session JSONL parser
- [x] Codex: session JSONL parser
- [x] Pricing module (LiteLLM fetch + cache)
- [x] `tokenpulse usage` with TUI dashboard

### Phase 2 - More Providers
- [x] OpenCode: SQLite session parser
- [x] PI: session JSONL parser
- [x] Gemini CLI: auth + quota + provisional session parser
- [x] GitHub Copilot: quota + usage parser
- [x] Antigravity: quota probe
- [ ] Antigravity: historical usage parser

### Phase 3 - Polish
- [x] More TUI tabs: Overview, Models, Daily, Activity
- [x] Color theming
- [x] Usage `--json` export mode
- [x] Overview token/cost chart toggle and scrollable top models table
- [x] Models quick filter (`/`)
- [x] Daily token trend column
- [x] Source filter overlay (`s`)
- [x] Block-character heatmap intensity for accessibility
- [x] Pace ETA and expected-progress marker on quota gauges
- [ ] Configurable TUI theme
- [ ] `--watch` mode (manual refresh with keyboard)

---

## Extensibility

Adding a new provider requires:
1. `auth/<provider>.rs` - credential loading (if quota needed)
2. `quota/<provider>.rs` - implement `QuotaFetcher` trait
3. `usage/<provider>.rs` - implement `SessionParser` trait
4. Register in `provider.rs` registry

```rust
#[async_trait]
pub trait QuotaFetcher {
    fn provider_name(&self) -> &str;
    async fn fetch_quota(&self) -> Result<QuotaSnapshot>;
}

pub trait SessionParser {
    fn provider_name(&self) -> &str;
    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>>;
}
```

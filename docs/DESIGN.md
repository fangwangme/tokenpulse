# TokenPulse - Design Document v1

## Overview

A Rust CLI tool with two core features:
1. **Quota** - On-demand check of remaining usage quota for coding agents
2. **Usage** - Ledger-backed historical usage dashboard with cost estimation

**Current Usage Scope:** Claude Code, Codex, OpenCode, Gemini CLI (historical only), PI, Copilot CLI, Antigravity
**Current Quota Scope:** Claude Code, Codex, GitHub Copilot, Antigravity
**Maturity Note:** Historical usage is strongest today for Claude Code, Codex, and OpenCode. Gemini CLI has been deprecated and is retained for historical data analytics only.

**Language:** Rust
**Key Principle:** On-demand by default, with optional auto-refresh. Run command → see results → exit. Configure a single auto-refresh interval (default 5m, applies to both quota and usage) in the CLI or live in the TUI Settings tab.

---

## Current State

As of 2026-04-25:

- usage parsing writes normalized messages into a local SQLite ledger
- the dashboard reads daily aggregates from the ledger, not from raw files in the TUI layer
- the usage TUI is organized around `Overview`, `Models`, `Daily`, `Activity`, `Quota`, and `Settings`
- CLI usage output includes daily, weekly, and monthly summaries
- pricing snapshots are stored per day/model so historical cost does not silently drift
- quota view shows up to the top 4 windows per provider in Overview tab; all windows in per-provider detail tabs
- each quota gauge shows an expected-progress marker (`▏`) and ETA to limit
- activity heatmap uses solid colored cells scaled to value intensity, with a theme-invariant background/border

Known gaps:

- durable scan-state persistence for append-only sources is not finished
- Gemini CLI historical coverage still needs more sample validation beyond the current JSON/JSONL parser fixes
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
│   │   ├── pi.rs             # PI parser
│   │   ├── antigravity.rs    # Antigravity parser
│   │   └── utils.rs          # model/provider normalization helpers
│       ├── quota/
│       │   ├── mod.rs
│       │   ├── claude.rs
│       │   ├── codex.rs
│       │   ├── copilot.rs
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
# Unified dashboard - interactive TUI on a terminal
tokenpulse

# Formatting and output mode options
tokenpulse --no-tui                       # plain-text summary of both quota and usage
tokenpulse --json                         # JSON summary for scripts
tokenpulse --csv daily                    # CSV daily usage table
tokenpulse --csv models                   # CSV models usage table

# Filters
tokenpulse --since 2026-03-01             # filter by date

# Ingest and rebuild options
tokenpulse --refresh-days 2026-03-01:2026-03-07
tokenpulse --refresh-pricing
tokenpulse --rebuild-all
tokenpulse --log                          # write a timestamped startup timing log
```

---

## TUI Dashboard Design

### Quota Tab

The quota view has two modes:
- **Overview tab** shows up to the top 4 windows per provider for a compact summary
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

### Dashboard TUI Layout

```
╭─────────────────────────────────────────────────────────────────────╮
│                   📊 TokenPulse - TUI Dashboard                     │
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

  Tab: [Overview] [Models] [Daily] [Activity] [Quota] [Settings]
  Press q to quit │ ←/→ switch tabs │ ↑/↓ move selected row/day
```

The per-tab behaviour, columns, key bindings, and Settings options are
documented authoritatively in [`modules/tui.md`](modules/tui.md); this section
only sketches the conceptual layout.

**Company vs Agent Distinction:**
- **Company color** = model family owner (`OpenAI`, `Google`, `Anthropic`, `Others`)
- **Agent** = client tool (`Claude Code`, `Codex`, `OpenCode`, `Gemini CLI`, `Copilot CLI`, `Pi`, `Antigravity`)

The TUI uses company color for model names and chart segments, while agent/source labels remain textual attribution. In data model terms, `UnifiedMessage.client` = agent and `UnifiedMessage.provider_id` = provider/backend identifier. `UnifiedMessage.client_detail` is optional sub-client attribution; Antigravity uses it to keep CLI/Desktop cache entries separate while displaying and aggregating them as one `antigravity` agent.

---

## Data Models

The shared data contracts (`QuotaSnapshot`, `RateWindow`, `CreditInfo`,
`TokenBreakdown`, `UnifiedMessage`, `ModelSummary`, …) live in
[`tokenpulse-core/src/provider.rs`](../tokenpulse-core/src/provider.rs) and
`tokenpulse-core/src/usage/mod.rs`. Those definitions are the source of truth —
read them there rather than maintaining a second copy here.

---

## API Details

> Per-provider endpoints, auth, and response mapping are documented
> authoritatively in [`modules/quota.md`](modules/quota.md) (quota) and
> [`modules/usage.md`](modules/usage.md) (session parsing). The notes below are a
> quick reference.

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
| Gemini CLI | `~/.gemini/tmp/**/session-*.json{,l}` | JSON + streamed JSONL session files |
| Antigravity | `~/.local/share/tokenpulse/antigravity-cache/cache.db` | SQLite |

Normal incremental scans parse changed file-backed session files in parallel and replace rows for the changed sessions. OpenCode is kept on direct SQLite timestamp filtering, and Antigravity keeps the dedicated LS-backed cache because those sources already expose stronger incremental boundaries than file mtime alone.

### Pricing Source

```
GET https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
Cache: ~/.local/share/tokenpulse/pricing.json (24h TTL)
```

---

## Implementation Phases

### Phase 1 - MVP
- [x] Cargo workspace setup
- [x] Claude Code: auth + quota fetching
- [x] Codex: auth + quota fetching
- [x] Quota TUI gauge display
- [x] Claude Code: session JSONL parser
- [x] Codex: session JSONL parser
- [x] Pricing module (LiteLLM fetch + cache)
- [x] Usage TUI dashboard

### Phase 2 - More Providers
- [x] OpenCode: SQLite session parser
- [x] PI: session JSONL parser
- [x] Gemini CLI: historical session parser with JSONL dedup + cache-overlap normalization
- [x] GitHub Copilot: quota + usage parser
- [x] Antigravity: quota probe
- [x] Antigravity: historical usage parser

### Phase 3 - Polish
- [x] More TUI tabs: Overview, Models, Daily, Activity
- [x] Color theming
- [x] Usage `--json` export mode
- [x] Overview token/cost chart toggle and scrollable top models table
- [x] Models quick filter (`/`)
- [x] Daily token trend column
- [x] Source filter overlay (`s`)
- [x] Solid-cell heatmap intensity
- [x] Pace ETA and expected-progress marker on quota gauges
- [x] Configurable TUI theme
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
    fn incremental_ingest_mode(&self) -> IncrementalIngestMode;
}
```

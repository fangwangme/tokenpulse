# TokenPulse - Design Document v1

## Overview

A Go CLI tool with two core features:
1. **Quota** - On-demand check of remaining usage quota for coding agents
2. **Usage** - Ledger-backed historical usage dashboard with cost estimation

**Current Usage Scope:** Claude Code, Codex, OpenCode, Gemini CLI (historical only), PI, Copilot CLI, Antigravity
**Current Quota Scope:** Claude Code, Codex, Antigravity
**Maturity Note:** Historical usage is strongest today for Claude Code, Codex, and OpenCode. Gemini CLI has been deprecated and is retained for historical data analytics only.

**Language:** Go
**Key Principle:** On-demand by default, with optional auto-refresh. Run command → see results → exit. Configure a single auto-refresh interval (default 5m, applies to both quota and usage) in the CLI or live in the TUI Settings tab.

---

## Current State

As of 0.7.0:

- usage parsing writes normalized messages into a local SQLite ledger
- the dashboard reads the same report queries as the phone app, not raw files
- the TUI is organized around `Usage`, `Quota`, `Keeper`, and `Settings`
- `tokenpulse serve` can own scanning, quota refreshes and Keeper pings while
  the TUI and phone app read its data
- price periods are stored per model, and stored costs move only through
  `tokenpulse reprice --apply`
- quota view shows up to the top 4 windows per provider in Overview tab; all windows in per-provider detail tabs
- each quota window renders a progress bar (with an expected-progress marker `▏`) and, when height allows, a detail line below it with the reset countdown, used/remaining percentages, and an at-current-rate pace indicator
- activity heatmap uses solid colored cells scaled to value intensity, with a theme-invariant background/border

Known gaps:

- durable scan-state persistence for append-only sources is not finished
- Gemini CLI historical coverage still needs more sample validation beyond the current JSON/JSONL parser fixes
- weekly/monthly session counts should not yet be treated as fully deduplicated unique-session metrics

---

## Project Structure

```
tokenpulse/
├── main.go                       # CLI entrypoint
├── internal/
│   ├── cli/                      # commands and text/JSON/CSV output
│   ├── config/                   # config.toml model and persistence
│   ├── keeper/                   # scheduled agent pings and trigger evaluation
│   ├── pricing/                  # catalog, price periods and reprice
│   ├── provider/                 # shared data contracts
│   ├── quota/                    # read-only quota fetching and cache
│   ├── remote/                   # serve, auth, API, Tailscale, push, local client
│   ├── report/                   # shared usage report queries
│   ├── scan/                     # refresh orchestration
│   ├── service/                  # systemd/launchd install
│   ├── tui/                      # Bubble Tea dashboard
│   ├── usage/                    # parsers and SQLite ledger
│   └── version/                  # the single version constant
├── web/                          # Solid + TypeScript + Vite phone app, embedded at build time
├── testdata/parity/              # synthetic fixtures and Rust 0.6.16 goldens
└── docs/
    ├── DESIGN.md                 # this file
    └── specs/                    # binding behavior
```

---

## Dependencies

| Module | Purpose |
|---|---|
| `net/http` + `gorilla/websocket` | quota/pricing HTTP and the remote event stream |
| `modernc.org/sqlite` | pure-Go SQLite, no cgo |
| `charmbracelet/bubbletea` + `lipgloss` | TUI framework |
| `BurntSushi/toml` | `config.toml` |
| `SherClockHolmes/webpush-go` | Web Push |
| `skip2/go-qrcode` | pairing QR code |
| `web/` (Solid + TypeScript + Vite) | mobile PWA embedded with `go:embed` |

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

Each quota window renders as a bar with its detail line directly beneath it, and
a blank row separating consecutive windows, when there is vertical room; it falls
back to a single line in compact cards:
- **Top line** — the gradient progress bar with an expected-progress marker
  (`▏`) showing where theoretical usage should be at this point in time. The
  fill respects the active display mode (used vs remaining); no number or time
  is printed on the bar itself.
- **Detail line** — `<TAG>: reset <countdown>   used X%   remaining Y%   <pace>`.
  The reset countdown and used/remaining percentages are colored by the
  balance amount (green/yellow/red); the pace text keeps its own pace color and
  carries the at-current-rate ETA when behind pace ("on track" / "N% under pace"
  / "+N% pace | eta …"), and is omitted entirely once the window is exhausted.
- The bar spans the full card width; the detail line starts with a uniform tag
  derived from the window's period and model family (`5H:`, `WEEKLY:`,
  `WEEKLY · SONNET:`, `5H · GEMINI:`), padded per card so the figures align.
- Compact cards collapse to one line: tag + bar + percentage + reset countdown.
- Codex can show each available manual rate-limit reset credit with its expiry
  as separate quota card rows, ordered by earliest expiry and collapsing only
  when height is constrained.

```
╭─────────────────────────────────────────────────────────────────────╮
│                    ⚡ TokenPulse - Quota Overview                    │
╰─────────────────────────────────────────────────────────────────────╯

  ╭─ ANTIGRAVITY ───────────────────────────────────────────────────────╮
  │  Plan: Pro                                                          │
  │                                                                     │
  │  Gemini (5h)  ████████████▏░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │
  │    3h 12m   used 42%   remaining 58%   On track                     │
  │                                                                     │
  │  Gemini (7d)  █████▏░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │
  │    4d 6h    used 18%   remaining 82%   12% under pace                │
  ╰─────────────────────────────────────────────────────────────────────╯
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

  Tab: [Usage] [Quota] [Keeper] [Settings]
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
`TokenBreakdown`, `UnifiedMessage`, report summaries, …) live in
`internal/provider` and `internal/report`. Those definitions are the source of
truth — read them there rather than maintaining a second copy here.

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

Credentials on macOS (ordered candidates):
  1. Current-user Keychain item "Claude Code-credentials"
  2. Legacy service-only Keychain lookup
  3. ~/.claude/.credentials.json
Credentials on other platforms: ~/.claude/.credentials.json

Credentials are read-only: an expired login is reported and the user opens
Claude Code to refresh it. No credential file is written.
```

### Codex Quota

```
GET https://chatgpt.com/backend-api/wham/usage
Headers:
  Authorization: Bearer <token>

Credentials: ~/.config/codex/auth.json or ~/.codex/auth.json
  → tokens.access_token, tokens.refresh_token

Credentials are read-only: an expired login is reported and the user opens
Codex to refresh it.

Quota windows:
  primary_window and secondary_window are positional only. Emit each non-null
  window exactly once and derive 5h/7d or neutral labels from
  limit_window_seconds; never synthesize a missing window.
```

### Session File Locations

| Agent | Path | Format |
|---|---|---|
| Claude Code | `~/.claude/projects/**/*.jsonl` | JSONL with type=assistant, message.usage |
| Codex | `~/.codex/sessions/*.jsonl` | JSONL with model, token deltas |
| OpenCode | `${XDG_DATA_HOME:-~/.local/share}/opencode/opencode*.db` | SQLite, 2.x `session_message` + 1.x `message` |
| PI | `~/.pi/agent/sessions/**/*.jsonl` | JSONL with header + entries |
| GitHub Copilot | `~/.local/share/github-copilot/events.jsonl` | OTEL JSONL events |
| Gemini CLI | `~/.gemini/tmp/**/session-*.json{,l}` | JSON + streamed JSONL session files |
| Antigravity | `~/.gemini/antigravity*/conversations/*.db` + Language Server | SQLite/protobuf and JSON-RPC |

Normal incremental scans parse changed file-backed session files in parallel and replace rows for the changed sessions. OpenCode is kept on direct SQLite timestamp filtering, and Antigravity keeps a cache because those sources expose stronger incremental boundaries than file mtime alone.

### Pricing Source

```
models.dev plus LiteLLM; official-provider entries win over regional and
OpenRouter fallbacks. Cache: ~/.local/share/tokenpulse/pricing-cache.json
```

---

## Implementation Phases

### Phase 1 - MVP
- [x] Go module setup
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
- [x] GitHub Copilot: usage parser (quota fetcher removed in 0.6.0)
- [x] Antigravity: quota probe
- [x] Antigravity: historical usage parser

### Phase 3 - Polish
- [x] More TUI tabs: Overview, Models, Daily, Activity
- [x] Color theming
- [x] Usage `--json` export mode
- [x] Overview token/cost chart toggle and scrollable top models table
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

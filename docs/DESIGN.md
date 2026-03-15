# TokenPulse - Design Document v1

## Overview

A Rust CLI tool with two core features:
1. **Quota** - On-demand check of remaining usage quota for coding agents
2. **Usage** - Fancy TUI dashboard for session usage statistics with cost calculation

**Supported Agents (Phase 1):** Claude Code, Codex, OpenCode, PI
**Future Agents:** Antigravity, Gemini CLI

**Language:** Rust
**Key Principle:** On-demand only. No auto-refresh, no polling. Run command → see results → exit.

---

## Project Structure

```
tokenpulse/
├── Cargo.toml                    # workspace root
├── AGENTS.md
├── docs/
│   └── DESIGN.md                 # this file
│
├── crates/
│   ├── tokenpulse-core/          # library crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs       # Provider trait + registry
│   │       │
│   │       ├── auth/             # credential loading & token refresh
│   │       │   ├── mod.rs
│   │       │   ├── claude.rs     # ~/.claude/.credentials.json + Keychain
│   │       │   ├── codex.rs      # ~/.config/codex/auth.json
│   │       │   ├── opencode.rs   # (if needed for future quota)
│   │       │   └── pi.rs         # (if needed for future quota)
│   │       │
│   │       ├── quota/            # API-based quota fetching
│   │       │   ├── mod.rs        # QuotaSnapshot, RateWindow structs
│   │       │   ├── claude.rs     # GET api.anthropic.com/api/oauth/usage
│   │       │   └── codex.rs      # GET chatgpt.com/backend-api/wham/usage
│   │       │
│   │       ├── usage/            # local session file parsing
│   │       │   ├── mod.rs        # UnifiedMessage, TokenBreakdown structs
│   │       │   ├── scanner.rs    # parallel file discovery (walkdir + rayon)
│   │       │   ├── claude.rs     # JSONL parser: ~/.claude/projects/
│   │       │   ├── codex.rs      # JSONL parser: ~/.codex/sessions/
│   │       │   ├── opencode.rs   # SQLite parser: ~/.local/share/opencode/
│   │       │   └── pi.rs         # JSONL parser: ~/.pi/agent/sessions/
│   │       │
│   │       └── pricing/          # model pricing & cost calculation
│   │           ├── mod.rs        # ModelPricing, cost calculation
│   │           └── litellm.rs    # fetch & cache LiteLLM pricing data
│   │
│   └── tokenpulse-cli/           # binary crate
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── commands/
│           │   ├── mod.rs
│           │   ├── quota.rs      # `tokenpulse quota` command
│           │   └── usage.rs      # `tokenpulse usage` command
│           └── tui/
│               ├── mod.rs        # TUI app state & event loop
│               ├── theme.rs      # colors, styles, gradients
│               ├── widgets/
│               │   ├── mod.rs
│               │   ├── gauge.rs      # fancy progress bars
│               │   ├── sparkline.rs  # mini trend charts
│               │   ├── barchart.rs   # usage bar charts
│               │   └── table.rs      # styled tables
│               └── views/
│                   ├── mod.rs
│                   ├── quota.rs      # quota dashboard view
│                   └── usage.rs      # usage dashboard view
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

# Usage dashboard - fancy TUI
tokenpulse usage                          # interactive TUI dashboard
tokenpulse usage --since 2026-03-01       # filter by date
tokenpulse usage -p claude,codex          # filter by provider
tokenpulse usage --json                   # non-interactive JSON dump
```

---

## TUI Dashboard Design

### Quota View (`tokenpulse quota`)

```
╭─────────────────────────────────────────────────────────────────────╮
│                    ⚡ TokenPulse - Quota Overview                    │
╰─────────────────────────────────────────────────────────────────────╯

  ╭─ Claude Code ─────────────────────────────────────────────────────╮
  │  Plan: Pro                                                        │
  │                                                                   │
  │  Session (5h)   ████████████░░░░░░░░░░░░░░░░░░  42%  ⏳ 3h 12m  │
  │  Weekly (7d)    █████░░░░░░░░░░░░░░░░░░░░░░░░░  18%  ⏳ 4d 6h   │
  │  Sonnet         ██████████████░░░░░░░░░░░░░░░░░  48%  ⏳ 4d 6h   │
  │  Opus           ███░░░░░░░░░░░░░░░░░░░░░░░░░░░  12%  ⏳ 4d 6h   │
  │  Credits        $12.40 / $100.00                                  │
  ╰───────────────────────────────────────────────────────────────────╯

  ╭─ Codex ───────────────────────────────────────────────────────────╮
  │  Plan: Pro                                                        │
  │                                                                   │
  │  Session (5h)   ████████████████████░░░░░░░░░░░  67%  ⏳ 1h 45m  │
  │  Weekly (7d)    █████████░░░░░░░░░░░░░░░░░░░░░░  31%  ⏳ 5d 2h   │
  │  Credits        $45.20 (unlimited)                                │
  ╰───────────────────────────────────────────────────────────────────╯

  Last fetched: 2026-03-15 14:32:05 UTC
  Press q to quit │ r to refresh │ j/k to scroll
```

### Usage Dashboard (`tokenpulse usage`)

```
╭─────────────────────────────────────────────────────────────────────╮
│                   📊 TokenPulse - Usage Dashboard                    │
╰─────────────────────────────────────────────────────────────────────╯

  ╭─ Daily Cost (Last 14 Days) ───────────────────────────────────────╮
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

  Tab: [Overview] [Daily] [Models] [Sessions]
  Press q to quit │ r to refresh │ ←/→ switch tabs │ j/k scroll
```

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

### Pricing Source

```
GET https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
Cache: ~/.cache/tokenpulse/pricing.json (24h TTL)
```

---

## Implementation Phases

### Phase 1 - MVP (Current Goal)
- [ ] Cargo workspace setup
- [ ] Claude Code: auth + quota fetching
- [ ] Codex: auth + quota fetching
- [ ] `tokenpulse quota` with basic TUI gauge display
- [ ] Claude Code: session JSONL parser
- [ ] Codex: session JSONL parser
- [ ] Pricing module (LiteLLM fetch + cache)
- [ ] `tokenpulse usage` with TUI dashboard (daily cost chart + provider breakdown)

### Phase 2 - More Providers
- [ ] OpenCode: SQLite session parser
- [ ] PI: session JSONL parser
- [ ] Gemini CLI: auth + quota + session parser
- [ ] Antigravity: quota probe

### Phase 3 - Polish
- [ ] More TUI tabs: Models, Sessions detail
- [ ] Sparkline trends in quota view
- [ ] Color theming / config file
- [ ] `--json` export mode
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

# TokenPulse

TokenPulse is a Rust CLI + TUI dashboard for tracking token usage, cost, and
quota across multiple AI coding agents. It parses local session data into a
SQLite ledger and presents an interactive dashboard — or plain-text/JSON/CSV
output for scripting.

## Screenshots

| Overview | Quota |
|---|---|
| ![Overview](docs/images/overview.png) | ![Quota](docs/images/quota.png) |

| Models | Daily |
|---|---|
| ![Models](docs/images/models.png) | ![Daily](docs/images/daily.png) |

| Activity | Settings |
|---|---|
| ![Activity](docs/images/activity.png) | ![Settings](docs/images/settings.png) |

## Supported Providers

**Usage parsing** (session history):

| Provider | Source |
|---|---|
| Claude Code | `~/.claude/projects/` / `~/.claude/transcripts/` |
| Codex | `~/.codex/sessions/` |
| GitHub Copilot | `~/.local/share/github-copilot/events.jsonl` |
| OpenCode | `~/.local/share/opencode/` |
| Gemini CLI | `~/.gemini/tmp/` |
| PI | `~/.pi/agent/sessions/` |
| Antigravity | Language Server (GUI/CLI) |

**Quota fetching** (remaining rate-limits / credits):

| Provider | Status |
|---|---|
| Claude Code | ✅ |
| Codex | ✅ |
| GitHub Copilot | ✅ |
| Antigravity | ✅ |

> Notes:
> - Gemini usage is retained for historical analytics; the parser handles
>   streamed JSONL deduplication and cache-inclusive input tokens.
> - Antigravity usage syncs with running Language Servers (GUI/CLI) and
>   maintains a local raw cache under
>   `~/.local/share/tokenpulse/antigravity-cache/`.

## Features

- **Unified dashboard** — usage (Overview/Models/Daily/Activity) and quota in
  one TUI, or plain-text/JSON/CSV output
- **Ledger-backed** — local SQLite with per-day pricing snapshots so historical
  cost does not silently drift
- **Quota overview** — up to the top 4 windows per provider with pace ETA and
  expected-progress marker; per-provider detail in the Quota tab
- **Auto-refresh in TUI** — configurable intervals for quota view (1/2/5/10/15
  min, default 5 min) and usage view (5/10/15/30 min, default 10 min); cycle
  or toggle in the Settings tab, countdown shown in footer
- **`r` to refresh** — in both quota and usage views without restarting TUI
- **60-day stacked bar chart** — switchable between token and cost views
- **Heatmap** — solid-cell coloring for value levels; GitHub-green palette for
  cost, Kaggle-blue palette for tokens; five intensity levels at 20/40/60/80%
  of the visible window peak; theme-invariant backgrounds and borders
- **Activity heatmap** — mouse-selectable with clickable legend ranges,
  agent/model drill-down, and scrollable selected-day detail
- **Models table** — `%` column showing share of the active sort metric; quick
  filter (`/`) and source filter overlay (`s`)
- **Company-aware model coloring** — agent / provider separation in the data
  model
- **Plain-text mode** — for scripting and remote shells
- **JSON output** — usage summary + quota snapshots in one payload
- **CSV output** — per-model or daily breakdown for further analysis

## Install

Requirements:

- Rust toolchain (1.75+)
- Local agent / session data on the same machine

```bash
cargo install tokenpulse
```

Or build from source:

```bash
git clone https://github.com/fangwangme/tokenpulse
cd tokenpulse
cargo install --path tokenpulse-cli
```

Or build the workspace manually:

```bash
cargo build --release --workspace
```

## Quick Start

### First time setup

```bash
# Interactive setup wizard — detects installed providers and guides configuration
tokenpulse init

# Auto-detect and enable found providers, skip prompts
tokenpulse init --default
```

### Run the dashboard

```bash
# Interactive TUI (auto-detects terminal)
tokenpulse

# Plain text output (scripting / remote shells)
tokenpulse --no-tui

# JSON output (usage + quota in one payload)
tokenpulse --json

# CSV output
tokenpulse --csv daily
tokenpulse --csv models
```

### Filters

```bash
# Show data starting from a specific date
tokenpulse --since 2026-04-01

# Re-parse a specific date range
tokenpulse --refresh-days 2026-04-01:2026-04-09

# Force a fresh pricing fetch (otherwise uses cached pricing)
tokenpulse --refresh-pricing

# Full rebuild from scratch
tokenpulse --rebuild-all

# Write timing diagnostics to a log file
tokenpulse --log
```

### Provider setup

```bash
# Show current config
tokenpulse config show

# Enable / disable providers
tokenpulse config enable claude
tokenpulse config disable gemini

# Quota display mode: "remaining" (default) or "used"
tokenpulse config set quota_display_mode=used

# Theme: auto, dark, light
tokenpulse config set theme=dark

# Auto-refresh interval in minutes for quota + usage (0 = disabled)
tokenpulse config set auto_refresh_interval=5

# Show empty providers in the dashboard
tokenpulse config set show_empty_providers=true

# Show / hide account name in quota cards
tokenpulse config set show_account=true
```

## Data Model

TokenPulse tracks two distinct concepts:

- **Agent** — the client tool you used (e.g., `Claude Code`, `Codex`,
  `GitHub Copilot`, `Gemini CLI`, `PI`, `Antigravity`)
- **Provider** — the backend / model company (e.g., `Anthropic`, `OpenAI`,
  `Google`, `GitHub Copilot`)

The dashboard keeps these separate so the same model family can be attributed
across multiple agents.

## Local Storage

All local state lives under a single directory:

```
~/.local/share/tokenpulse/
├── config.toml        # user configuration
├── usage.db           # SQLite usage ledger
├── pricing.json       # cached model pricing
├── antigravity-cache/ # raw Antigravity session cache
└── log/               # performance diagnostics
```

## Project Structure

```
tokenpulse/
├── Cargo.toml                  # workspace root
├── tokenpulse-core/            # library: parsing, pricing, quota, ledger
│   ├── Cargo.toml
│   └── src/
│       ├── auth/               # provider credential detection
│       ├── config/             # config model & persistence
│       ├── pricing/            # model pricing (LiteLLM, OpenRouter, Models.dev)
│       ├── quota/              # live quota fetchers
│       └── usage/              # session parsers & SQLite store
├── tokenpulse-cli/             # binary: CLI entrypoints & TUI
│   ├── Cargo.toml
│   └── src/
│       ├── commands/           # init, config, usage, quota registry
│       ├── tui/                # ratatui dashboard
│       │   ├── views/usage/    # Overview, Models, Daily, Activity, Quota, Settings
│       │   └── widgets/        # barchart, heatmap, gauge, trend
│       └── main.rs
└── docs/
    └── DESIGN.md               # design document & architecture notes
```

## Development

```bash
cargo fmt --all
cargo test --workspace
```

> **Performance Profiling**
>
> When testing startup latency, file parsing speed, or TUI performance, always
> build and run with `--release` (e.g., `cargo run --release`). The unoptimized
> debug build (`dev` profile) runs mtime stat checks, JSON parsing, and SQLite
> queries up to 5× slower, which can distort measurements.

Primary design notes live in [docs/DESIGN.md](docs/DESIGN.md).

## Acknowledgments

TokenPulse was inspired by and builds on ideas from these projects:

- **[CodexBar](https://github.com/steipete/CodexBar)** by [@steipete](https://github.com/steipete) —
  macOS menu bar app for real-time quota visibility. The auth flows and quota
  API patterns for Codex, Copilot, Gemini, and Antigravity were informed by
  its open-source implementation.

- **[tokscale](https://github.com/junhoyeo/tokscale)** by [@junhoyeo](https://github.com/junhoyeo) —
  Rust CLI + TUI for token usage and costs across multiple AI coding agents.
  Inspired the multi-tab dashboard layout and multi-agent attribution design.

- **[openusage](https://github.com/robinebers/openusage)** by [@robinebers](https://github.com/robinebers) —
  macOS menu bar for AI subscription usage tracking. Inspired the breadth of
  provider coverage and the plugin-based approach to adding new quota sources.

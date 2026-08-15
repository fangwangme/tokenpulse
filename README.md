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

| Keeper | Activity |
|---|---|
| ![Keeper](docs/images/keeper.png) | ![Activity](docs/images/activity.png) |

| Settings | |
|---|---|
| ![Settings](docs/images/settings.png) | |

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

- **Unified dashboard** — usage (Overview/Models/Daily/Keeper/Activity) and quota in
  one TUI, or plain-text/JSON/CSV output
- **Session Keeper & Automated Heartbeats** — scheduled lightweight heartbeats for
  Claude Code, Codex, and Google Antigravity to trigger 5h cooldown timers early
  and auto-sync immediately after weekly quota resets. Off by default; each ping
  spends real quota, so you opt in per agent
- **Live Execution Stream** — real-time heartbeat log panel with executed commands,
  prompts, replies, durations, status codes, and smooth mouse-wheel scrolling
- **Ledger-backed** — local SQLite with per-day pricing snapshots so historical
  cost does not silently drift
- **Quota overview** — up to the top 4 windows per provider; each is a progress
  bar with an expected-progress marker plus a detail line showing the reset
  countdown, used/remaining percentages, and an at-current-rate pace indicator
- **Auto-refresh in TUI** — one interval for both quota and usage
  (0/1/2/5/10/15 min, default 5 min); cycle or toggle in the Settings tab, with
  a countdown shown in the footer
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
- **Observation history** — every quota poll (per provider, rate window, and
  model family) and every Keeper run is appended to a local SQLite database for
  your own trend analysis

## Install

TokenPulse reads agent session data from the local filesystem, so install it on
the same machine the agents run on.

### npm (prebuilt binary, no Rust toolchain needed)

```bash
npm install -g @fangwangme/tokenpulse
tokenpulse
```

Or run it without installing:

```bash
npx @fangwangme/tokenpulse
```

Prebuilt binaries are published for macOS (Intel and Apple Silicon) and Linux
(x64 and arm64). npm resolves the one matching your machine; on any other
platform, build from source instead.

### From source

Requires a Rust toolchain (1.75+):

```bash
cargo install --git https://github.com/fangwangme/tokenpulse tokenpulse-cli
```

Or clone and build:

```bash
git clone https://github.com/fangwangme/tokenpulse
cd tokenpulse
cargo install --path tokenpulse-cli
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
```

> [!IMPORTANT]
> **Antigravity Sync after Rebuild**: If you rebuild the database (e.g. via `--rebuild-all`), you must run the Antigravity CLI and Desktop concurrently once to allow full synchronization of usage history. This is required because they need to be active at the same time to sync and align the data.

```bash

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
# "auto" follows the OS appearance and re-checks on each refresh, so the TUI
# switches light/dark when you change the system setting while it's running (macOS).
tokenpulse config set theme=dark

# Auto-refresh interval in minutes for quota + usage (0 = disabled)
tokenpulse config set auto_refresh_interval=5

# Show empty providers in the dashboard
tokenpulse config set show_empty_providers=true

# Show / hide account name in quota cards
tokenpulse config set show_account=true

# Enable / disable quota refresh (default: true)
tokenpulse config set refresh_quota=false

# Quota recovery alerts: how far the notification reaches
#   off       nothing at all
#   in_app    sound + the in-TUI toast and ambient pulse
#   terminal  ... plus a terminal bell and an OSC 9 desktop notification
#   system    ... plus a macOS Notification Center banner
tokenpulse config set notification_level=system

# The sound every level except `off` plays. Either `chime` (built in),
# `none` to stay silent, or any name under /System/Library/Sounds.
tokenpulse config set notification_sound=chime

# Fire a sample alert to check the whole chain without waiting for a real reset
tokenpulse config test-notification
```

> **Note on audibility.** The sound goes through `afplay` rather than the
> terminal bell, because most terminals ship with the audible bell disabled
> (Ghostty, for example, defaults to `bell-features = no-system,no-audio`). The
> built-in chime is normalised to roughly 11 dB above macOS `Ping.aiff` so it is
> hard to miss over headphones. If `notification_level=system` shows no banner,
> allow your terminal to post notifications in System Settings > Notifications;
> the sound plays regardless.

### Session Keeper (Heartbeats & Wakeup)

The **Keeper** tab lets you manage scheduled pings and wakeups for Claude Code, Codex, and Google Antigravity to keep sessions warm and trigger cooldown cycles at designated times.

Every ping spends real quota, so the engine ships **disabled**. Turn it on with
the `keeper_engine` row in the Settings tab, then enable the switches you want
per agent. Wakeup times, models, prompts, and commands live in `config.toml`
under `[keeper]`; the tab header shows the exact path.

- **`←` / `→` (or `h` / `l`)** — Switch between dashboard tabs
- **`↑` / `↓` (or `k` / `j` / `Tab`)** — Select agent card (Claude Code / Codex / Antigravity)
- **`1` / `d`** — Toggle 5h daily morning wakeup timer (default `10:30`; a missed run is only caught up within 2 hours of it)
- **`2` / `w`** — Toggle weekly auto-sync (fires 1 min after quota resets)
- **`p`** — Immediately test ping the selected agent
- **Mouse Wheel** — Scroll the live execution and heartbeat history stream

The panel shows the newest 50 runs and is restored on launch; the full history
is kept in `tokenpulse.db`.

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
├── keeper_state.json  # Keeper last-fired bookkeeping
├── usage.db           # SQLite usage ledger
├── tokenpulse.db      # quota cache + quota/Keeper observation history
├── pricing.json       # cached model pricing
├── antigravity-cache/ # raw Antigravity session cache
└── log/               # daily rotating diagnostics log
```

`tokenpulse.db` keeps an append-only record of every quota poll (per provider,
per rate window, per model family) and every Keeper run, for later analysis. It
has no retention policy — delete old rows yourself if it grows too large.

## Project Structure

```
tokenpulse/
├── Cargo.toml                  # workspace root
├── tokenpulse-core/            # library: parsing, pricing, quota, ledger
│   ├── Cargo.toml
│   └── src/
│       ├── auth/               # provider credential detection
│       ├── config/             # config model & persistence
│       ├── history/            # quota + Keeper observation history (SQLite)
│       ├── keeper/             # scheduled agent pings
│       ├── pricing/            # model pricing (LiteLLM, OpenRouter, Models.dev)
│       ├── quota/              # live quota fetchers
│       └── usage/              # session parsers & SQLite store
├── tokenpulse-cli/             # binary: CLI entrypoints & TUI
│   ├── Cargo.toml
│   └── src/
│       ├── commands/           # init, config, usage, quota registry
│       ├── tui/                # ratatui dashboard
│       │   ├── views/usage/    # Overview, Models, Daily, Activity, Quota, Keeper, Settings
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

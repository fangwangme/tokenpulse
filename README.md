# TokenPulse

TokenPulse is a Go CLI and TUI dashboard for tracking token usage, cost, and
quota across AI coding agents. It stores parsed sessions in a local SQLite
ledger and exposes the data through a terminal dashboard, a mobile web app
over Tailscale, and plain-text, JSON, or CSV output.

## Install

TokenPulse reads agent session data from the local filesystem, so install it on
the same machine the agents run on.

### Prebuilt binary (no build toolchain needed)

Install globally via npm:

```bash
npm install -g @fangwangme/tokenpulse
```

Or via bun:

```bash
bun add -g @fangwangme/tokenpulse
```

Or run directly without installing:

```bash
npx @fangwangme/tokenpulse
```

```bash
bunx @fangwangme/tokenpulse
```

Once installed, launch the dashboard:

```bash
tokenpulse
```

Prebuilt binaries are published for macOS (Intel and Apple Silicon) and Linux
(x64 and arm64). npm/bun resolves the one matching your machine; on any other
platform, build from source instead.

### From source

Requires Go 1.27+ and Node 20+ (to build the embedded phone app):

```bash
git clone https://github.com/fangwangme/tokenpulse
cd tokenpulse/web && npm ci && npm run build && cd ..
go build -o tokenpulse .
```

If you only need the CLI/TUI (without the phone web app), `go install` works
without Node and embeds a placeholder page at `/`:

```bash
go install github.com/fangwangme/tokenpulse@latest
```

## Supported Providers

**Usage parsing** (session history):

| Provider | Source |
|---|---|
| Claude Code | `~/.claude/projects/` / `~/.claude/transcripts/` |
| Codex | `~/.codex/sessions/` |
| GitHub Copilot | `~/.local/share/github-copilot/events.jsonl` |
| OpenCode | `${XDG_DATA_HOME:-~/.local/share}/opencode/` (1.x and 2.x storage, or `OPENCODE_DB`) |
| Gemini CLI | `~/.gemini/tmp/` |
| PI | `~/.pi/agent/sessions/` |
| Antigravity | Language Server (GUI/CLI), local conversation databases |

**Quota fetching** (remaining rate-limits / credits):

| Provider | Status |
|---|---|
| Claude Code | ✅ |
| Codex | ✅ |
| Antigravity | ✅ |

> Notes:
> - Gemini usage is retained for historical analytics; the parser handles
>   streamed JSONL deduplication and cache-inclusive input tokens.
> - Antigravity usage syncs with running Language Servers (GUI/CLI) and
>   maintains a local raw cache under
>   `~/.local/share/tokenpulse/antigravity-cache/`.

## Features

- **Unified usage report** — one Usage view shared by the TUI and phone app,
  with range presets, period comparison, a Token/Cost switch, heatmaps,
  date/model lists, day/model detail, and an agent filter
- **Remote phone mode** — `tokenpulse serve` owns scanning and quota refreshes,
  serves a mobile PWA over Tailscale Serve HTTPS, and syncs live with the TUI
  over a local Unix socket
- **Read-only quota tracking** — checks Claude Code, Codex, and Antigravity
  quota without writing or refreshing credentials; shows remaining/used bars,
  reset times, pace, extra usage, and reset credits (off by default; enable it
  in Settings or with `tokenpulse config set refresh_quota=true`)
- **Session Keeper** — optional scheduled heartbeats for Claude Code, Codex,
  and Antigravity to trigger 5h cooldowns early and wake weekly windows after
  resets (disabled by default; each ping spends real quota)
- **Ledger & price periods** — local SQLite ledger with official-provider-first
  pricing and dated price periods; stored costs stay untouched unless you run
  `tokenpulse reprice --apply`
- **Auto-refresh & on-demand reload** — configurable refresh interval
  (default 15 min) plus `r` in any TUI tab to refresh immediately
- **Scripting outputs** — `--no-tui` plain text, `--json` (usage + quota),
  and `--csv daily|models`
- **Observation history** — every quota poll and Keeper run is appended to
  `tokenpulse.db` for trend analysis

## Quick Start

### First time setup

```bash
# Detect installed providers and enable the ones found
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

### TUI keys

- `Tab` / `Shift+Tab` — switch between Usage, Quota, Keeper, and Settings; `q` / `Ctrl+C` quits.
- Usage: `1`–`6` presets (`today`, `7d`, `30d`, `year`, `all`, `custom`), `m` toggles
  Token/Cost, `v` toggles dates/models list, `f` switches focus between heatmap
  and list, `Enter` opens detail, `[` / `]` shifts the range, `s` opens the agent
  filter, `r` refreshes.
- Quota: `↑` / `↓` selects a provider.
- Keeper: `e` toggles the engine, `Enter`/`Space` toggles the 5h keeper,
  `w` toggles the weekly keeper, `p` sends a test ping.
- Settings: `↑` / `↓` moves, `Enter`/`Space` changes the row; the quota-provider
  row cycles Claude → Codex → Antigravity.
- `b` cycles the theme; `Esc` closes an overlay without quitting.

### Filters & maintenance

```bash
# Read the cached ledger without scanning
tokenpulse --no-scan

# Show data starting from a specific date
tokenpulse --since 2026-04-01

# Re-parse a specific date range
tokenpulse --refresh-days 2026-04-01:2026-04-09

# Force a fresh pricing fetch (otherwise uses cached pricing)
tokenpulse --refresh-pricing

# Preview or apply repricing under current price rules
tokenpulse reprice

# Full rebuild from scratch
tokenpulse --rebuild-all

# Write timing diagnostics to a log file
tokenpulse --log
```

> [!IMPORTANT]
> **Antigravity Sync after Rebuild**: After running `--rebuild-all`, launch the
> Antigravity CLI and Desktop together once so both can sync and align history.

### Provider setup

```bash
# Show current config
tokenpulse config show

# Enable / disable quota providers (claude, codex, antigravity)
tokenpulse config enable claude
tokenpulse config disable antigravity

# Quota display mode: "remaining" (default) or "used"
tokenpulse config set quota_display_mode=used

# Theme: auto, dark, light
# "auto" follows the OS appearance and re-checks on each refresh, so the TUI
# switches light/dark when you change the system setting while it's running (macOS).
tokenpulse config set theme=dark

# Auto-refresh interval in minutes for quota + usage (0, 1, 2, 5, 10, 15; 0 = disabled)
tokenpulse config set auto_refresh_interval=15

# Show empty providers in the dashboard
tokenpulse config set show_empty_providers=true

# Account row in quota cards: none, plan, or full (plan + email; default)
tokenpulse config set account_display=plan

# Enable / disable quota refresh (default: false)
tokenpulse config set refresh_quota=true

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

If `config.toml` has an error, the commands and Settings rows that change it
leave it untouched and report the error instead of rewriting it from defaults;
fix or remove the file, then retry.

### Session Keeper (Heartbeats & Wakeup)

The **Keeper** tab manages scheduled pings for Claude Code, Codex, and
Antigravity to start 5h cooldown cycles early and wake weekly windows after a
reset.

Because every ping spends real quota, the engine ships **disabled**. Enable it
in the Keeper tab (`e`) or Settings tab, then opt in per agent. Wakeup times,
prompts, and command templates live under `[keeper]` in `config.toml`; Keeper
picks the cheapest active model per agent from local catalogs at startup and
once a day. The Keeper tab shows the newest 20 runs; full history stays in
`tokenpulse.db`.

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
├── usage.db           # SQLite usage ledger + price periods
├── tokenpulse.db      # quota cache + quota/Keeper observation history
├── pricing-cache.json # cached model pricing catalog
├── antigravity-cache/ # raw Antigravity session cache and learned aliases
├── remote.json        # remote token and hashed phone sessions
├── serve.sock         # local control socket while serve runs
├── vapid.json         # Web Push key pair
└── log/               # rotating diagnostics log
```

Set `TOKENPULSE_DATA_DIR` to move this directory, or
`TOKENPULSE_CONFIG_PATH` to override only the config file. The directory is
created private (`0700`). `TOKENPULSE_ENV=test` marks a test run and requires
`TOKENPULSE_DATA_DIR`.

`tokenpulse.db` keeps an append-only record of every quota poll (per provider,
per rate window, per model family) and every Keeper run, for later analysis. It
has no retention policy — delete old rows yourself if it grows too large.

## Remote mode

Run TokenPulse on the computer that holds the agent sessions:

```bash
tokenpulse serve
```

`serve` prints a pairing URL and QR code. The URL carries the token in its
fragment, so it never reaches a request line or log. Open it on a phone on the
same tailnet; the web app can be added to the home screen. Default ports are
`4120` (local HTTP) and `8444` (Tailscale Serve HTTPS); a test run uses
`14120`/`18444`.

- `--no-tailscale` binds localhost only; `--lan` binds all interfaces and warns
  that LAN traffic is not encrypted.
- `tokenpulse remote url` prints the pairing URL again; `tokenpulse remote
  reset-token` invalidates every phone session.
- `tokenpulse service install` installs a systemd user unit on Linux (or a
  launchd agent on macOS) that runs `serve` and restarts it on failure.
- One runtime owns refreshing behind `runtime.lock`. You can also toggle
  **Web remote** directly from the TUI Settings tab to start or stop a
  background `serve` without leaving the dashboard.

## Project Structure

```
tokenpulse/
├── main.go                  # CLI entrypoint
├── internal/
│   ├── cli/                 # commands and output
│   ├── config/              # config.toml model and persistence
│   ├── keeper/              # scheduled agent pings
│   ├── pricing/             # catalog, price periods, reprice
│   ├── quota/               # read-only quota fetchers and cache
│   ├── remote/              # serve, auth, API, Tailscale, push, local client
│   ├── report/              # shared usage report queries
│   ├── scan/                # refresh orchestration
│   ├── service/             # systemd/launchd install
│   ├── tui/                 # Bubble Tea dashboard
│   ├── usage/               # session parsers and SQLite store
│   └── ...
├── web/                     # Solid + TypeScript + Vite phone app, embedded at build time
├── testdata/parity/         # synthetic parser and quota fixtures + goldens
└── docs/
    ├── specs/               # binding behavior
    └── DESIGN.md            # design notes
```

## Development

```bash
gofmt -w .
go vet ./...
go test ./...

# Phone app (Node 20+); the Go binary embeds web/dist
cd web && npm ci && npm run build
```

A plain `go build` needs no Node: `web/dist/index.html` is a committed
placeholder until the web build replaces it.

Primary design notes live in [docs/DESIGN.md](docs/DESIGN.md) and the binding
behavior is in [docs/specs](docs/specs).

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

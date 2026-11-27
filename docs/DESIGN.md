# TokenPulse - Design Document

## Overview

TokenPulse is a single Go binary that tracks AI coding agent usage, checks
subscription quota, optionally runs Session Keeper pings, and exposes the data
through a TUI, CLI, and mobile web app.

- **Quota** — on-demand, read-only quota checks for Claude Code, Codex,
  OpenCode Go, and Antigravity.
- **Usage** — a local SQLite ledger of session usage, priced from an
  official-provider-first catalog and shared across the TUI, CLI, and phone app.
- **Remote mode** — `tokenpulse serve` owns scanning, quota refreshes, and
  Keeper execution while serving the embedded mobile PWA and syncing with the
  TUI over a local Unix socket.

Supported usage sources: Claude Code, Codex, OpenCode, Gemini CLI (historical),
Pi, GitHub Copilot, and Antigravity.

The binary is cgo-free and statically linked. The TUI uses Bubble Tea; the
phone app is Solid + TypeScript + Vite and is embedded at build time.
Authoritative behavior lives in `docs/specs/`; this document summarizes the
architecture.

---

## Current architecture

```
agent session files / APIs
        │
        ├── internal/usage   parsers + SQLite ledger (usage.db)
        ├── internal/quota   read-only fetchers + cache (tokenpulse.db)
        └── internal/pricing catalog, price periods, cost lookup
                    │
             internal/scan   one refresh: price, parse, ingest, rebuild
                    │
        ┌───────────┼────────────┐
        │           │            │
 internal/report  internal/cli  internal/keeper
        │                        │
        └─────────┬──────────────┘
           internal/runtime   single writer, flock, refresh loop, event bus
                  │
        ┌─────────┴──────────┐
   internal/tui       internal/remote
  (thin client)   (HTTP/WS + embedded web/)
```

- `internal/usage` writes `usage.db`: normalized messages and daily rollups.
  A scan replaces the rows it parsed, preserves stored costs, and rebuilds the
  affected days in one transaction. Parser versions and the one-time Codex
  repair are documented in `usage-sources.md` and `go-rewrite.md`.
- `internal/quota` only reads provider credentials. Snapshots and observation
  history live in `tokenpulse.db`; credentials are never written or refreshed.
- `internal/pricing` selects one price per model from the merged catalog,
  stores dated price periods, and computes message costs. Stored costs move
  only through `tokenpulse reprice` (`pricing.md`).
- `internal/scan` prices the catalog, parses the enabled sources, ingests the
  messages and rebuilds the rollups.
- `internal/report` is the one usage report used by the TUI, the CLI and the
  phone (`usage-report.md`).
- `internal/runtime` is the single logical writer and scheduler for a data
  directory, guarded by an OS `flock` (`runtime.lock`). It owns scanning,
  quota refreshes, the 60-second manual cooldown, Keeper scheduling, and an
  event bus. Both `internal/tui` and `internal/remote` consume it through the
  same `runtime.Client` / `*runtime.Runtime` boundary.
- `internal/remote` is a thin transport layer over `internal/runtime` that
  serves the PWA, JSON API, WebSocket, pairing, settings allowlist, Tailscale
  resolution, Web Push, and the local Unix-socket `SocketClient` (`remote.md`).
- `internal/keeper` schedules the optional agent pings and records them in
  `tokenpulse.db`.
- `internal/service` installs `serve` as a systemd user unit or launchd agent.
- `internal/tui` is a thin Bubble Tea frontend over `runtime.Client`
  (`InProcessClient` locally, or `SocketClient` attached to a background
  `serve`), including a `Web remote` toggle in Settings (`tui-usage.md`).
- `internal/cli` implements every command and the text/JSON/CSV output
  (`cli.md`).

### Startup and refresh

`tokenpulse` reads the ledger first. If `serve` is running, it reuses the
serve-owned scan and cache; otherwise it scans in the background and shows the
TUI immediately. `--no-scan` always reads the cached ledger. Read-only commands
open SQLite with `mode=ro`, so a concurrent scan cannot produce
`SQLITE_BUSY`. `serve` warms its caches, then refreshes usage and quota on the
configured interval.

### Data model

`internal/provider` defines `QuotaSnapshot`, `RateWindow`, `CreditInfo`,
`TokenBreakdown` and `UnifiedMessage`; `internal/report` defines the report
types. An **agent** is the client tool (`claude`, `codex`, `antigravity`, …),
a **provider** is the backend/company, and `client_detail` keeps runtime
variants such as Antigravity CLI/Desktop separate while reports aggregate them
as one agent.

---

## Project structure

```
tokenpulse/
├── main.go                       # CLI entrypoint
├── internal/
│   ├── cli/                      # commands and text/JSON/CSV output
│   ├── config/                   # config.toml model and persistence
│   ├── format/                   # shared money/number display
│   ├── keeper/                   # scheduled agent pings and history
│   ├── modelid/                  # canonical model ids and companies
│   ├── notification/             # quota-recovery alerts
│   ├── paths/                    # data-directory resolution
│   ├── pricing/                  # catalog, price periods and reprice
│   ├── provider/                 # shared data contracts
│   ├── quota/                    # read-only fetchers and cache
│   ├── remote/                   # serve, auth, API, Tailscale, push, client
│   ├── report/                   # shared usage report queries
│   ├── runtime/                  # single-writer runtime, flock, event bus, Client
│   ├── scan/                     # refresh orchestration
│   ├── service/                  # systemd/launchd install
│   ├── summary/                  # 0.6-compatible JSON/text summary
│   ├── tui/                      # Bubble Tea dashboard
│   ├── usage/                    # parsers and SQLite ledger
│   └── version/                  # the single version constant
├── web/                          # Solid + TypeScript + Vite phone app
├── npm/                          # launcher and package assembly
├── testdata/parity/              # synthetic fixtures and 0.6.16 goldens
└── docs/specs/                   # binding behaviour
```

---

## CLI interface

```bash
tokenpulse                                  # TUI on a terminal, text otherwise
tokenpulse --no-tui                         # plain-text report
tokenpulse --json                           # JSON summary + quota
tokenpulse --csv daily|models               # CSV tables
tokenpulse --raw --date 2026-11-20          # raw model rows
tokenpulse --since 2026-03-01
tokenpulse --refresh-days 2026-03-01:2026-03-07
tokenpulse --refresh-pricing | --rebuild-all | --no-scan | --log | --debug
tokenpulse init --default
tokenpulse config show|enable|disable|set|test-notification
tokenpulse reprice [--apply --yes --expect <fingerprint>] [--json]
tokenpulse serve [--port N] [--https-port N] [--lan] [--no-tailscale] [--web-dir DIR]
tokenpulse remote reset-token|url|status
tokenpulse service install|uninstall|status
```

See `cli.md`, `config.md`, `pricing.md` and `remote.md` for the full rules.

---

## TUI

The dashboard restores the 0.6.16 seven-tab layout over `runtime.Client`:
`Overview`, `Models`, `Daily`, `Activity`, `Quota`, `Keeper`, `Settings`.

- **Overview / Models / Daily**: share one range (`1`–`6` presets, default
  `30d`, `[`/`]` shift, `n` today) and the global agent filter (`s`). Overview
  shows KPIs, a stacked company bar chart (`t`/`c`), and Top Models; Models and
  Daily are sortable tables (`c`/`t`/`d`) with detail popups (`Enter`), and
  Daily adds the vs-7-days column (`v`).
- **Activity**: fixed one-year Monday-start heatmap (`t`/`c`), period overview,
  and selected-day breakdown (`↑`/`↓`, `n`, `[`/`]`).
- **Quota**: multi-column provider cards in `display.quota_provider_order` with
  gauges, pace, reset times, extra usage, and banked resets.
- **Keeper**: engine toggle (`e`), agent cards (`↑`/`↓`/`Tab`, `1`/`d` 5h,
  `2`/`w` weekly, `p` manual ping), and live execution stream.
- **Settings**: `↑`/`↓` moves, `Enter`/`Space` changes the selected row,
  `J`/`K` reorders quota providers, plus the `Web remote` toggle.

See [`specs/tui-usage.md`](specs/tui-usage.md) and
[`modules/tui.md`](modules/tui.md) for full details. A running `serve` owns
scanning and Keeper execution; the standalone TUI runs the runtime in-process.
The phone and TUI share the same report queries and service status.

---

## Phone app

`serve` embeds `web/dist` (or serves `--web-dir`). Pairing uses a fragment
token exchanged for a hashed session cookie. The API and WebSocket are
described in `remote.md`; the app shell, pages and design tokens are specified
there too. Web Push carries `{title, body}` for quota-recovery alerts.

---

## Dependencies

| Module | Purpose |
|---|---|
| `net/http` + `gorilla/websocket` | API, pairing and the live event stream |
| `modernc.org/sqlite` | pure-Go SQLite, no cgo |
| `charmbracelet/bubbletea` + `lipgloss` | TUI framework |
| `BurntSushi/toml` | `config.toml` |
| `SherClockHolmes/webpush-go` | Web Push |
| `skip2/go-qrcode` | pairing QR code |
| `klauspost/compress/zstd` | compressed Codex rollouts |
| `web/` (Solid + TypeScript + Vite) | mobile PWA embedded with `go:embed` |

---

## Adding a source

1. Add a parser in `internal/usage` implementing `Parser` (`Name`,
   `ParserVersion`, `SessionPaths`, `Parse`, `IncrementalMode`), plus a test.
2. Register it in the `parsers` function in `internal/scan/scan.go` and in
   `ProvidersFromConfig`.
3. Add the source name and colors to the report/TUI/web agent lists.
4. Add a synthetic fixture under `testdata/parity/` and update the golden.
5. Document the paths and edge cases in `docs/specs/usage-sources.md`.

Adding a quota provider follows the same shape in `internal/quota`, plus its
read-only credential path and `docs/specs/quota.md` entry.

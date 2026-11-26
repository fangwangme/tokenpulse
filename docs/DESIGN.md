# TokenPulse - Design Document

## Overview

TokenPulse is one Go binary with two jobs:

1. **Quota** - on-demand, read-only quota checks for Claude Code, Codex and
   Antigravity.
2. **Usage** - a local SQLite ledger of session usage, priced from a
   provider-first catalog and rendered by the TUI, the CLI and the phone app.

Supported usage sources: Claude Code, Codex, OpenCode, Gemini CLI (historical),
Pi, GitHub Copilot and Antigravity.

The program is cgo-free and statically linked. The TUI uses Bubble Tea; the
phone app is Solid + TypeScript + Vite and is embedded into the binary at
build time. A running `tokenpulse serve` can own scanning, quota refreshes and
Keeper pings while the TUI and phone read its API.

The authoritative behaviour lives in `docs/specs/`; this document describes
the architecture and points at those specs.

---

## Current architecture

```
agent session files / APIs
        │
        ├── internal/usage   parsers + SQLite ledger (usage.db)
        ├── internal/quota   read-only fetchers + cache (tokenpulse.db)
        └── internal/pricing catalog, price periods, cost lookup
                    │
             internal/scan     one refresh: price, parse, ingest, rebuild
                    │
        ┌───────────┼────────────┐
        │           │            │
   internal/report  │      internal/keeper
        │           │            │
   internal/tui  internal/cli  internal/remote
        │                        │
   internal/report         HTTP/WS + embedded web/
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
  messages and rebuilds the rollups. `serve` and the TUI call the same scanner.
- `internal/report` is the one usage report used by the TUI, the CLI and the
  phone (`usage-report.md`).
- `internal/remote` serves the PWA, the JSON API and the WebSocket, pairs
  phones, enforces the settings allowlist, fetches Tailscale state, stores push
  subscriptions and exposes a local Unix-socket client (`remote.md`).
- `internal/keeper` schedules the optional agent pings and records them in
  `tokenpulse.db`.
- `internal/service` installs `serve` as a systemd user unit or launchd agent.
- `internal/tui` is the terminal dashboard (`tui-usage.md`).
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
tokenpulse --refresh-pricing | --rebuild-all | --no-scan | --log
tokenpulse init --default
tokenpulse config show|enable|disable|set|test-notification
tokenpulse reprice [--apply --yes --expect <fingerprint>] [--json]
tokenpulse serve [--port N] [--https-port N] [--lan] [--no-tailscale]
tokenpulse remote reset-token|url|status
tokenpulse service install|uninstall|status
```

See `cli.md`, `config.md`, `pricing.md` and `remote.md` for the full rules.

---

## TUI

The dashboard has four tabs: `Usage`, `Quota`, `Keeper`, `Settings`.

- **Usage**: range presets `1-6`, a Token/Cost switch (`m`), a list/heatmap
  switch (`f`), row movement (`↑/↓` or `j/k`), detail (`Enter`), range shift
  (`[`/`]`), agent filter (`s`), refresh (`r`), theme (`b`), quit (`q`).
- **Quota**: provider cards with bars, reset time and pace; `↑/↓` moves the
  provider cursor.
- **Keeper**: per-agent switches and history; `e` toggles the engine.
- **Settings**: `↑/↓` moves, `Enter`/`Space` changes the selected row.

A running `serve` owns scanning and Keeper execution; the standalone TUI runs
the Keeper scheduler itself. The phone and TUI share the same report queries
and service status.

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

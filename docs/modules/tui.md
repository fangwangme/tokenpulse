# TUI Module

The dashboard uses Bubble Tea and Lip Gloss and draws the 0.6.16 desktop
layout on the Go runtime. Binding behavior is in
[`../specs/tui-usage.md`](../specs/tui-usage.md), with theme and table rules in
`tui-theme.md` and `tui-table-layout.md`.

## Files

- `tui.go` — model, update loop, data commands and cache keys, event
  subscription, settings patches and the Web remote toggle.
- `overview.go`, `models.go`, `daily.go`, `activity.go`, `quota.go`,
  `keeper.go`, `settings.go` — page renderers and page keys.
- `canvas.go` — a small cell buffer, borders and splits; pages place text at
  exact coordinates instead of nesting boxes.
- `theme.go` — the port of the Rust theme: company and agent colors, gauges,
  heatmap palettes and the light/dark modes.
- `format.go` and `table.go` — compact tokens, money, percent deltas, source
  labels, model/agent column sizing and scroll helpers.
- `help.go` — the agent filter and keybinding overlays.

## Tabs

- **Overview** — KPI row, stacked company chart (tokens or cost), top models.
- **Models** — sortable table; `Enter` opens the model detail.
- **Daily** — sortable table with the vs-7-days column; `Enter` opens the day
  detail.
- **Activity** — one-year Monday-start heatmap, period overview and selected day
  detail.
- **Quota** — provider cards in `display.quota_provider_order`, gauges, pace,
  resets, credits and extra usage.
- **Keeper** — engine and per-agent switches, wakeup times, weekly targets,
  manual pings and executions.
- **Settings** — phone-editable config keys, theme, Web remote and provider
  rows with `J`/`K` ordering.

Overview, Models and Daily share one range (default `30d`) and the agent
filter. Activity, Quota, Keeper and Settings ignore the range.

## Refresh ownership

`internal/tui` checks `serve.sock` at start and on each tick. Without `serve` it
owns scanning, quota refresh, scheduled Keeper pings and notifications. With
`serve` it reads the databases, listens to the local WebSocket events, sends
settings/Keeper writes through the local API, and `r` requests a refresh instead
of scanning. A stale socket is ignored and the TUI takes ownership again.

## Keys

`q`/`Ctrl+C` quit, `←`/`→`/`h`/`l`/`Tab`/`Shift+Tab` switch tabs, a click on a
tab selects it, `s` filters agents, `b` cycles the theme, `r` refreshes and `?`
opens the help overlay. Overview, Models and Daily add `1`–`6` presets, `[`/`]`
range shifting and `n` back to today; the page-specific keys are in
`tui-usage.md`.

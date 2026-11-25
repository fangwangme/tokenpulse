# TUI Module

The dashboard uses Bubble Tea and Lip Gloss. Binding behavior is in
[`../specs/tui-usage.md`](../specs/tui-usage.md), with theme and table rules in
`tui-theme.md` and `tui-table-layout.md`.

## Tabs

- **Usage** — one report page: presets/custom range, agent filter, Token/Cost
  switch, KPI row, heatmap, day/model lists, and day/model detail panels.
- **Quota** — per-provider cards with plan/account, ordered windows, bars,
  pace, resets, credits and extra usage.
- **Keeper** — engine and per-agent switches, wakeup times, weekly targets,
  manual pings and recent executions.
- **Settings** — the same config keys the phone may change, plus theme and
  local-only display choices.

## Refresh ownership

`internal/tui` checks `serve.sock` at start and on each tick. Without `serve` it
owns scanning, quota refresh, scheduled Keeper pings and notifications. With
`serve` it reads the databases, listens to the local WebSocket events, sends
settings/Keeper writes through the local API, and `r` requests a refresh instead
of scanning. A stale socket is ignored and the TUI takes ownership again.

## Keys

`q`/`Ctrl+C` quit, `Tab`/`Shift+Tab` switch tabs, `b` cycles the theme and `r`
refreshes. The Usage page adds `1`–`6` presets, `[`/`]` range shifting, `m`
metric, `v` list mode, `f` heatmap focus, `Enter` detail and `Esc` close.

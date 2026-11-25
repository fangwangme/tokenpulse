# TUI Usage Page

The TUI has four tabs: **Usage**, **Quota**, **Keeper**, **Settings**. Usage
replaces the former Overview, Models, Daily and Activity tabs, which were all
date trends; it presents the report of `usage-report.md`, the same numbers the
phone shows. Quota, Keeper and Settings keep their behavior, except where
`quota.md` changes quota rules. Theme, table layout and truncation follow
`tui-theme.md` and `tui-table-layout.md`.

## Page state

The page state is the **range** (preset, and `from`/`to` for custom), the
**agent filter** and the **metric** (tokens or cost). All three apply to the
whole page, including detail panels, and persist for the TUI process only.

- The range starts at `today`, as on the phone. `--since` on the command line starts it as a custom
  range from that date to today.
- The agent filter starts with every enabled source. At least one agent stays
  selected: deselecting the last one is refused.
- The metric drives heatmap intensity, list proportion bars and the model
  detail's daily bars at once.

## Layout

Top to bottom:

1. **Range bar** — the presets `today · 7d · 30d · year · all · custom`, the
   resolved dates, and the scan time (`scannedAt`). Custom opens an inline
   `from`/`to` date entry limited to first-day…today.
2. **KPI row** — cost, tokens, sessions. In a period each shows the comparison
   change (`usage-report.md` "Comparison") and the comparison dates; a single
   day shows no comparison. An unpriced-model notice and the estimate notice
   appear when they apply.
3. **Heatmap** — periods only, in the layout `usage-report.md` selects for the
   range; when the terminal is too narrow for `week-columns`, it shows the
   newest weeks that fit and scrolls sideways. It shows active days, current
   and longest streak, and the legend; a cell can be selected to open the day
   detail.
4. **List** — a single day always lists models. A period lists dates (newest
   first) or models (tokens descending), switched by a key. Each row shows the
   full date or model name (truncated only by the column rules), a second
   detail `N sessions` for dates or `N sessions · agents` for models, cost and
   tokens right-aligned, and a proportion bar against the largest row under
   the current metric. Models are colored by company.

## Details

`Enter` on a list row or heatmap cell opens a detail panel over the list;
`Esc` closes it (`Esc` never quits).

- **Day detail** — totals (cost, tokens, input, output, cache read, cache write
  as `—` when 0, messages, sessions), then per agent its cost and per model its
  cost, tokens, cache read and cache hit with two decimals. Cache hit is per
  model, not a day-wide total across models.
- **Model detail** — the model in its company color, `company · range`,
  totals, last used, per-agent tokens and cost, and in a period the per-day
  bars.

## Keys

The existing global keys keep their meaning (`q`/`Ctrl+C` quit, `←`/`→`/`Tab`
switch tabs, `j`/`k` move, `s` agent filter overlay, `a` all agents in the
overlay, `b` theme, `r` refresh). On the Usage page:

| Key | Action |
| :--- | :--- |
| `1`–`6` | select preset today, 7d, 30d, year, all, custom |
| `[` / `]` | shift the range one range-length earlier / later (never past today) |
| `m` | toggle metric tokens / cost |
| `v` | toggle period list dates / models |
| `Enter` / `Esc` | open / close detail |
| `n` | back to the preset's range ending today |
| `f` | move focus between heatmap and list (periods only) |
| arrow keys / `h` `j` `k` `l` with the heatmap focused | move the selected day, scrolling `week-columns` sideways as needed |

While the heatmap has focus, `←`/`→`/`h`/`l` move the selected day instead of
switching tabs; `Tab`/`Shift+Tab` switch tabs whatever has focus.

`r` refreshes usage and quota; when `serve` runs it asks `serve` instead and the
page reloads when `scannedAt` changes, and the quota-recovery toast follows
`serve`'s recovery event (`remote.md` "Desktop TUI").

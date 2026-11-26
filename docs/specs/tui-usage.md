# TUI Pages

The TUI draws the 0.6.16 desktop dashboard on the 0.7 runtime. It has seven
tabs — **Overview**, **Models**, **Daily**, **Activity**, **Quota**,
**Keeper**, **Settings** — laid out, labelled and keyed as in the Rust release
(`v0.6.16:tokenpulse-cli/src/tui/`; its screenshots are `docs/images/*.png` at
the commit before #108). It takes the phone's range model and detail panels
where the old dashboard had none. It opens on Overview.

The TUI is a frontend of `runtime.Client` only (`remote.md` "Desktop TUI"): it
reads the report, day detail, model detail, quota, Keeper records, status and
config the phone reads, and needs no API of its own. Where 0.6.16 and 0.7 rules
differ, the layout is 0.6.16's and the numbers are 0.7's (`usage-report.md`,
`pricing.md`, `quota.md`), so the TUI and the phone show the same values.
Theme, table layout and truncation follow `tui-theme.md` and
`tui-table-layout.md`.

## Range and filter

- **Range.** Overview, Models and Daily share one range: the presets
  `today · 7d · 30d · year · all · custom` of `usage-report.md`, starting at
  `30d`. `--since` starts it as a custom range from that date to today. A bar
  under the tabs shows the presets, the resolved dates and the scan time;
  custom opens an inline `from`/`to` entry limited to first day…today.
- Activity, Quota, Keeper and Settings do not use the range; on Activity the
  bar names the Activity window instead.
- **Agent filter.** `s` opens the agent overlay (`a` selects all). The filter
  applies to every page and every usage request. At least one agent stays
  selected: deselecting the last one is refused.
- Range, filter, sort and metric choices last for the TUI process only.

## Overview

1. **KPI row** — cost, tokens and sessions of the range. A period shows each
   value's comparison change and the comparison dates
   (`usage-report.md` "Comparison"); a single day shows none. The unpriced-model
   and estimate notices appear as on the phone.
2. **Chart** — the 0.6.16 stacked daily bar chart over the range's days, the
   newest that fit when the range has more days than the width holds, with
   0.6.16's bar widths, axis and date ticks. Its title is
   `Token Usage (N days)` or `Cost Usage (N days)`; `t` and `c` choose the
   metric.
   - Each bar is split by company: Anthropic, OpenAI and Google are the sums of
     that company's models' per-day values (the model detail over the range,
     one request per such model); **Others** is the day total minus those
     three, floored at zero. Models with company `other` are not requested.
   - The legend is `● Anthropic  ● OpenAI  ● Google  ● Others` in the company
     colors.
   - Until the company values arrive, bars show the day total in one color.
   - A single-day range has no chart; the model table takes its space.
3. **Top models** — the 0.6.16 table: `Model`, `Agent`, `Tokens`, `Cost`, `%`.

## Models

- The 0.6.16 table with the columns of `tui-table-layout.md`. `c`, `t` and `d`
  sort by cost, tokens and last used; pressing the active key again reverses
  the order.
- `Enter` opens the model detail: the model in its company color,
  `company · range`, totals, last used, per-agent tokens and cost, and in a
  period the per-day bars. `Esc` closes it.

## Daily

- One row per date of the range: `Date`, `Tokens`, `Cost`, `Input`, `Output`,
  `Cache R`, `Cache W`, `Msgs`, and from an inner width of 100 the
  `Token vs 7d` / `Cost vs 7d` column (`v` switches its metric).
- The vs-7-days column compares a date with the date seven days earlier, also
  when that date lies before the range start. It reads `+12.3%↑` or
  `-12.3%↓` (one decimal, explicit sign) and `—` when the earlier date has no
  value or zero. The earlier dates come from one extra report whose start is
  never before the first data day.
- `c`, `t` and `d` sort; `Enter` opens the day detail of the phone (totals, per
  agent its cost, per model its cost, tokens, cache read and cache hit with two
  decimals). `Esc` closes it.

## Activity

- **Window** — 365 days ending today, extended back to January 1 when that is
  earlier, and never before the first data day. It ignores the range.
- The 0.6.16 Monday-start year heatmap with active days, current and longest
  streak and the legend; `t` and `c` choose the metric.
- **Period overview** — tokens and cost for `Today`, `Week` (Monday of this
  week through today), `Month` (calendar month to date), `Year` (calendar year
  to date), `Last 7 days` (today − 6 through today) and `Last 30 days`
  (today − 29 through today). A short card drops the rolling rows first.
- **Selected day** — `↑`/`↓`/`j`/`k` move the selected day, `n` selects today,
  `[`/`]` scroll its details; the details list each agent and model with its
  cost from the day detail.

## Quota

- The 0.6.16 provider cards: two columns from a width of 110, standard cards
  (up to four windows) above tall cards, gradient gauges, pace, reset time,
  plan and account per `display.account_display`, banked resets and extra
  usage. OpenCode Go gets the same card.
- Cards follow `display.quota_provider_order`. Disabled providers are hidden;
  with `display.show_empty_providers` an enabled provider without a snapshot
  shows an empty card. Windows, percentages and pace follow `quota.md`.

## Keeper

- The 0.6.16 agent cards and execution log. `↑`/`↓`/`Tab` select an agent,
  `1`/`d` toggle its 5h keeper, `2`/`w` its weekly keeper, `p` runs a manual
  ping, `e` toggles the Keeper engine. Toggles go through `PatchSettings`,
  pings through `KeeperPing`.
- A card's last run and model come from the newest execution record of that
  agent; its weekly status comes from the agent's quota snapshot.

## Settings

- The 0.6.16 settings list plus the 0.7 rows, including **Web remote**
  (`remote.md` "Desktop TUI").
- **Quota providers** lists one row per provider in
  `display.quota_provider_order`. `Enter`/`Space` toggles the provider; `J` and
  `K` move it one place down or up and write the new order through
  `PatchSettings`, as the phone's drag does.
- Changes made elsewhere arrive as `settings_updated` and redraw the page.

## Global keys

`q`/`Ctrl+C` quit; `←`/`→`/`h`/`l`/`Tab`/`Shift+Tab` switch tabs, and a click
on a tab selects it; `j`/`k` move; `s` agent filter; `b` theme; `r` refresh;
`?` toggles the 0.6.16 help overlay listing the current page's keys. `Esc`
closes a detail, the overlay or the help, and never quits.

On Overview, Models and Daily:

| Key | Action |
| :--- | :--- |
| `1`–`6` | select preset today, 7d, 30d, year, all, custom |
| `[` / `]` | shift the range one range-length earlier / later (never past today) |
| `n` | back to the preset's range ending today; on Daily also select today's row |

## Data loading

- Requests run as Bubble Tea commands, never on the update loop.
- One report per range and agent filter feeds Overview, Models and Daily.
  Daily's earlier-dates report and the Activity window report (one custom
  range report each) load when their tab first needs them; Overview's company values load after the report;
  details load on `Enter`.
- `usage_updated`, a range change or a filter change drops the cached usage
  data; the current tab reloads at once and the others when next shown.
  `quota_updated` and a config change reload Quota, `keeper_updated` reloads
  Keeper, `settings_updated` reloads the config.
- `r` refreshes usage and quota; when `serve` runs it asks `serve` instead
  (`remote.md`). `quota_recovered` shows the 0.6.16 recovery toast and border
  pulse.

## Formats

- Tokens use the 0.6.16 compact form: two decimals and ` K`, ` M` or ` B`
  (`1.20 K`, `3.45 M`); below one thousand the plain value with two decimals.
- Costs use the shared money format (`usage-report.md` "Numbers").
- Models are colored by company with the 0.6.16 palette.

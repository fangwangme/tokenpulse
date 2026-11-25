# Usage Report

The usage report is the one reading of the usage ledger that every surface
shares: the TUI Usage page (`tui-usage.md`) and the phone app (`remote.md`).
Both show the same numbers for the same range, agents and time, because both
call the same report functions.

## Dimensions

Usage is looked at by **date** and by **model**. The agent is a filter, never a
dimension: there is no per-agent table. An earlier design had separate
overview, daily and activity views; all three were date trends and were
replaced by this one model.

## Presets and ranges

A request names a preset. `today` is the computer's local date.

| Preset | From | To |
| :--- | :--- | :--- |
| `today` | today | today |
| `7d` | today − 6 days | today |
| `30d` | today − 29 days | today |
| `year` | January 1 of today's year | today |
| `all` | the first day with usage | today |
| `custom` | the given `from` | the given `to` |

- `custom` swaps `from` and `to` when given in reverse; a missing endpoint is
  today. An endpoint after today or before the first day with usage is a
  request error naming both limits; the surfaces only offer dates inside that
  span.
- `all` with no usage at all starts today.
- **The first day with usage** is the earliest ledger date among the selected
  agents.
- A range of one day is a **single day**; anything longer is a **period**.
  The layout follows this distinction, never the preset: a custom range whose
  ends are equal is a single day.

Dates are local `YYYY-MM-DD` strings everywhere, in the API and internally.

## Comparison

Each report carries the totals of a comparison range next to its own, or none:

- `year` and `all` — no comparison.
- A single day that is today — the same weekday one week earlier, counting only
  messages whose local time of day is at or before the current minute
  (`untilMinutes`, minutes after local midnight).
- Any other single day — the same weekday one week earlier, the whole day.
- A period of N days — the N days just before it.
- No comparison when the comparison range starts before the first day with
  usage.

The surfaces show the change in percent with one decimal, at every size. A
single day carries its comparison in the report but the surfaces do
not show it.

## Heatmap layout

A period is shown as a grid of days; the orientation follows the range length
only:

| Range | Layout | Shape |
| :--- | :--- | :--- |
| ≤ 7 days | `strip` | one row starting at `from`, each cell labeled with its weekday |
| spanning ≤ 6 Monday-based weeks | `week-rows` | one row per week, columns Monday to Sunday; days outside the range are empty |
| longer | `week-columns` | one column per Monday-based week, rows Monday to Sunday; scrolls sideways, opens at the newest week, no zoom |

A heatmap shows the active days, the current and the longest streak inside the
range, and a five-step legend plus an empty step; selecting a step shows the
value range it covers. Cell intensity follows the page's metric (tokens or
cost). Selecting a day with usage opens its day detail.

## Numbers

- **Total tokens** is the ledger's total: input + output + cache read + cache
  write + reasoning, where reasoning is non-zero only for sources that report
  it apart from output. The totals carry no separate reasoning field; it is
  inside `tokens` only.
- **Cache hit** = cache read ÷ total tokens × 100, rounded to two decimals; 0
  when the total is 0.
- **Messages** counts ledger messages after the ledger's own de-duplication.
- **Sessions** counts distinct `(canonical source, session id)` pairs inside the
  range. A range total is never the sum of daily session counts: one session
  spanning two days is one session in the range.
- **Cost** is the ledger's stored cost (`pricing.md`). A model is **unpriced**
  in a range when any of its messages there is unpriced; the surfaces show a
  notice whenever the range contains an unpriced model, and always say that
  cost is an estimate at public API prices that subscriptions are not billed
  by.
- **Money display** is one format everywhere: below $1,000, `$` and two
  decimals; from $1,000 to below $1,000,000, `$` and three decimals with a `K`
  suffix; from $1,000,000, `$` and three decimals with an `M` suffix. Examples:
  `$999.99`, `$1.000K`, `$9.877K`, `$10.000K`, `$1.235M`. The phone KPI cards,
  list rows, sheets, heatmap legend and Quota extra usage, and the TUI/CLI
  amounts all use it; machine-readable JSON and CSV keep the exact number.
- Antigravity sources (`antigravity`, `antigravity-cli`, `antigravity-desktop`,
  `antigravity-ide`) count as one agent, `antigravity`, as the ledger already
  does.

## Report

`report(preset, today, nowMinutes, from?, to?, agents)` answers:

| Field | Meaning |
| :--- | :--- |
| `range` | `{ from, to }` after resolution |
| `firstDay` | first day with usage among `agents`, or null |
| `scannedAt` | ms since epoch of the last completed scan |
| `pricesAvailable` | whether a price catalog is loaded (`pricing.md`) |
| `totals` | `UsageTotals` for the range |
| `comparison` | `{ from, to, untilMinutes?, totals }` or null |
| `days` | one `UsageDaySummary` per date with usage, newest first |
| `models` | one `UsageModelSummary` per canonical model, by tokens descending |

`UsageTotals` = `{ cost, tokens, input, output, cacheRead, cacheWrite, messages,
sessions }`. A day summary adds `date`. A model summary adds `model`, `company`
(`anthropic`, `openai`, `google`, `other`), `agents` (the agents that used it in
the range), `lastUsed` (its latest date in the range) and `unpriced`.

An empty `agents` list is a request error, not an empty report: every surface
keeps at least one agent selected.

## Day detail

`day(date, agents)` answers `{ date, totals, agents: [...] }`. Each agent entry
carries its `UsageTotals` and `models`: per model, its `UsageTotals` and
`company`. Agents and their models are ordered by tokens descending. The
surfaces show total input, output, cache read, cache write (`—` when 0),
messages and sessions. Cache hit is shown per model, next to its cache read,
because a cache is read by one model; a day-wide hit rate mixing models has no
meaning.

## Model detail

`model(model, from, to, agents)` answers `{ model, company, totals, lastUsed,
agents: [{ agent, ...UsageTotals }], perDay: [{ date, tokens, cost }] }`.
`perDay` has one entry for every date of the range, zero-filled where the model
has no usage. In a period the surfaces draw `perDay` as small bars scaled by the
page's metric.

## JSON

Field names are camelCase; dates are `YYYY-MM-DD`; timestamps are ms since
epoch; `untilMinutes` is omitted when absent. Agent names are TokenPulse's
(`claude`, `codex`, `copilot`, `gemini`, `opencode`, `pi`, `antigravity`)
inside the program; the HTTP API maps them for the phone app (`remote.md`).

## Caching

A long-running process may cache reports keyed by `scannedAt` and every request
parameter (for `today`, also the current minute), holding at most 64 entries.
Every completed scan, reprice or rebuild clears the cache.

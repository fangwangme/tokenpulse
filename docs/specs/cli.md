# CLI

`tokenpulse` prints the interactive dashboard when stdin and stdout are
terminals and `TERM` is not `dumb`; otherwise it prints the text summary. `q`
or Ctrl+C quits. Version output is `tokenpulse <version>` (`<version> (test)`
under `TOKENPULSE_ENV=test`).

## Global flags

| Flag | Meaning |
| :--- | :--- |
| `--since <YYYY-MM-DD>` | Only parse and show usage from this date |
| `--refresh-days <A:B>` | Reparse one date range; `B` must not precede `A` |
| `--refresh-pricing` | Fetch the price catalog now |
| `--rebuild-all` | Reparse every source from scratch |
| `--csv daily\|models` | CSV instead of text or the dashboard; conflicts with `--json` |
| `--json` | JSON instead of text or the dashboard |
| `--no-scan` | Read the cached ledger; never start a scan |
| `--no-tui` | Plain text even on a terminal |
| `--log` | Write usage timing to `log/usage-YYYY-MM-DD.log` |
| `--raw`, `--date <YYYY-MM-DD>` | Raw model usage for a date (default today); `--date` requires `--raw` |
| `-h`, `--help`, `-V`, `--version` | Help and version |

A run reads the ledger first. If `serve` is running and its cache is ready,
the command uses that scan instead of scanning again; `--no-scan` always reads
the cache. Without serve, the command scans before reading unless `--no-scan`
is set. `--since` narrows both the parse window and the output.
`--refresh-days` only reparses the range, but the output starts at its first
day. Explicit `--refresh-days`, `--refresh-pricing` and `--rebuild-all` always
scan.

## Commands

- `tokenpulse init [--default]` — setup. `--default` enables detected providers
  and writes the config; the interactive selector is not part of the Go build.
- `tokenpulse usage [--raw] [--date] [--json]` — the same report as the root
  command.
- `tokenpulse config show` — provider states, display settings and per-agent
  Keeper rows with the resolved model.
- `tokenpulse config enable|disable <provider>` — only quota providers.
- `tokenpulse config set KEY=VALUE` — see `config.md`.
- `tokenpulse config test-notification` — fires one sample recovery at the
  configured level.
- `tokenpulse reprice [--apply --yes --expect <fingerprint>] [--json]` — see
  `pricing.md`.
- `tokenpulse serve ...` — see `remote.md`.
- `tokenpulse remote reset-token|url|status` — resets the token and sessions;
  prints the pairing URL and QR; reports the local socket and a human-readable
  last-scan time.
- `tokenpulse service install|uninstall|status` — see `remote.md` "Service".

Every subcommand accepts `-h`/`--help` and prints its own usage instead of the
root help.

## Text output

`=== Usage Summary ===` with total cost, tokens, messages, sessions, active
days, average daily cost and tokens; `=== By Provider ===` (name uppercased,
tokens, cost, messages, sessions); `=== By Model ===` (`model [sources]`);
`=== Recent Daily Totals ===` (last 365 days); `=== Weekly Totals ===` (last 8
weeks); `=== Monthly Totals ===` (last 6 months); a final loaded-messages line.
Quota follows: per provider its plan/account as the display setting allows, one
line per window (`label: percent used|remaining (resets in Xh Ym)`), credits and
banked resets. With no data: `No usage data found in the local ledger.` on
stderr, followed by the checked source paths.

Counts use comma grouping; costs use the compact money format in
`usage-report.md` "Numbers". JSON and CSV keep exact numbers.

## JSON

`{ "usage": UsageSummary, "quota": [QuotaSnapshot] }`. `UsageSummary` keeps the
0.6.x snake_case shape: `total_cost`, `total_tokens`, `message_count`,
`session_count`, `active_days`, `avg_daily_cost`, `max_daily_cost`,
`avg_daily_tokens`, `max_daily_tokens`, `daily`, `weekly`, `monthly`,
`by_provider`, `by_model`. Each daily row has `date`, token fields, `cost`,
`messages`, `sessions` and the two intensity buckets. Weekly/monthly rollups
carry `label`, `start_date`, `end_date`, totals, messages, sessions and
`active_days`. Providers have `provider`, `cost`, `tokens`, `message_count`,
`sessions` and `percent`; models add `model`, `provider`, `source`, cache fields,
`message_count`, `session_count` and `percent`. Quota snapshots use the API
shape (camelCase, `remote.md`).

With no data, `--json` prints an empty summary.

## CSV

- `daily`: `date,source,total_tokens,cost_usd,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,messages,sessions`
- `models`: `model,provider,source,tokens,cost_usd,messages,sessions,percent`

Six decimals for costs.

## Raw output

Table columns: Source, Raw / Served Model, Tier / Alias (or `-`), Canonical,
Total Tkns, Cost USD, Calls. The detailed In/Out/CR/R values are in the JSON
payload, not the text table. Rows are grouped by agent (total tokens
descending), then canonical model, then raw variant. The
JSON shape is `{date, total_tokens, total_cost_usd, message_count,
session_count, models: [...]}` with the raw fields and six-decimal costs.
`--date` must parse as `YYYY-MM-DD`.

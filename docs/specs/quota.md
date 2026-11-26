# Quota

Quota is how much of each subscription's rate-limit windows is used, for the
agents whose provider exposes it: Claude Code, Codex and Antigravity. The
provider endpoints, response mapping, account display and observation history
are described in the provider sections of this spec (ported from the Rust
release); this file fixes the rules every surface shares.

## Snapshot

A provider's quota is one snapshot: `provider`, optional `plan`, optional
`account`, `windows`, the credit fields the Rust release already carries
(Codex credit balance, and rate-limit reset credits with their expiry), optional
`extraUsage` and `fetchedAt`.

A window has `id`, `label`, optional `family` (the model family it meters, such
as `Gemini`, `Claude`, `Opus`), `usedPercent` (0–100 as reported), `periodMs`
(the window length, when known) and `resetsAt` (or none). `id` is the
provider's own bucket id when the response carries one, else a stable slug of
`family` and `label`; it exists for the API and never replaces the
`label`/`family` keys of the observation history. A snapshot holds
exactly the windows the provider reported; no window is ever added from a fixed
per-provider list, because windows that exist in code but not in the user's
data misled.

`extraUsage` = `{ used, limit, currency }`, present when Claude reports
`extra_usage.is_enabled`; `used` and `limit` are scaled by
`10 ^ (decimal_places ?? 2)`, and it is shown even at 0.

## Order

Providers are listed Claude, Codex, Antigravity.

Windows are grouped by `family` in the order the provider first names each
family; windows without a family form one group of their own in the same way.
Within a group, windows are ordered by `periodMs` ascending, unknown last — the
5-hour window before the 7-day one. This holds for every provider, even where
the API lists weekly first.

## Level and pace

**Level** depends only on `usedPercent`: `normal` below 80, `warn` from 80,
`exhausted` at 100.

**Pace** compares the used share with the elapsed share of the window. With
`remaining = resetsAt − now` and `elapsed = periodMs − remaining`:

- No pace when `usedPercent` ≤ 0 or ≥ 100, when `periodMs` or `resetsAt` is
  unknown, or when `elapsed` ≤ 0 or `remaining` ≤ 0.
- `expected = elapsed ÷ periodMs × 100`; `deficit = usedPercent − expected`.
- `|deficit| < 5` → `on_track`.
- `deficit > 0` → `ahead`, with `eta = (100 − usedPercent) ÷ (usedPercent ÷
  elapsed)`, the time until the window runs out at the current rate.
- otherwise → `behind`.

The Rust releases named these two the other way round (`behind` for a window
used faster than time) and inverted the colors to compensate, and colored
gauges at 50/75. Both are replaced by the rules here; they are not parity.

**Status color**, one rule for bars, dots and figures: danger when exhausted or
ahead; warn when level is warn or pace is on track; good otherwise. Pace text:
good for behind, warn for on track, danger for ahead or exhausted. An earlier
design colored bars by pace and numbers by share, which put a red bar next to a
yellow number.

Whether bars show remaining or used share is the `quota_display_mode` setting,
not a page toggle; the bar, the time marker and the legend all follow it.

## Credentials are read-only

TokenPulse never refreshes, rotates or writes an agent's credentials. A
rotating refresh token used by both TokenPulse and the agent can sign the agent
out, and a long-running `serve` would make that likely; the gain is only a
quota figure. This replaces the Claude, Codex and Antigravity token refresh of
the Rust releases, including Antigravity's copy of a Keychain credential into
`~/.gemini/antigravity-cli/antigravity-oauth-token`: TokenPulse writes no file
of an agent's at all.

- Credential sources are read in their existing order. A source whose access
  token has expired is skipped; when every source is expired or the usage call
  is rejected (401/403), the provider fails with a message naming the agent and
  saying to open it so it refreshes its login, e.g. `Claude: login expired —
  open Claude Code to refresh it`, `Antigravity: login expired — open
  Antigravity to refresh it`. No token endpoint is called.
- The next fetch reads the sources again, so quota returns as soon as the agent
  has refreshed. A Keeper ping runs the agent's own CLI, which refreshes its
  own login.
- Antigravity's `agy` CLI source runs even when the cached token file has
  expired, because the CLI refreshing its own login is the agent acting on its
  own credential.

## Fetching and failures

- Quota and usage refresh on one schedule (`display.auto_refresh_secs`), owned
  by `serve` when it runs (`remote.md`) and by the TUI otherwise. Enabled
  providers fetch in parallel, and a slow provider never blocks the others or
  the usage scan.
- A requested refresh runs that target and waits for it; each target has a
  60-second cooldown. A repeat inside the cooldown returns the previous cache
  without calling a provider, and never moves the periodic timer.
- Every other read answers from the quota cache and never calls a provider. The
  cache is warmed from disk on server start, so a restart does not blank the
  phone.
- Expected errors are per provider and never fail the whole refresh:
  credentials missing, credentials expired or rejected, network unreachable or
  timed out, an unexpected HTTP status (including 429), a response that cannot
  be parsed. Each carries a short message naming the provider and the cause.
- A provider that fails keeps its previous snapshot next to the new error, and
  the surfaces show both. It is not retried before the next interval.
- `display.refresh_quota = false` keeps its existing meaning: no provider is
  called, cached snapshots are shown.

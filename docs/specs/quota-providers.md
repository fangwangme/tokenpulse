# Quota Providers

The provider sections below describe each provider's credential source and
response mapping. `quota.md` carries the rules every surface shares and the
read-only credential policy.

## Credential sources

- Claude, in this order: current-user macOS Keychain item
  (`Claude Code-credentials`, account = login name), legacy Keychain item,
  `~/.claude/.credentials.json`. JSON shape:
  `{ "claudeAiOauth": { accessToken, refreshToken, expiresAt, subscriptionType,
  ... } }`. `expiresAt` is milliseconds; `0` means unknown.
- Codex, first readable of `$CODEX_HOME/auth.json`,
  `~/.config/codex/auth.json`, `~/.codex/auth.json`. Shape:
  `{ "tokens": { access_token, refresh_token, id_token, account_id } }` or
  `{ "OPENAI_API_KEY": "..." }`. Expiry comes from the access token's JWT
  `exp` with a five-minute buffer.
- OpenCode: `$OPENCODE_DB` (a relative value is inside the OpenCode data
  directory), else `$XDG_DATA_HOME/opencode/opencode.db` or
  `~/.local/share/opencode/opencode.db`. Read-only SQLite table `credential`,
  active row where `integration_id = 'opencode'`. Value shape:
  `{ "type": "key", "key": "..." }` or
  `{ "type": "oauth", "access": "...", "expires": <ms>, "metadata": { "orgID":
  "...", "email": "..." } }`. `expires` is milliseconds; `0` means unknown.
  Expired and `external` credentials are skipped.
- Antigravity: `~/.gemini/antigravity-cli/antigravity-oauth-token` (override
  `TOKENPULSE_ANTIGRAVITY_AUTH_PATH`), then the macOS Keychain item `gemini` /
  `antigravity` (value `go-keyring-base64:<base64 JSON>`). Shape:
  `{ "token": { access_token, expiry, refresh_token }, "id_token": "..." }`.
  A missing or unparsable `expiry` counts as expired.

Expired sources are skipped. No source is refreshed and no file is written.

## Claude

`GET https://api.anthropic.com/api/oauth/usage` with the access token as a
bearer, `anthropic-beta: oauth-2025-04-20`, 20-second timeout.

Windows, in response order: `five_hour` → `Session (5h)`, 5 h; `seven_day` →
`Weekly (7d)`, 7 d; `seven_day_sonnet` → `Sonnet (7d)`, family `Sonnet`;
`seven_day_opus` → `Opus (7d)`, family `Opus`; a `limits` entry with
`kind = weekly_scoped` and model display name `Fable` → `Fable (7d)`, family
`Fable`. Percent is `utilization` (or `percent` for the Fable limit); reset is
RFC3339. Plan is the credential's `subscriptionType`; the account is
`oauthAccount.emailAddress` in `~/.claude.json`, read only when the account
display asks for it.

`extra_usage` is present when `is_enabled` is true: `used = used_credits / 10 ^
decimal_places` (default 2), `limit = monthly_limit / scale`, currency as
reported (default `USD`), shown even at 0.

## Codex

`GET https://chatgpt.com/backend-api/wham/usage` with the access token, 20-second
timeout; then `GET https://chatgpt.com/backend-api/wham/rate-limit-reset-credits`.

`rate_limit.primary_window` and `secondary_window` map only when present.
`used_percent` accepts a number or a numeric string. The label follows
`limit_window_seconds`: 18000 → `Session (5h)`, 604800 → `Weekly (7d)`, another
positive value → `Window (1d)`/`Window (1h)`/`Window (1m)`/`Window (Ns)`, absent
→ `Primary window`/`Secondary window`. `periodMs` is the duration in
milliseconds when positive. Reset is `reset_at` (Unix seconds), else now plus
`reset_after_seconds`. Plan is `plan_type`; account is the `id_token` JWT
`email`. Reset credits keep only `status = available`. All windows have no
family.

## OpenCode Go

`GET https://opencode.ai/inference/go/v1/usage` with the credential as a bearer,
`User-Agent: tokenpulse/<version>`, an optional `x-opencode-org-id` from
`metadata.orgID`, and a 20-second timeout.

`rolling` → `5h`, 5 h; `weekly` → `Weekly`, 7 d; `monthly` → `Monthly`, 30 d.
The labels stay short; the period behind them is what pace uses. Percent is
`percent`, clamped to 0–100; `status = rate-limited` counts as 100. Reset is
RFC3339. There is no plan in the response. The account is `metadata.email`,
read only when the account display asks for it.

A 401 or 403 omits the provider (expired login or no Go subscription); it is
not an error.

## Antigravity

Order: a running language server, then the `agy` CLI, then Cloud Code.

Language server: POST the standard IDE metadata body to
`exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary` and
`GetUserStatus` on each detected connection (Desktop, then CLI, then others).
A summary is `{groups: [{displayName, buckets: [{bucketId, window,
remainingFraction, resetTime}]}]}`. Used percent is
`(1 - clamp(remainingFraction, 0, 1)) * 100`. A group named `Gemini Models` or
`Gemini` becomes `Gemini`; one containing `claude` or `gpt` becomes `Claude`;
any other name drops a trailing ` models`, and an empty name becomes `Usage`
(no family). Window `5h` → suffix `5h`, 5 h; `weekly` → `7d`, 7 d; anything else
keeps its name and has no period. The label is `<group> (<suffix>)`; the bucket
id is the window id when present. The fetcher orders Gemini, then Claude, then
other groups in API order, and 5h before weekly inside a group. Account is
`userStatus.email`; plan is `userTier.name`, else
`planStatus.planInfo.planName`.

`agy` CLI fallback: only when `agy --version` is at least 1.4; run
`agy -p /usage --output-format json` with a 15-second timeout. Data is
`command.data.groups` with the same bucket fields (`remaining_fraction`,
`reset_time`). No plan; account from the cached `id_token` when readable.

Cloud Code fallback: only with an unexpired access token.
`POST https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist` with
`{metadata:{ideType:ANTIGRAVITY, platform:PLATFORM_UNSPECIFIED,
pluginType:GEMINI}}`; project is `cloudaicompanionProject` (string or `.id`),
plan is `paidTier.name`, else `currentTier.name`. Then
`POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary`
with `{project}` when known, `User-Agent:
antigravity/hub/2.9.1 <os>/<arch>`. The body may be wrapped in `response`;
windows map as above. Account is the credential's `id_token` email. A 401/403 is
the expired-login error.

## Cache and history

`tokenpulse.db`: `quota_cache` holds one JSON snapshot per provider with
`observed_at`, `expires_at` (five minutes), `fetched_at`; a successful fetch
also appends one `quota_observations` row per window, one
`quota_credit_observations` row for credits and one `quota_fetch_failures` row
per failure. Observation rows keep `provider`, `observed_at`, `fetched_at`,
`plan`, `account`, `window_label`, `model_family`, `used_percent`, `resets_at`
and `period_duration_ms`. The schema version is 2; `model_family` is added to an
older table.

Reads that are not a refresh use the unexpired cache and never call a provider.
A failed fetch keeps the previous snapshot next to the new error.

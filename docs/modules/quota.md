# Quota Module - Detailed Design

## Overview

On-demand fetching of remaining usage quota from coding agent APIs. No polling.

Quota providers: Claude Code, Codex and Antigravity. They are registered in one
list, `QUOTA_PROVIDERS` in `tokenpulse-cli/src/commands/quota.rs`; the Settings
rows, the ids `config enable` accepts and the providers quota resolution honours
are all derived from it. A `[providers.<id>]` entry for anything else — such as
`copilot` or `gemini` from an older config — still loads but is ignored. Copilot
CLI and Gemini CLI *usage* parsing is separate and unaffected.

## Architecture

```
quota/
├── mod.rs          # QuotaFetcher trait, QuotaSnapshot struct, fetch_all()
├── claude.rs       # Claude Code quota fetcher
├── codex.rs        # Codex quota fetcher
├── antigravity.rs  # Antigravity quota fetcher
└── cache.rs        # Quota response caching (one overwritten row per provider)
```

Observation history lives outside this module in `core/src/history/`, because it
also stores Keeper executions. See [Observation history](#observation-history).

## QuotaFetcher Trait

```rust
#[async_trait]
pub trait QuotaFetcher: Send + Sync {
    fn provider_name(&self) -> &str;
    fn provider_display_name(&self) -> &str;
    async fn fetch_quota(&self) -> Result<QuotaSnapshot>;
}
```

## Claude Code

### Credential Flow
1. On macOS, build ordered credential candidates from the current-user Keychain item (`Claude Code-credentials` plus the explicit macOS account), the legacy service-only Keychain item, then `~/.claude/.credentials.json`. Duplicate credentials are removed. On other platforms, read only the credentials file.
2. Try each candidate in order. A missing or rejected refresh token (`invalid_grant`) or a rejected access token can fall through to the next candidate; network, proxy, rate-limit, and provider errors stop the refresh instead of trying unrelated credentials.
3. Check `expiresAt` — if within 5 minutes, refresh that candidate at most once via `POST https://platform.claude.com/v1/oauth/token`:
   - `grant_type=refresh_token`
   - `client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e`
   - `refresh_token=<token>`
   - Reuse the credential's stored `scopes` unchanged. If scope metadata is absent, omit `scope` so the refresh inherits the original authorization.
4. Save a successful rotation only to the source that supplied the credential, preserving the original scope metadata and all unknown credential fields. Before writing, re-read the candidates; a newer Claude Code login wins and the in-flight rotation is discarded.
5. After a successful usage response, re-read the credential candidates before publishing the snapshot. If Claude Code logged in again during the request, discard the old response and restart the complete quota fetch once.

Provider detection, initialization hints, credential status, and quota fetching all use this same candidate lookup.

### Quota API
```
GET https://api.anthropic.com/api/oauth/usage
Authorization: Bearer <access_token>
anthropic-beta: oauth-2025-04-20
```

### Response Mapping
| API Field                              | → RateWindow                                   |
| -------------------------------------- | ---------------------------------------------- |
| `five_hour.utilization`                | Session (5h); `used_percent = utilization`     |
| `seven_day.utilization`                | Weekly (7d)                                    |
| `seven_day_sonnet.utilization`         | Sonnet (7d); `model_family = "Sonnet"`         |
| `seven_day_opus.utilization`           | Opus (7d); `model_family = "Opus"`             |
| `limits[]` where `kind == "weekly_scoped"` and `scope.model.display_name == "Fable"` | Fable (7d); `used_percent = percent`; `model_family = "Fable"` |

The Session and Weekly windows meter the pooled quota, so they leave
`model_family` empty; only the per-family weekly windows set it.

`utilization` and `percent` are already 0–100 values; TokenPulse stores them directly as `used_percent` without rescaling.

Extra-credit usage (`extra_usage`) is intentionally not surfaced for Claude Code.

### Plan and Account
- `plan` is the `subscriptionType` (`pro`, `max`, ...) of the credential that
  served the request, stored as-is; nothing is hardcoded.
- `account` is `oauthAccount.emailAddress` from `~/.claude.json`, Claude Code's
  global config. The file is read only when `display.account_display = "full"`,
  since that is the only setting that shows the email.

## Codex

### Credential Flow
1. Read `~/.config/codex/auth.json` or `~/.codex/auth.json`
2. Fallback: env `CODEX_HOME` / macOS Keychain
3. Check `last_refresh` — if >8 days, refresh
4. Refresh: `POST https://auth.openai.com/oauth/token`
   - form-encoded: `grant_type=refresh_token&client_id=app_EMoamEEZ73f0CkXaXp7hrann&refresh_token=<token>`

### Quota API
```
GET https://chatgpt.com/backend-api/wham/usage
Authorization: Bearer <access_token>
```

Manual rate-limit reset credits are fetched separately:

```
GET https://chatgpt.com/backend-api/wham/rate-limit-reset-credits
Authorization: Bearer <access_token>
```

### Response Mapping
`primary_window` and `secondary_window` are response positions, not semantic names. TokenPulse emits exactly the non-null windows returned by the API and derives each label from `limit_window_seconds`:

| API Value | → RateWindow |
| --------- | ------------ |
| `limit_window_seconds == 18000` | `Session (5h)` |
| `limit_window_seconds == 604800` | `Weekly (7d)` |
| other positive duration | neutral formatted label such as `Window (1d)` |
| missing or non-positive duration | neutral positional label such as `Primary window` |
| `reset_at` | window reset countdown source |
| `reset_after_seconds` | reset countdown fallback |
| `plan_type` | snapshot plan |
| reset credits `credits[].expires_at` | manual reset-credit expiry |

No missing 5-hour or 7-day window is synthesized. Reset-credit fetching and display are independent and unchanged.

## Antigravity

### Credential Flow
No external auth lookup. Antigravity quota is read from a running local Antigravity language server.

### Quota Probe
1. Discover running Antigravity CLI/Desktop language server processes
2. Prefer CLI LS, then Desktop LS, then unknown Antigravity LS processes
3. Send a Connect-RPC `RetrieveUserQuotaSummary` request to the local language server; on success, make a best-effort `GetUserStatus` call for account email + plan name
4. If no language server responds, fall back to the Cloud Code quota API. That response has no plan or account, so the snapshot leaves both empty rather than guessing one

### Response Mapping
`RetrieveUserQuotaSummary` returns one group per model family, each with a `5h` and a `weekly` bucket. Each bucket maps to a `RateWindow`:

| Bucket field                 | → RateWindow                                                    |
| ---------------------------- | --------------------------------------------------------------- |
| group `displayName`          | Label prefix and `model_family` (`Gemini Models` → `Gemini`, `Claude and GPT models` → `Claude`). An unnamed group falls back to the label `Usage` with an empty `model_family`. |
| `window` (`5h` / `weekly`)   | Label suffix `(5h)` / `(7d)` and period duration (5h / 7d)      |
| `remainingFraction`          | `used_percent = round((1 - remainingFraction) * 100)`           |
| `resetTime`                  | `resets_at`                                                     |

Windows are sorted Gemini before Claude, and within each group the 5-hour limit before the weekly limit.

---



All providers fetched in parallel via `tokio::join!`:

```rust
pub async fn fetch_all(providers: &[Box<dyn QuotaFetcher>]) -> Vec<Result<QuotaSnapshot>> {
    let futures: Vec<_> = providers.iter().map(|p| p.fetch_quota()).collect();
    futures::future::join_all(futures).await
}
```

## Account Display

`display.account_display` decides how much of the account the quota views show:

| Value  | Quota card row                 | Plain-text output        |
| ------ | ------------------------------ | ------------------------ |
| `none` | no account row                 | no `Plan:` / `Account:`  |
| `plan` | `PLAN: Plus`                   | `Plan:`                  |
| `full` | `PLAN: Plus · user@example.com` | `Plan:` and `Account:`  |

The default is `full`. Plan names are capitalized at display time (`plus` →
`Plus`, `max` → `Max`); snapshots, the quota cache, history and `--json` keep the
provider's raw value. Config v4 replaced the earlier `show_account = true |
false` with this setting: `true` migrates to `full`, `false` to `none`, and the
rewritten file no longer contains `show_account`. Set it from Settings (cycles
`none → plan → full`) or with `tokenpulse config set account_display=<value>`.

## Observation history

The cache in `cache.rs` keeps exactly one row per provider and overwrites it on
every poll, so it answers "what is the balance now" and nothing else. Durable
history is appended separately by `core/src/history/`, into the same
`~/.local/share/tokenpulse/tokenpulse.db` (WAL, `PRAGMA user_version = 2`).

Writes hang off `QuotaCacheStore::save`, the one point that knows a fresh
snapshot was observed, so both the TUI's background reload and the plain-text /
JSON commands are covered without per-call-site wiring.

| Table                        | One row per                                             |
| ---------------------------- | ------------------------------------------------------- |
| `quota_observations`         | rate window per poll — provider, `observed_at`, `fetched_at`, plan, account, `window_label`, `model_family`, `used_percent`, `resets_at`, `period_duration_ms` |
| `quota_credit_observations`  | poll, when the provider reports credits                 |
| `quota_fetch_failures`       | failed poll, with the provider attributed               |
| `keeper_executions`          | Keeper ping — agent, trigger, model, prompt, command, duration, exit code, output |

Every poll is written, including unchanged values, so the series is an evenly
spaced grid that needs no gap filling to query. `used_percent` is stored at full
precision; the TUI rounds only for display.

`fetch_all` returns bare `Result`s with no provider id, so failures are
attributed by zipping against `commands::quota::quota_fetcher_ids`, which
reproduces the fetcher order.

There is no retention policy — the tables grow until pruned by hand.

## Error Handling

- Auth file not found → skip provider, show "Not configured"
- Token refresh fails → show "Auth expired, run `claude` / `codex` to re-login"
- API error → show status code and message
- Network timeout → 10 second timeout per provider

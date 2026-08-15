# Quota Module - Detailed Design

## Overview

On-demand fetching of remaining usage quota from coding agent APIs. No polling.

## Architecture

```
quota/
├── mod.rs          # QuotaFetcher trait, QuotaSnapshot struct, fetch_all()
├── claude.rs       # Claude Code quota fetcher
├── codex.rs        # Codex quota fetcher
├── copilot.rs      # GitHub Copilot quota fetcher
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

## GitHub Copilot

### Credential Flow
1. Check `GITHUB_TOKEN` environment variable
2. Fallback: `gh auth token` CLI command
3. Fallback: `~/.config/github-copilot/hosts.json` or `apps.json` → `oauth_token` field

### Quota API
```
GET https://api.github.com/copilot_internal/user
Authorization: token <token>
Editor-Version: vscode/1.100.0
Editor-Plugin-Version: copilot/1.300.0
User-Agent: GitHubCopilotChat/1.300.0
X-Github-Api-Version: 2025-04-01
```

Note: Uses `token` auth scheme, NOT `Bearer`.

### Paid Tier Response
```json
{
  "copilot_plan": "business",
  "quota_reset_date": "2025-08-01T00:00:00Z",
  "quota_snapshots": {
    "completions": { "percent_remaining": 75.0, "entitlement": 1000 },
    "premium_requests": { "percent_remaining": 50.0, "entitlement": 500 }
  }
}
```

### Free Tier Response
```json
{
  "copilot_plan": "free",
  "limited_user_quotas": { "chat_completions": 40.0 },
  "monthly_quotas": { "chat_completions": 100.0 },
  "limited_user_reset_date": "2025-08-01"
}
```

### Response Mapping
| Tier | Calculation                                                        |
| ---- | ------------------------------------------------------------------ |
| Paid | `used_percent = (100 - percent_remaining).clamp(0, 100)`           |
| Free | `used_percent = ((total - remaining) / total * 100).clamp(0, 100)` |

## Antigravity

### Credential Flow
No external auth lookup. Antigravity quota is read from a running local Antigravity language server.

### Quota Probe
1. Discover running Antigravity CLI/Desktop language server processes
2. Prefer CLI LS, then Desktop LS, then unknown Antigravity LS processes
3. Send a Connect-RPC `RetrieveUserQuotaSummary` request to the local language server; on success, make a best-effort `GetUserStatus` call for account email + plan name
4. Do not use OAuth files, keyring lookups, or direct Cloud Code HTTP for Antigravity quota

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

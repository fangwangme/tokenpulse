# Quota Module - Detailed Design

## Overview

On-demand fetching of remaining usage quota from coding agent APIs. No polling.

## Architecture

```
quota/
├── mod.rs          # QuotaFetcher trait, QuotaSnapshot struct, fetch_all()
├── claude.rs       # Claude Code quota fetcher
└── codex.rs        # Codex quota fetcher
```

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
1. Read `~/.claude/.credentials.json` → `claudeAiOauth` object
2. Fallback: macOS Keychain `"Claude Code-credentials"`
3. Check `expiresAt` — if within 5 minutes, refresh token
4. Refresh: `POST https://platform.claude.com/v1/oauth/token`
   - `grant_type=refresh_token`
   - `client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e`
   - `refresh_token=<token>`
5. Save refreshed token back to credentials file

### Quota API
```
GET https://api.anthropic.com/api/oauth/usage
Authorization: Bearer <access_token>
anthropic-beta: oauth-2025-04-20
```

### Response Mapping
| API Field | → RateWindow |
|---|---|
| `five_hour.utilization` | Session (5h), used_percent = utilization * 100 |
| `seven_day.utilization` | Weekly (7d) |
| `seven_day_sonnet.utilization` | Sonnet |
| `seven_day_opus.utilization` | Opus |
| `extra_usage.used / extra_usage.limit` | Credits |

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

### Response Mapping
| API Field | → RateWindow |
|---|---|
| `rate_limit.primary_window.used_percent` | Session |
| `rate_limit.secondary_window.used_percent` | Weekly |
| `credits.balance` | Credits |
| `plan_type` | plan field |

Also check response headers: `x-codex-primary-used-percent`, `x-codex-secondary-used-percent`

## Concurrent Fetching

All providers fetched in parallel via `tokio::join!`:

```rust
pub async fn fetch_all(providers: &[Box<dyn QuotaFetcher>]) -> Vec<Result<QuotaSnapshot>> {
    let futures: Vec<_> = providers.iter().map(|p| p.fetch_quota()).collect();
    futures::future::join_all(futures).await
}
```

## Error Handling

- Auth file not found → skip provider, show "Not configured"
- Token refresh fails → show "Auth expired, run `claude` / `codex` to re-login"
- API error → show status code and message
- Network timeout → 10 second timeout per provider

# Quota Module

Quota reads each agent's existing login and the provider's read-only usage API.
Binding behavior is in [`../specs/quota.md`](../specs/quota.md) and
[`../specs/quota-providers.md`](../specs/quota-providers.md).

## Providers

| Provider | Source |
| :--- | :--- |
| Claude | macOS Keychain or `~/.claude/.credentials.json`, then `GET /api/oauth/usage` |
| Codex | `$CODEX_HOME/auth.json`, `~/.config/codex/auth.json` or `~/.codex/auth.json`, then the `wham` usage and reset-credit endpoints |
| OpenCode Go | `$OPENCODE_DB`, or `opencode.db` / `opencode-<channel>.db` under `${XDG_DATA_HOME:-~/.local/share}/opencode/`, then `GET https://opencode.ai/inference/go/v1/usage` |
| Antigravity | running language server, `agy` CLI ≥1.4, then Cloud Code with the cached OAuth token |

Credentials are never refreshed or written. An expired login is returned as a
provider error telling the user to open that agent.

## Snapshot

A snapshot has plan, account, ordered windows (`label`, `family`, used percent,
period, reset time, API id), optional credits/extra usage and reset credits.
Windows are grouped by family in first-named order, then by period ascending
with unknown last. The API id comes from the provider bucket id, else a stable
family/label slug.

`quota.md` defines the level (normal/warn/exhausted) and pace
(on_track/ahead/behind) rules. The API exposes the same shape with provider
`agy` for Antigravity.

## Cache and history

`tokenpulse.db` stores one snapshot per provider (`quota_cache`, five-minute
TTL), append-only `quota_observations`, `quota_credit_observations`, and
`quota_fetch_failures`, plus `keeper_executions`. A failed fetch keeps the last
snapshot next to the new error. Reads that are not a refresh never call a
provider.

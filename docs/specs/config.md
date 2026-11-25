# Config

`config.toml` lives in the data directory (default
`~/.local/share/tokenpulse/config.toml`; `TOKENPULSE_CONFIG_PATH` overrides the
file alone). The current schema version is 5. A missing file is written with
the defaults. An older version is upgraded in place. Missing Keeper agents and
empty Keeper commands are filled from the defaults. A file that does not parse
is never overwritten: `config set`, `enable`, `disable`, the Keeper toggles and
`save` all fail and leave it byte-identical.

## Keys and defaults

| Key | Default | Values |
| :--- | :--- | :--- |
| `version` | 5 | integer |
| `providers.<id>.enabled` | true for claude, codex, antigravity | boolean |
| `providers.<id>.path` | absent | path (quota config only) |
| `display.show_empty_providers` | false | boolean |
| `display.theme` | `auto` | `auto`, `dark`, `light` |
| `display.quota_display_mode` | `remaining` | `used`, `remaining` |
| `display.auto_refresh_secs` | 300 | 0, 60, 120, 300, 600, 900 (0 = off) |
| `display.account_display` | `full` | `none`, `plan`, `full` |
| `display.scan_antigravity` | true | boolean |
| `display.refresh_quota` | true | boolean |
| `display.notification_level` | `system` | `off`, `in_app`, `terminal`, `system` |
| `display.notification_sound` | `chime` | `chime`, `none`, or a macOS system sound name |
| `keeper.enabled` | false | boolean |
| `keeper.check_interval_secs` | 60 | seconds |
| `keeper.agents.<agent>.session_keeper_enabled` | true | boolean |
| `keeper.agents.<agent>.daily_wakeup_time` | `10:30` | `HH:MM` |
| `keeper.agents.<agent>.weekly_keeper_enabled` | true | boolean |
| `keeper.agents.<agent>.command` | see below | shell template |
| `keeper.agents.<agent>.prompt` | `Hi` | text |
| `keeper.agents.<agent>.weekly_all_groups` | false | boolean, Antigravity only |

Default Keeper agents are `claude`, `codex` and `antigravity`. Their commands:

```text
claude      claude -p "{prompt}" --model {model} --no-session-persistence
codex       codex exec --skip-git-repo-check --ephemeral --model {model} "{prompt}"
antigravity agy --model {model} --prompt "{prompt}"
```

`providers` is the quota map. An entry without a quota fetcher (a legacy
`copilot`, a hand-written `gemini`) is ignored, not removed. Usage sources are
not configured here.

## CLI values

`tokenpulse config set KEY=VALUE` accepts `quota_display_mode`,
`show_empty_providers`, `account_display`, `theme`, `auto_refresh_interval`
(minutes 0/1/2/5/10/15), `refresh_quota`, `notification_level`,
`notification_sound`, `keeper_engine` and `keeper_antigravity_groups`
(`gemini_flash`/`all`). Any other key or value is rejected with the accepted
values. `config enable`/`disable` accept exactly `claude`, `codex` and
`antigravity`.

## Keeper state

`keeper_state.json` next to the config holds `daily_triggered`
(agent → `YYYY-MM-DD`), `weekly_triggered` (target key → RFC3339) and
`weekly_failed` (target keys). A missing or corrupt file reads as empty state.

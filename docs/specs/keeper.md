# Keeper

The Keeper drives agent CLIs with tiny prompts so rate-limit windows start
when the user wants them to. It fires three kinds of ping: a 5h daily
wakeup at a configured time, a weekly ping after a weekly quota window
resets, and a manual ping (`p` on the Keeper tab). Every ping spends real
quota, so the master switch `keeper_engine` defaults to off.

Model resolution is fail-fast: a model that cannot be resolved fails the
ping, and is recorded as a failed execution. There is no hardcoded fallback
model, because a stale default would silently ping a retired model.

## Weekly targets

A weekly target is one weekly quota window the Keeper keeps anchored. Each
target has:

- a **key**, which names its last-fired time in `keeper_state.json` and its
  in-flight lock. Only one ping per key runs at a time;
- a **group**: the `model_family` of the weekly window it follows, or none,
  meaning any weekly window of the agent.

A weekly ping fires for a target when its weekly window is observed to go
from used (> 0%) back to 0% (at most once per 12 hours per key). It also
fires as a fallback once `resets_at + 1 minute` has passed without a ping
for that cycle.

### Baseline

The "used" side of that check is the **baseline**: the last quota snapshot
the Keeper evaluated for the agent. It is persisted in `keeper_state.json`
(`weekly_baseline`, keyed by agent) and replaced after every quota refresh
the TUI receives, so a weekly reset that happens while TokenPulse is not
running, including an early reset by the provider, is observed on the first
refresh after it starts. Without it the fallback cannot catch such a reset:
once a window has reset, the snapshot's `resets_at` already points at the
next cycle.

- Only the TUI Keeper writes the baseline. Quota fetched by other commands
  (plain-text and JSON output) also updates the quota cache, but not the
  baseline, so it cannot consume a reset the Keeper has not seen.
- The baseline has no age limit: a used → 0% change after any gap means the
  window reset. The 12-hour per-key guard still applies.
- With no baseline for the agent (first run, or a `keeper_state.json` from
  an older version), the first refresh fires nothing and becomes the
  baseline. The file's existing trigger times still load.

Claude and Codex always have exactly one target, keyed by the agent name,
with no group.

## Antigravity quota groups

Antigravity meters quota per model group. The quota API returns each group
as a set of windows with that group's name as `model_family`. Today these
are `Gemini`, `Claude`, and `Gemini 4 Argon`, each with its own weekly
reset time. Groups are taken from the snapshot, never from a hardcoded
list, so a new group the API starts reporting is covered automatically.

### Setting

- Config: `[keeper.agents.antigravity] weekly_all_groups`, a boolean,
  default `false`. An existing config without the key reads as `false`.
- Settings row and `tokenpulse config set` key: `keeper_antigravity_groups`,
  values `gemini_flash` (false) and `all` (true). Any other value is
  rejected with the list of accepted values. `tokenpulse config show`
  reports the current value.
- Only Antigravity reads this setting.

### Targets

The groups are the distinct `model_family` values among the Antigravity
snapshot's weekly windows, in snapshot order. The first is the **primary
group** (`Gemini`, which the quota fetcher sorts first).

- `gemini_flash` (default): one target, key `antigravity`, the primary
  group. A weekly reset in any other group fires nothing.
- `all`: one target per group. The primary group keeps key `antigravity`,
  and every other group uses `antigravity:<group>` (e.g.
  `antigravity:Gemini 4 Argon`). Each target is evaluated against its own
  group's weekly window and reset time, independently of the others.
- With no snapshot, or one without weekly windows, there is one target,
  key `antigravity`, with no group.

The primary group keeps the key it always had, so switching the setting
either way neither replays nor double-fires a ping.

The manual ping follows the same setting: one ping for the primary group,
or one ping per group.

The 5h daily wakeup is not affected and always pings the primary group's
model.

### Model per group

- **Primary group** (and the no-group target): the agent's regular
  resolver, the newest Gemini Flash Low from the local alias history.
- **Every other group**: chosen when the ping fires, from the output of
  `agy models`. Each line of that output is `<model id>\t<label>`; lines
  that do not have this shape are ignored.
  - A model belongs to the group whose slug is the longest prefix of the
    model id among the snapshot's groups. The slug is the group name
    lowercased, with whitespace runs replaced by `-` (`Gemini 4 Argon` →
    `gemini-4-argon`). So Argon models never count as `Gemini` models while
    an Argon group exists.
  - Candidates are ranked by:
    1. the cheapest tier: an id containing `flash`, `haiku` or `lite`
       comes first, an id containing `opus` comes last, and everything else
       is in between;
    2. the newest version, comparing the numeric runs in the id;
    3. the lowest thinking level: `-low` first, then no level suffix, then
       `-medium`, then `-high`.
  - For example, `Claude` resolves to `claude-sonnet-5-5-low` and
    `Gemini 4 Argon` resolves to `gemini-4-argon-low`. `gpt-oss-*` is not a
    candidate for `Claude`, because it does not match the `claude` slug.
  - If `agy models` cannot run or exits with an error, or no model matches
    the group, the ping fails with that reason. No other model is tried.

### Keeper card

"Weekly Sync: Next" shows a single time: the earliest pending trigger
among the agent's targets. In `all` mode the group's name follows the
time. The card shows a ping as running while any of the agent's targets
holds its lock.

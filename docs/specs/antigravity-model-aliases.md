# Antigravity Model Aliases

How TokenPulse names the model behind each Antigravity generation, and the
evidence behind the seeded aliases.

## Resolution

Every generation reports two identities:

- **enum** — `model_enum` in local `gen_metadata` (field 20) or `chatModel.model`
  over RPC, e.g. `MODEL_PLACEHOLDER_M319`. Stable: the model the user picked.
- **served name** — field 19 locally, `responseModel` over RPC. The backend's
  internal id, sometimes a codename (`gemini-3.8-flash-n` serves Gemini 3.8
  Flash) or a routing variant (`gemini-3.6-flash-tiered`).

`resolve_antigravity_model` takes the first that applies:

1. the enum's alias, when the alias table names it;
2. the served name, unless it is a pseudo id (`gemini-default`);
3. the raw enum (`model-placeholder-m319`);
4. `unknown`.

Both raw identities are stored on each cached usage row (`model_enum`,
`served_model`) and re-resolved on every read. A session whose id changes is
re-stamped, so an alias learned later corrects history in the cache and, via
the incremental ingest, in the ledger.

An alias's id comes from its **label**: `Gemini 3.1 Pro (High)` →
`gemini-3-1-pro-high`. Every Antigravity id gets spelling normalization only —
lowercase, no `antigravity-` prefix, `.`/`_`/spaces as single `-`. Variants
such as `thinking` or `high` stay; `model_id::canonical` folds them (and
`-tiered`, `preview`) for display, and pricing applies the same spelling rules
to catalog keys (`docs/model-pricing-mapping.md`). There are no per-model
rules.

## Alias table

Merged in this order, later entries winning:

1. static seeds (`STATIC_MODEL_ALIASES`: enum, label, evidence);
2. the history file from previous runs;
3. the live `GetUserStatus` list.

`GetUserStatus` pairs each `clientModelConfigs[].modelOrAlias.model` with its
`label`; that pairing is the evidence, so any label is accepted. Each sync
writes the merged table to
`~/.local/share/tokenpulse/antigravity-cache/model-aliases.json`, keeping
`firstSeenAt` and updating `lastSeenAt`. On release, fold its new entries into
`STATIC_MODEL_ALIASES` so fresh installs resolve them offline. Add a seed only
with evidence: a captured `GetUserStatus`, local generation metadata, another
project, or a public source.

```json
{
  "version": 1,
  "updatedAt": "2026-05-20T10:55:00Z",
  "aliases": {
    "model_placeholder_m26": {
      "rawModelId": "MODEL_PLACEHOLDER_M26",
      "modelId": "claude-opus-4-6-thinking",
      "label": "Claude Opus 4.6 (Thinking)",
      "source": "antigravity-get-user-status",
      "firstSeenAt": "2026-05-20T10:55:00Z",
      "lastSeenAt": "2026-05-20T10:55:00Z"
    }
  }
}
```

Keys are lowercase; lookups also try dashes as underscores, so
`MODEL_PLACEHOLDER_M26`, `model-placeholder-m26` and `model_placeholder_m26`
match the same entry. `modelId` is informational — the label decides the id.

## Seeds

| Enum | Label | Evidence |
| --- | --- | --- |
| `M26` | Claude Opus 4.6 (Thinking) | `openusage`; Antigravity Mobility CLI article; `GetUserStatus` |
| `M35` | Claude Sonnet 4.6 (Thinking) | Antigravity Mobility CLI article; `GetUserStatus` |
| `M12` | Claude Opus 4.5 (Thinking) | user-provided |
| `MODEL_CLAUDE_4_5_SONNET` | Claude Sonnet 4.5 | user-provided |
| `M36` / `M37` | Gemini 3.1 Pro (Low) / (High) | Antigravity Mobility CLI article; Tokscale; `GetUserStatus` |
| `M16` | Gemini 3.1 Pro (High) | `GetUserStatus` |
| `M7` / `M8` / `M9` | Gemini 3 Pro (Low) / (High) / (Image) | user-provided |
| `M18` / `M47` | Gemini 3 Flash | user-provided; M47 also Antigravity Mobility CLI article, Tokscale |
| `M132` / `M20` / `M187` | Gemini 3.5 Flash (High) / (Medium) / (Low) | `GetUserStatus` |
| `M71` / `M72` / `M73` | Gemini 3.6 Flash (High) / (Medium) / (Low) | `GetUserStatus` |
| `M196` / `M264` | Gemini 3.6 Flash | sub-agent enums absent from `GetUserStatus`; field 19 is always `gemini-3.6-flash[-tiered]` |
| `M298` / `M299` / `M300` | Gemini 3.7 Flash (High) / (Medium) / (Low) | `GetUserStatus` |
| `M318` / `M319` / `M320` | Gemini 3.8 Flash (High) / (Medium) / (Low) | `GetUserStatus` |
| `MODEL_OPENAI_GPT_OSS_120B_MEDIUM` | GPT-OSS 120B (Medium) | Antigravity Mobility CLI article; Tokscale; `GetUserStatus` |

`Mn` stands for `MODEL_PLACEHOLDER_Mn`.

## Sources

- `tokscale` (commit `270d64c`): `crates/tokscale-core/src/pricing/aliases.rs`,
  `crates/tokscale-core/src/sessions/antigravity.rs`.
- `openusage` model notes: M26 is "Claude Opus 4.6 (Thinking)".
- Antigravity Mobility CLI article: display names for M26, M35, M36, M37, M47
  and `MODEL_OPENAI_GPT_OSS_120B_MEDIUM`.
- `opencode-antigravity-auth` API spec: human-readable ids such as
  `claude-opus-4-6-thinking`.

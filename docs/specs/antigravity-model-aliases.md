# Antigravity Model Aliases

This document records how TokenPulse turns what Antigravity reports for each
generation into a model id, and the evidence behind the seeded aliases.

## Rule

Every generation carries two identities:

- the **enum** — `model_enum` in the local `gen_metadata` blob (field 20), or
  `chatModel.model` over RPC — such as `MODEL_PLACEHOLDER_M319`. It is
  Antigravity's stable identity for the model the user picked;
- the **served name** — field 19 locally, `responseModel` over RPC — the
  backend's internal id for what actually ran. It is sometimes a release
  codename (`gemini-3-flash-a` served Gemini 3.5 Flash, `gemini-3.8-flash-n`
  serves Gemini 3.8 Flash) or a routing variant (`gemini-3.6-flash-tiered`).

Resolution order, in `resolve_antigravity_model`:

1. The enum, when the alias table names it.
2. The served name, unless it is a pseudo id such as `gemini-default`.
3. The raw enum (`model-placeholder-m319`).
4. `unknown`.

Both raw identities are stored with each cached usage row (`model_enum`,
`served_model`). Every read re-resolves them against the aliases known at that
point and re-stamps any session whose id changed, so a later-learned alias
corrects history in the cache and, through the incremental ingest, in the
ledger. There are no per-model rules: no codename mappings, version lists, or
`-preview` lists.

An alias's model id comes from its **label**, never from a hand-written id:
`(` and `)` become spaces, then the id is spelling-normalized —
`Gemini 3.1 Pro (High)` → `gemini-3-1-pro-high`. Spelling normalization is the
only transformation applied to any Antigravity id: lowercase, strip the
`antigravity-`/`anti-gravity-` prefix, turn `.`, `_` and spaces into `-`, and
collapse repeated hyphens. Meaningful variants such as `thinking`, `high`, and
`low` stay in the cache. Grouping and pricing interpret the id later:
`model_id::canonical` strips tier, routing (`-tiered`) and `preview` segments
for display, and pricing applies the same spelling rules to catalog keys (see
`docs/model-pricing-mapping.md`).

The alias table is merged in this order, later entries winning:

1. Static seeds (`STATIC_MODEL_ALIASES`): enum, label and evidence source.
2. The persisted history ledger from previous runs.
3. Current online `GetUserStatus` aliases.

`GetUserStatus` returns `clientModelConfigs[]` entries pairing the internal
`modelOrAlias.model` with its user-facing `label`. Any label is accepted; the
pairing itself is the evidence, so a placeholder is only ever connected to a
model when Antigravity returns the pair. Static seeds cover offline syncs, fresh
installs, and enums that no longer appear in the live list.

TokenPulse persists every observed mapping in
`~/.local/share/tokenpulse/antigravity-cache/model-aliases.json`. Each sync seeds
it from the static table and merges the current `GetUserStatus` list into it,
preserving `firstSeenAt` and updating `lastSeenAt`. Fold its new entries back
into `STATIC_MODEL_ALIASES` on release so fresh installs resolve them too.

Do not add a static seed unless a captured `GetUserStatus` response, local
generation metadata, another project, or a public source backs it.

## History file format

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

Keys are lowercase. Lookups try the key as given and with dashes converted to
underscores, so `MODEL_PLACEHOLDER_M26`, `model-placeholder-m26`, and
`model_placeholder_m26` resolve to the same entry. `modelId` is informational:
the label decides the id, so entries written by older releases are re-derived.

## Seed aliases

| Enum | Label | Evidence |
| --- | --- | --- |
| `MODEL_PLACEHOLDER_M26` | Claude Opus 4.6 (Thinking) | `openusage`; Antigravity Mobility CLI article; captured `GetUserStatus`. |
| `MODEL_PLACEHOLDER_M35` | Claude Sonnet 4.6 (Thinking) | Antigravity Mobility CLI article; captured `GetUserStatus`. |
| `MODEL_PLACEHOLDER_M12` | Claude Opus 4.5 (Thinking) | User-provided initial mapping. |
| `MODEL_CLAUDE_4_5_SONNET` | Claude Sonnet 4.5 | User-provided initial mapping. |
| `MODEL_PLACEHOLDER_M36` | Gemini 3.1 Pro (Low) | Antigravity Mobility CLI article; captured `GetUserStatus`. |
| `MODEL_PLACEHOLDER_M37` | Gemini 3.1 Pro (High) | Antigravity Mobility CLI article; Tokscale. |
| `MODEL_PLACEHOLDER_M16` | Gemini 3.1 Pro (High) | Captured `GetUserStatus` (Antigravity 2.0.1). |
| `MODEL_PLACEHOLDER_M7` / `M8` / `M9` | Gemini 3 Pro (Low) / (High) / (Image) | User-provided initial mapping. |
| `MODEL_PLACEHOLDER_M18` / `M47` | Gemini 3 Flash | User-provided initial mapping; M47 also Antigravity Mobility CLI article and Tokscale. |
| `MODEL_PLACEHOLDER_M132` / `M20` / `M187` | Gemini 3.5 Flash (High) / (Medium) / (Low) | Captured `GetUserStatus`. |
| `MODEL_PLACEHOLDER_M71` / `M72` / `M73` | Gemini 3.6 Flash (High) / (Medium) / (Low) | Captured `GetUserStatus`. |
| `MODEL_PLACEHOLDER_M196` / `M264` | Gemini 3.6 Flash | Sub-agent routing enums absent from `GetUserStatus`; every local generation carrying them names `gemini-3.6-flash[-tiered]` in field 19. |
| `MODEL_PLACEHOLDER_M298` / `M299` / `M300` | Gemini 3.7 Flash (High) / (Medium) / (Low) | Captured `GetUserStatus`. |
| `MODEL_PLACEHOLDER_M318` / `M319` / `M320` | Gemini 3.8 Flash (High) / (Medium) / (Low) | Captured `GetUserStatus`. |
| `MODEL_OPENAI_GPT_OSS_120B_MEDIUM` | GPT-OSS 120B (Medium) | Antigravity Mobility CLI article; Tokscale; captured `GetUserStatus`. |

## Sources checked

- Local `tokscale` clone at `/private/tmp/tokscale`, commit
  `270d64c4d268d5bcc380690441d6a891687d6794`:
  `crates/tokscale-core/src/pricing/aliases.rs` and
  `crates/tokscale-core/src/sessions/antigravity.rs`.
- `openusage` model notes: `MODEL_PLACEHOLDER_M26` is "Claude Opus 4.6
  (Thinking)".
- Antigravity Mobility CLI article: lists `MODEL_PLACEHOLDER_M37`,
  `MODEL_PLACEHOLDER_M36`, `MODEL_PLACEHOLDER_M47`, `MODEL_PLACEHOLDER_M35`,
  `MODEL_PLACEHOLDER_M26`, and `MODEL_OPENAI_GPT_OSS_120B_MEDIUM` with display
  names.
- Antigravity Token Monitor marketplace page: confirms the same approach of
  resolving `MODEL_PLACEHOLDER_*` IDs to human-readable names before JSONL
  serialization, with `responseModel` preferred when present.
- `opencode-antigravity-auth` API spec: confirms human-readable Antigravity
  model IDs such as `claude-sonnet-4-6` and `claude-opus-4-6-thinking`.

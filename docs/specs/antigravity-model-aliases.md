# Antigravity Model Aliases

This document records the evidence used for Antigravity model ID normalization.
These aliases are applied when writing or normalizing TokenPulse's Antigravity
cache artifacts, before usage rows are ingested.

## Rule

The cache should store meaningful Antigravity model names, not opaque internal
placeholder IDs. This step does not perform display aggregation or pricing
canonicalization. Meaningful variants such as `thinking`, `high`, and `low`
should remain in the cache; later usage summary/model-table code can aggregate
those variants when needed.

A trailing `-tiered` segment is the exception: it is a routing suffix used by
sub-agent generators, not a meaningful model variant. Sub-agent metadata such as
`{"responseModel": "gemini-3.6-flash-tiered", "model": "MODEL_PLACEHOLDER_M196"}`
still resolves through `responseModel`, but the final `-tiered` is dropped so the
cache stores the base model (`gemini-3-6-flash`). Only a final `-tiered` segment
is removed; occurrences elsewhere in a model ID are preserved. The shared usage
canonicalizer and the pricing candidate normalizer strip the same suffix, so
display aggregation and pricing lookup stay aligned with the cache.

The preferred source of truth is the running Antigravity language server:
`GetUserStatus` returns `clientModelConfigs[]` entries containing both the
user-facing `label` and the internal `modelOrAlias.model`. TokenPulse should use
that dynamic mapping first when syncing cache artifacts. Labels from known model
families (`Claude`, `Gemini`, and `GPT-OSS`) are converted mechanically into
cache IDs, preserving meaningful suffixes such as `thinking`, `high`, and `low`.
This is not a guessed placeholder mapping: the placeholder is only connected to
the model when Antigravity itself returns the pair in `GetUserStatus`.

The effective cache-write dictionary is built in this order:

1. Static evidence-backed seed aliases from this document and the code table.
2. The persisted historical ledger from previous runs.
3. Current online `GetUserStatus` aliases, which override older entries.

This merged dictionary is written back to the ledger on sync. Static aliases are
fallbacks for offline syncs, legacy cache normalization, or old sessions whose
placeholder no longer appears in the live model list.

TokenPulse persists every dynamically observed mapping in:

`~/.local/share/tokenpulse/antigravity-cache/model-aliases.json`

That file is a historical mapping ledger. Each sync seeds it from the static
table and merges the current `GetUserStatus` model list into the ledger,
preserving `firstSeenAt` and updating `lastSeenAt`. Cache normalization reads
the merged ledger, so old sessions can still be resolved after Antigravity
removes or renames a model in the active list.

Do not add a static placeholder mapping unless it is backed by another project,
a public source, or a captured Antigravity `GetUserStatus` response. Unknown
placeholders stay unchanged.

TokenScale handles unresolved Antigravity IDs at pricing/display time, not at
cache-write time. Its Antigravity-specific alias table only covers
`MODEL_PLACEHOLDER_M26`, `MODEL_PLACEHOLDER_M35`, `MODEL_PLACEHOLDER_M36`,
`MODEL_PLACEHOLDER_M37`, `MODEL_PLACEHOLDER_M47`, and
`MODEL_OPENAI_GPT_OSS_120B_MEDIUM`. If a placeholder is not in that table,
TokenScale keeps the raw model ID, then pricing lookup tries exact,
normalized, prefix/suffix-stripped, and fuzzy matches. If all fail, no pricing
is applied and display code can only format the raw string. It does not contain
a historical mapping ledger for unresolved placeholders such as
`MODEL_PLACEHOLDER_M7`, `MODEL_PLACEHOLDER_M8`, `MODEL_PLACEHOLDER_M12`, or
`MODEL_PLACEHOLDER_M18`.

## End-to-end flow

1. Antigravity sync detects the running language server and calls
   `GetUserStatus`.
2. TokenPulse builds the alias dictionary from static seeds, historical ledger,
   and current online aliases.
3. Session metadata is written to the TokenPulse Antigravity cache with
   meaningful model IDs such as `claude-opus-4-6-thinking`,
   `gemini-3.1-pro-preview-high`, or `gemini-3.5-flash-medium`, not
   `MODEL_PLACEHOLDER_*`.
4. The scanner reads the cache and stores usage rows.
5. Usage/model summaries group variants by normalizing provider prefixes,
   quality suffixes, `free`, and `thinking` suffixes. This is where
   `claude-opus-4-6-thinking` becomes `claude-opus-4-6` for the Models tab.
6. Pricing lookup uses the raw usage model and provider hint against the merged
   pricing catalog from LiteLLM, OpenRouter, and models.dev. Pricing aliases may
   remove Antigravity prefixes or quality suffixes, but must not map one real
   model version to another. For example, `claude-opus-4-6` must not use
   `claude-opus-4-5` pricing, and `gemini-3.1-pro` must not use
   `gemini-3-pro-preview` pricing.

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

Keys are normalized to lowercase with dashes converted to underscores so
`MODEL_PLACEHOLDER_M26`, `model-placeholder-m26`, and
`model_placeholder_m26` resolve to the same history entry.

## Adopted mappings

| Raw Antigravity ID | Cache model ID | Evidence |
| --- | --- | --- |
| `MODEL_PLACEHOLDER_M26` | `claude-opus-4-6-thinking` | `openusage` lists this internal ID as "Claude Opus 4.6 (Thinking)"; Antigravity Mobility CLI article lists the same. Tokscale confirms this placeholder belongs to Claude Opus 4.6, but collapses thinking for pricing. |
| `MODEL_PLACEHOLDER_M35` | `claude-sonnet-4-6-thinking` | Antigravity Mobility CLI article lists this internal ID as "Claude Sonnet 4.6 (Thinking)". Tokscale confirms the placeholder belongs to Claude Sonnet 4.6, but collapses thinking for pricing. |
| `MODEL_PLACEHOLDER_M36` | `gemini-3.1-pro-preview-low` | Antigravity Mobility CLI article lists this internal ID as "Gemini 3.1 Pro (Low)"; TokenPulse keeps `preview` because Gemini 3.1 Pro is priced and displayed as the preview model family. |
| `MODEL_PLACEHOLDER_M37` | `gemini-3.1-pro-preview-high` | Antigravity Mobility CLI article lists this internal ID as "Gemini 3.1 Pro (High)"; TokenPulse keeps `preview` because Gemini 3.1 Pro is priced and displayed as the preview model family. |
| `MODEL_PLACEHOLDER_M47` | `gemini-3-flash-preview` | Antigravity Mobility CLI article lists this internal ID as "Gemini 3 Flash"; Tokscale maps M47 to `gemini-3-flash-preview`. |
| `MODEL_OPENAI_GPT_OSS_120B_MEDIUM` | `gpt-oss-120b-medium` | Antigravity Mobility CLI article lists this internal ID as "GPT-OSS 120B (Medium)"; Tokscale maps the same placeholder to `gpt-oss-120b-medium`. |
| `MODEL_PLACEHOLDER_M132` | `gemini-3.5-flash-high` | Captured local Antigravity 2.0.1 `GetUserStatus` response. |
| `MODEL_PLACEHOLDER_M20` | `gemini-3.5-flash-medium` | Captured local Antigravity 2.0.1 `GetUserStatus` response. |
| `MODEL_PLACEHOLDER_M16` | `gemini-3.1-pro-preview-high` | Captured local Antigravity 2.0.1 `GetUserStatus` response; normalized with `preview` for the same model-family rule as M37. |
| `gemini-3-flash-a` | `gemini-3.5-flash` | Product/runtime finding for this branch: Antigravity reports this internal Flash A ID for the Gemini 3.5 Flash model. Public sources confirm Gemini 3.5 Flash availability in Antigravity, but no public source found for the internal `gemini-3-flash-a` ID. Keep this mapping isolated and revisit when Antigravity publishes or another project records the ID. |

## Dynamic mappings captured from Antigravity

On 2026-05-20, a local Antigravity 2.0.1 language server returned these
`GetUserStatus` model configs:

| `modelOrAlias.model` | `label` | Cache model ID |
| --- | --- | --- |
| `MODEL_PLACEHOLDER_M36` | `Gemini 3.1 Pro (Low)` | `gemini-3.1-pro-low` |
| `MODEL_PLACEHOLDER_M35` | `Claude Sonnet 4.6 (Thinking)` | `claude-sonnet-4-6-thinking` |
| `MODEL_PLACEHOLDER_M26` | `Claude Opus 4.6 (Thinking)` | `claude-opus-4-6-thinking` |
| `MODEL_OPENAI_GPT_OSS_120B_MEDIUM` | `GPT-OSS 120B (Medium)` | `gpt-oss-120b-medium` |
| `MODEL_PLACEHOLDER_M132` | `Gemini 3.5 Flash (High)` | `gemini-3.5-flash-high` |
| `MODEL_PLACEHOLDER_M20` | `Gemini 3.5 Flash (Medium)` | `gemini-3.5-flash-medium` |
| `MODEL_PLACEHOLDER_M16` | `Gemini 3.1 Pro (High)` | `gemini-3.1-pro-high` |

These rows should be written into `model-aliases.json` by sync. They are also
safe static fallbacks because their source is a captured live Antigravity
response.

## Format-only aliases

These preserve the same model and variant while making the ID consistent enough
for display and provider detection.

| Raw ID | Cache model ID |
| --- | --- |
| `claude-opus-4.6` | `claude-opus-4-6` |
| `claude-sonnet-4.6` | `claude-sonnet-4-6` |
| `claude-haiku-4.6` | `claude-haiku-4-6` |
| `claude-opus-4.6-thinking` | `claude-opus-4-6-thinking` |
| `claude-sonnet-4.6-thinking` | `claude-sonnet-4-6-thinking` |
| `antigravity-claude-opus-4-6-thinking` | `claude-opus-4-6-thinking` |
| `antigravity-claude-sonnet-4-6-thinking` | `claude-sonnet-4-6-thinking` |
| `gemini-3.1-pro-high` | `gemini-3.1-pro-high` |
| `gemini-3.1-pro-low` | `gemini-3.1-pro-low` |
| `gemini-3-pro-high` | `gemini-3-pro-high` |
| `gemini-3-pro-low` | `gemini-3-pro-low` |
| `gemini-3.0-pro-preview-high` | `gemini-3-pro-preview-high` |
| `gemini-3.0-pro-preview-low` | `gemini-3-pro-preview-low` |
| `gemini-3-pro-preview-high` | `gemini-3-pro-preview-high` |
| `gemini-3-pro-preview-low` | `gemini-3-pro-preview-low` |
| `gemini-3-flash-c` | `gemini-3-flash-preview` |
| `gemini-3.6-flash-tiered` | `gemini-3-6-flash` |
| `gemini-3.5-flash-tiered` | `gemini-3.5-flash` |

## Explicitly not mapped

The current local Antigravity data has these placeholders, but no source was
found that identifies them. Leave them as-is until there is evidence:

| Placeholder |
| --- |
| `MODEL_PLACEHOLDER_M7` |
| `MODEL_PLACEHOLDER_M8` |
| `MODEL_PLACEHOLDER_M12` |
| `MODEL_PLACEHOLDER_M18` |

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

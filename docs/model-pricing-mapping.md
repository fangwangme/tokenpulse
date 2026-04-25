# Model Pricing Mapping

This document explains how TokenPulse maps model ids found in local agent logs to pricing keys from the LiteLLM pricing cache.

## Goal

Agent logs are not consistent. The same model can appear as:

- `glm-5.1`
- `glm5.1`
- `z-ai/glm-5.1`
- `zai/glm-5.1`
- `openrouter/z-ai/glm-5.1`

The pricing cache may use another spelling again. The mapping layer should therefore prefer general normalization rules over one-off aliases.

## Lookup Pipeline

`lookup_model_pricing()` builds ordered candidates, then checks each candidate by exact match and case-insensitive match.

The current candidate order is:

1. Raw model id from the ledger.
2. Explicit aliases for model families that cannot be inferred safely.
3. Generalized normalization candidates.
4. `-free` stripped candidates and their normalized forms.
5. Quality-tier suffix normalization where applicable.
6. Common provider-prefix candidates for unprefixed models.
7. Date-suffix stripped candidates and their normalized forms.
8. Slash-to-dot variants for providers that publish keys with dot separators.

This keeps exact pricing preferred, while still recovering from common provider spelling differences.

## GLM / Z.ai Rules

GLM models are handled by a generic canonicalization rule instead of enumerating every released version.

The rule:

1. Normalize `_` to `-`.
2. Recognize bare GLM model ids that begin with `glm`, including compact forms like `glm5.1`.
3. Canonicalize them to `glm-{version-or-suffix}`.
4. Add Z.ai provider candidates:
   - `zai/{canonical_model}`
   - `zai.{canonical_model}`
5. Normalize provider spellings:
   - `z-ai/` -> `zai/`
   - `z.ai/` -> `zai/`

Examples:

| Input model id | Generated pricing candidates |
|---|---|
| `glm5.1` | `glm-5.1`, `zai/glm-5.1`, `zai.glm-5.1` |
| `glm-4.7-free` | `glm-4.7`, `zai/glm-4.7`, `zai.glm-4.7` |
| `z-ai/glm5.1` | `zai/glm5.1`, `zai/glm-5.1`, `zai.glm-5.1` |
| `openrouter/z-ai/glm-5.1` | raw key, `openrouter/zai/glm-5.1`, GLM family candidates |

This means a future `glm-5.2` should resolve automatically as soon as the pricing cache contains a compatible `zai/glm-5.2` or `zai.glm-5.2` key.

## Quality Tier Suffixes

Some coding agents append reasoning or service-tier labels to the model id. These labels should not split model rollups because they describe how the same model was invoked, not a separate base model.

For display and aggregation, TokenPulse strips final tier suffixes:

- `-high`
- `-medium`
- `-low`

Examples:

| Raw model id | Aggregated model name |
|---|---|
| `antigravity-claude-opus-4-5-thinking-high` | `antigravity-claude-opus-4-5-thinking` |
| `gemini-3-pro-medium` | `gemini-3-pro` |
| `z-ai/glm-5.1-low` | `glm-5-1` |

This normalization is intentionally applied only at the end of the model id so names that contain those words in the middle are preserved.

## Built-In Pricing Overrides

LiteLLM can lag behind newly released models. When a model is official but absent from the cache, TokenPulse may add a narrow built-in override in `PricingCache`.

Current override:

| Model | Source | Input / 1M | Cached input / 1M | Output / 1M |
|---|---|---:|---:|---:|
| `GLM-5.1` | Z.ai official pricing | `$1.40` | `$0.26` | `$4.40` |

Reference: Z.ai pricing docs list GLM-5.1 at `$1.4` input, `$0.26` cached input, and `$4.4` output per 1M tokens: https://docs.z.ai/guides/overview/pricing

The override is intentionally separate from mapping. Mapping should be general; pricing data is a source-data concern.

## When To Add Explicit Aliases

Add an explicit alias only when a rule would be unsafe or ambiguous.

Good reasons:

- The logged model id is a product label, not a model id.
- The provider uses a renamed model family with no stable textual relation.
- A routing provider logs a synthetic model id that must resolve to a specific vendor key.

Avoid explicit aliases for simple spelling differences such as:

- Hyphen vs no hyphen (`glm5.1` vs `glm-5.1`)
- Provider spelling (`z-ai` vs `zai`)
- Slash vs dot provider separators

Those should be handled by generalized candidates.

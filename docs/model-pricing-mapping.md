# Model Pricing Mapping

This document explains how TokenPulse maps model ids found in local agent logs to pricing keys from the merged pricing catalog.

## Goal

Agent logs are not consistent. The same model can appear as:

- `glm-5.1`
- `glm5.1`
- `z-ai/glm-5.1`
- `zai/glm-5.1`
- `openrouter/z-ai/glm-5.1`

The pricing cache may use another spelling again. The mapping layer should therefore prefer general normalization rules over one-off aliases.

## Lookup Pipeline

`PricingCatalog::lookup()` normalizes the model id with `model_id::pricing_key`, builds ordered candidates, and normalizes every candidate with the same rule the catalog applied to its keys on insert before checking it. Both sides therefore share one key space: lowercase, `.`/`_`/spaces as `-`, date and tier suffixes stripped, and a three-segment key reduced to provider plus model (`openrouter/google/gemini-3-pro-preview` is stored and looked up as `openrouter/gemini-3-pro-preview`). `pricing_key` keeps `-preview`: catalogs list `X` and `X-preview` separately, sometimes at different prices, so the two must never collapse into one key.

The current candidate order is:

1. The model id itself, then its provider-hinted form.
2. Explicit aliases for model families that cannot be inferred safely.
3. Generalized family candidates — for Gemini: bare, `google/`, `gemini/`, `openrouter/google/`.
4. `-free` stripped candidates and their normalized forms.
5. Quality-tier suffix normalization where applicable.
6. Common provider-prefix candidates for unprefixed models.
7. Date-suffix stripped candidates and their normalized forms.
8. Slash-to-dot variants for providers that publish keys with dot separators.
9. Steps 1–8 again for `<model>-preview`, when the id does not already end in it. Some models are only ever published as previews, and sources that name models from UI labels (Antigravity) never carry the suffix.
10. `vertex_ai/<model>` and `vertex_ai/<model>-preview` for Gemini, last of all. Vertex keeps listing models the first-party and router catalogs have retired, but it must never outrank them.

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
- `-tiered`

`-tiered` is Antigravity's routing suffix for sub-agent generators. It describes
how the request was routed, not a quality tier and not a separate model family,
so it is stripped by the same rule.

Examples:

| Raw model id | Aggregated model name |
|---|---|
| `antigravity-claude-opus-4-5-thinking-high` | `antigravity-claude-opus-4-5-thinking` |
| `gemini-3-pro-medium` | `gemini-3-pro` |
| `gemini-3.6-flash-tiered` | `gemini-3-6-flash` |
| `z-ai/glm-5.1-low` | `z-ai/glm-5.1` |

This normalization is intentionally applied only at the end of the model id so names that contain those words in the middle are preserved.

Display and aggregation (`model_id::canonical`) additionally drop every `preview` segment: vendors attach it arbitrarily, and a GA release is a new version rather than the same model promoted, so `gemini-3-pro-preview-high` and `gemini-3-pro-high` group as `gemini-3-pro`. Pricing does not drop it (see the lookup pipeline above). Stored grouping ids in `usage.db` are re-derived on startup whenever these rules change.

## Pricing Sources

TokenPulse does not hardcode model prices.

Pricing comes from a merged catalog with this priority:

1. LiteLLM
2. models.dev
3. OpenRouter

Mapping stays separate from source data. Normalization decides which lookup keys to try; source priority decides which matched record wins.

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

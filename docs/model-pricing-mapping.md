# Model Pricing Mapping

How TokenPulse maps model ids from agent logs to keys in the merged pricing
catalog. Logs and catalogs spell the same model differently (`glm-5.1`,
`glm5.1`, `z-ai/glm-5.1`, `openrouter/z-ai/glm-5.1`), so general rules are
preferred over one-off aliases.

## Two normalizations

`model_id` defines both; they share the spelling rules — lowercase, last path
segment only, date suffix and `antigravity-` prefix dropped, `.`/`_`/spaces as
`-`, and trailing `-high`, `-medium`, `-low`, `-thinking`, `-tiered` (Antigravity
sub-agent routing) and `-free` stripped.

- **`pricing_key`** — catalog keys and lookups. Keeps `-preview`: catalogs list
  `X` and `X-preview` separately, sometimes at different prices.
- **`canonical`** — display and grouping. Also drops every `preview` segment;
  vendors attach it arbitrarily and a GA release is a new version, not the
  preview promoted. Stored grouping ids in `usage.db` are re-derived on startup
  when these rules change.

| Raw model id | `canonical` | `pricing_key` |
|---|---|---|
| `antigravity-claude-opus-4-5-thinking-high` | `claude-opus-4-5` | `claude-opus-4-5` |
| `gemini-3.1-pro-preview-high` | `gemini-3-1-pro` | `gemini-3-1-pro-preview` |
| `gemini-3.6-flash-tiered` | `gemini-3-6-flash` | `gemini-3-6-flash` |
| `z-ai/glm-5.1-low` | `glm-5-1` | `glm-5-1` |

## Lookup

`PricingCatalog::lookup()` builds ordered candidates from the `pricing_key`,
normalizes each one the way catalog keys were normalized on insert
(`openrouter/google/X` is stored and looked up as `openrouter/X`), and returns
the first usable record:

1. the id itself, then its provider-hinted form;
2. explicit aliases (below);
3. family candidates — Gemini: bare, `google/`, `gemini/`, `openrouter/google/`;
   GLM, Kimi, MiniMax, DeepSeek, Qwen, Claude and GPT have their own;
4. `-free`, tier, date and three-segment-prefix variants, plus slash-to-dot
   forms;
5. steps 1–4 again for `<model>-preview` — some models only ever ship as
   previews, and label-derived ids (Antigravity) never carry the suffix;
6. `vertex_ai/<model>[-preview]` for Gemini — Vertex keeps models other
   catalogs have retired, but never outranks them;
7. the same model under any provider prefix, for models no rule reaches (e.g.
   `muse-spark-1-3-contributor`, listed only as `meta/…` and by resellers):
   highest-priority source first, then the price most listings agree on, so one
   reseller's markup cannot win.

A zero price (a provider's free tier, such as `opencode/…`) is never usable, so
the lookup continues to the paid price of the same model.

### GLM / Z.ai

Bare GLM ids, including compact forms like `glm5.1`, become `glm-{version}`
with `zai/` and `zai.` candidates; `z-ai/` and `z.ai/` are read as `zai/`. A
future `glm-5.2` resolves as soon as the catalog lists a matching key.

## Sources

No prices are hardcoded. The catalog merges LiteLLM, then models.dev, then
OpenRouter; normalization decides which keys to try, source priority decides
which record wins.

## Explicit aliases

Add one only when a rule would be unsafe or ambiguous: the logged id is a
product label, a family was renamed with no textual relation
(`grok-code` → `xai/grok-code-fast-1`), or a router logs a synthetic id. Never
for spelling differences (`glm5.1` vs `glm-5.1`, `z-ai` vs `zai`, slash vs
dot) — the normalizations cover those.

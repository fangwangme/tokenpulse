# Model Ids

Every source's model id is stored as reported and normalized for grouping and
pricing.

## Canonical (display and grouping)

1. Trim, lowercase.
2. Strip a trailing `-YYYYMMDD` or `-YYYY-MM-DD` date suffix.
3. Keep the text after the last `/` (the provider prefix is dropped).
4. Replace `.`, `_` and spaces with `-`, collapse repeated hyphens.
5. Strip an `antigravity-` or `anti-gravity-` prefix.
6. Strip `-high`, `-medium`, `-low`, `-free`, `-thinking` and `-tiered`
   repeatedly, then one trailing `-0`, then leading/trailing hyphens.
7. Drop any `preview` segment wherever it sits.

`pricing_key` is the same rule but keeps `preview`, because catalogs price it
separately.

Examples: `openai/gpt-4.1-mini-2025-04-14` → `gpt-4-1-mini`;
`gemini-3.1-pro-preview-high` → `gemini-3-1-pro`;
`antigravity-claude-opus-4-5-thinking-high-free` → `claude-opus-4-5`.

`canonical` is stored on every ledger row. Opening the store re-derives it for
every distinct model id and rebuilds affected days, so a rule change never
splits one model across two groups.

## Pseudo ids

Empty, `unknown`, `auto-*`, `*-auto-review` and `*-default` are pseudo ids:
they never resolve against a catalog and never keep a cost repair pending.

## Company

For report labels: a key starting with `codex`, `gpt`, `o1`, `o3` or `o4` is
`openai`; `claude`, `sonnet`, `opus` or `haiku` (with either antigravity
prefix) is `anthropic`; `gemini` (with either antigravity prefix) is `google`;
everything else is `other`.

## Antigravity ids

Antigravity ids get spelling normalization only: lowercase, no
`antigravity-` prefix, `.`/`_`/spaces as single hyphens. Tier words such as
`high` and `thinking` stay; `model_id::canonical` folds them for display.
Resolution of the enum/served name and unversioned families is in
`antigravity-model-aliases.md`.

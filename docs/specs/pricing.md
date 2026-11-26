# Pricing

Cost is an estimate at public API prices. Subscriptions are not billed this
way, and every surface that shows cost says so. Token counts are facts read
from agent files, some of which no longer exist on disk; prices and costs are
derived from them and can always be recomputed.

This replaces the earlier LiteLLM-first selection with fuzzy, alias, family and
nearest-version matching, and the per-day pricing snapshots. Those picked
reseller or neighbouring-model prices for models whose owner publishes its own
price.

## Catalog sources

- `models.dev` (`https://models.dev/api.json`) is the primary catalog. LiteLLM's
  price list is secondary and only its official entries are used (Selection).
  OpenRouter, Vercel and Cloudflare prices are read from their `models.dev`
  provider entries; there is no separate OpenRouter fetch.
- Both are fetched in parallel with a 30-second timeout each and cached on disk
  as `pricing-cache.json` in the data directory, carrying a `schemaVersion`,
  for 24 hours. A long-running process re-checks the age
  before each scan and re-fetches once stale. `--refresh-pricing` forces a
  fetch.
- A failed `models.dev` fetch changes nothing: the previously loaded catalog
  stays selected, no price period is synced, and the disk cache is not
  overwritten. A LiteLLM failure alone keeps the `models.dev` result, and the
  cache is written.
- A cache with another `schemaVersion` is ignored, and the Rust releases'
  `pricing.json` is never read, so an upgrade never prices from the retired
  selection.
  Prices stay unavailable until the first successful `models.dev` fetch.
- Official `release_date` metadata is kept in the cache and synced into
  `model_releases` for Antigravity's unversioned-family resolution
  (`antigravity-model-aliases.md`).

## Identity

- A ledger message's canonical model id and every catalog model id — with its
  provider prefix (the text up to the last `/`) removed — pass the same
  canonical normalization the ledger uses. An entry prices a message only when
  the two keys are equal. Beyond that shared normalization (which already
  strips date and variant suffixes) no mapping applies: no aliases, families,
  `-preview` retries, nearest versions or fuzzy matching. A model
  whose official id differs after normalization misses the official stages and
  continues down the chain; a rule is added only when a real bad case shows
  up.
- **Same-provider collision**: when several entries of one provider reach the
  same key, all-zero entries are dropped first; then an entry whose id, prefix
  removed and otherwise unnormalized, equals the key wins (`claude-opus-4-5`
  beats `claude-opus-4-5-20251101`). If none is exact and the rest disagree on
  price, that provider is skipped for that model and the collision is logged.
- An entry whose input and output prices are both 0 never prices a message.
  `-free` usage is priced at its normalized model's price.

## Selection

Each model is priced from the first stage that has an exact entry:

1. **Official, global** — the owner's own provider: Claude → `anthropic`;
   GPT/Codex → `openai`; Gemini → `google`, then `google-vertex`; DeepSeek →
   `deepseek`; GLM → `zai`; MiniMax → `minimax`; Kimi → `moonshotai`; Muse →
   `meta`; Nemotron → `nvidia`; Inkling → `thinkingmachines`. The owner comes
   from the model id; a provider hosting another company's model (NVIDIA
   serving DeepSeek, Vertex serving Claude) is never that model's official
   price. The owner's `models.dev` providers are checked in order, then the
   allowlisted LiteLLM entries; LiteLLM is read only when none of those
   `models.dev` providers has the exact entry.
2. **Official, regional** — GLM → `zhipuai`; MiniMax → `minimax-cn`; Kimi →
   `moonshotai-cn`.
3. **Fallback** — `openrouter`, then `vercel`, then `cloudflare-workers-ai`,
   then `cloudflare-ai-gateway`.
4. Otherwise the model is **unpriced**.

- A LiteLLM entry is usable only when its `litellm_provider` is exactly one of
  `anthropic`, `openai`, `gemini`, `vertex_ai`, `vertex_ai-language-models`,
  `zai`, `moonshot`, `minimax`. No prefix matching: `vertex_ai-anthropic_models`
  and other `vertex_ai-*` hosts are not Google's own models.
- A model whose owner is not listed skips the official stages and is logged
  once per run.
- The selected price carries a `tier` (`official-global`, `official-regional`,
  `fallback-openrouter`, `fallback-vercel`, `fallback-cloudflare`) and a
  `source` (the provider key, or a LiteLLM entry's `litellm_provider`). Both are
  metadata and never affect selection.
- Cache reads use the cache-read price, else 0.1 × the input price; cache
  writes use the cache-write price, else the input price. A published `0` is a
  real price. Only the base context tier is priced. Reasoning tokens are priced
  at the output price.

## Price periods

Prices are stored as periods: `model_price_periods (model, effective_from,
effective_to, input_rate, output_rate, cache_read_rate, cache_write_rate, tier,
source)`, rates in USD per million tokens, `tier` and `source` nullable, next to
`model_releases (model, release_date)`. Only the catalog sync and `reprice`
write periods; looking up a price never writes one.

- A model without a period gets its selected price from `0000-01-01`, open
  ended, so it also covers earlier unpriced messages.
- On later syncs an unchanged rate vector creates no period (a changed `tier`
  or `source` is updated in place). When the rates change: if the active
  period starts today or later it is updated in place; otherwise it is closed
  at yesterday and a new one opens today. A model that becomes unpriced has its
  active period closed at yesterday.
- Every period change recomputes `cost_usd` and `unpriced` of the messages it
  covers from the period's start, including messages ingested today. Earlier
  days keep their historical period.
- After each catalog load that changes entries, periods and releases are synced
  before the scan prices new messages; then every `unpriced = 1` message of
  every source is looked up by `(canonical model, date)` and, when a period
  covers it, backfilled with that cost and `unpriced = 0`.
- A model priced again after a closed gap opens a new period from today;
  messages dated inside the gap stay unpriced until `reprice --apply`.
- No sync runs from an unavailable or empty catalog, so an offline start can
  never close every period.

### Migration from the Rust releases

The ledger gains `unpriced` on each message, set to 1 where the stored cost is
0 and the tokens are not, else 0. A message with a stored cost above 0 keeps
that cost, byte for byte, until the user runs `reprice --apply`. Unpriced
messages of every date are backfilled by the first sync as any unpriced
message is: they had no cost, and gain the period price that covers them. The
per-day pricing snapshots are no longer read.

## Reprice

`tokenpulse reprice [--apply --yes --expect <fingerprint>] [--json]`
recomputes derived values under the current rules without touching facts. It
runs through a running `serve` (`remote.md` "Local control") so it waits for an
in-flight scan and clears the report cache afterwards, or directly on the
database when no `serve` runs.

- **Dry run** (default) writes nothing: a logical dump of the database
  (`sqlite3 .dump`) is identical before and after. Per model it reports the message count,
  token sums, current and new cost, the selected tier, source and rates; then
  the current and new totals, the unpriced models, and the **catalog
  fingerprint**: a hash of every model's selected rate vector, tier and source
  at the stored precision.
- Dry run and apply both need a successfully fetched catalog; otherwise they
  fail with `prices unavailable: models.dev has not been fetched successfully`
  and write nothing.
- **Apply** needs `--yes` and `--expect <fingerprint>`. A different current
  fingerprint (the catalog refreshed since the dry run) is refused and nothing
  is written. Otherwise it first writes a WAL-safe backup (`VACUUM INTO`) next
  to the database under a name that never overwrites an earlier backup, mode
  0600, then in one transaction deletes every period, inserts one
  period per selectable model from `0000-01-01` at its selected price,
  recomputes `cost_usd` and `unpriced` for every message from its stored
  tokens, rebuilds the daily rollups, and records the rebuild time.
- Reprice never adds, removes or rescans messages and never changes their token
  counts, keys or model identity, nor `model_releases`.
- A failed apply leaves the database unchanged; the backup stays for manual
  rollback.

## Unpriced and offline

A model no catalog prices is unpriced: its tokens count, its cost is 0, and
reports mark it. Offline, a cached catalog of the current schema serves even
when older than 24 hours; with no usable cache every model is unpriced and
reports say prices are unavailable. A failed catalog load is retried on the
first scan at least 10 minutes after the failure, not on every scan.

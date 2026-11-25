# Pricing Module

Pricing turns token facts into estimated cost. Binding behavior is in
[`../specs/pricing.md`](../specs/pricing.md).

## Catalog

`internal/pricing` fetches models.dev and LiteLLM, indexes every model by exact
raw id and by the canonical/pricing key, and selects one entry per model:

1. an official provider in the global catalog;
2. an official regional provider;
3. OpenRouter as the fallback.

Entries whose rates disagree within the same provider are skipped; an all-zero
entry never prices. The selected catalog and its fetch time are cached as
`pricing-cache.json`; the 0.6.x `pricing.json` is ignored.

## Periods and cost

`usage.db` stores `model_price_periods` and `model_releases`. A model gets an
open period from `0000-01-01` the first time it is priced. Later rate changes
close the active period at yesterday and open a new one today; a period that
already started today is updated in place. Messages are costed by the period
covering their date.

A stored cost above 0 is a fact: migration, scans and catalog refreshes never
change it. Only `tokenpulse reprice --apply` rewrites costs. Unpriced messages
are backfilled when a period starts covering them.

## Commands

```bash
tokenpulse reprice                         # dry run: fingerprint + per-model report
tokenpulse reprice --json
tokenpulse reprice --apply --yes --expect <fingerprint>
```

A dry run writes nothing. Apply refuses a stale fingerprint, writes a
`*.pre-reprice.<timestamp>.bak` backup with mode 0600, replaces every period
with one open period per selected model, recomputes every message from its
stored tokens and rebuilds daily rollups. When `serve` runs, the command goes
through its local socket so it waits for an in-flight scan and clears the
report cache.

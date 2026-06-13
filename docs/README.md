# TokenPulse Documentation

This folder is the technical and maintainer documentation. To avoid the docs
drifting or repeating each other, every topic has **one canonical home** — edit
that file and link to it from elsewhere rather than copying content.

## Map

| Doc | Audience | Canonical home for |
| --- | --- | --- |
| [`../README.md`](../README.md) | Users | Install + quick start, feature highlights, screenshots. Keep it short; link here for depth. |
| [`DESIGN.md`](DESIGN.md) | Contributors | Architecture overview: scope, principles, crate/module layout, data flow. |
| [`modules/quota.md`](modules/quota.md) | Contributors | Per-provider quota fetching: endpoints, auth, response→`RateWindow` mapping. |
| [`modules/usage.md`](modules/usage.md) | Contributors | Usage parsing/ingest pipeline, providers, store schema, CLI/TUI output. |
| [`modules/pricing.md`](modules/pricing.md) | Contributors | Pricing catalog, lazy refresh, daily snapshots, cost derivation. |
| [`modules/tui.md`](modules/tui.md) | Contributors | TUI tabs, key bindings, settings, rendering widgets. |
| [`specs/`](specs/) | Contributors | Product/feature specs (named by feature, no dates). See `specs/README.md`. |
| [`model-pricing-mapping.md`](model-pricing-mapping.md) | Contributors | Reference: model id → pricing/provider mapping rules. |
| [`RELEASING.md`](RELEASING.md) | Maintainers | Versioning, tagging, GitHub release, npm publishing. |
| [`archive/`](archive/) | — | Historical/superseded plans; not maintained. |

## Where things live (to prevent duplication)

- **Key bindings, tab behaviour, settings** → `modules/tui.md` only.
- **Quota window labels / durations / per-provider endpoints** → `modules/quota.md` only.
- **Config keys and defaults** → described where they are consumed
  (`modules/tui.md` for the settings tab, `modules/usage.md`/`modules/quota.md`
  for behaviour); the authoritative list is the `DisplayConfig` struct.
- **Release/packaging steps** → `RELEASING.md` only.
- **Working notes, dated decisions, plans** → `.agents/notes/` and
  `.agents/plans/` (not committed to `docs/`).

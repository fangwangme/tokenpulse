# Raw Model Usage Inspection Spec

## Purpose
Allow inspection of un-normalized, raw model usage for a specified date (defaulting to today) via the CLI without disturbing canonical model normalization in the TUI or daily aggregations. This allows developers to observe model routing shifts (e.g. `served_model`), tier variations (e.g. `-high`, `-medium`, `-low`, `-thinking`), and internal snapshots.

## CLI Interface
- `tokenpulse --raw [--date <YYYY-MM-DD>] [--json]`
- `tokenpulse usage --raw [--date <YYYY-MM-DD>] [--json]`
- Behavior:
  - If `--date` is omitted, defaults to the current local date.
  - `--date` requires `--raw` (enforced via clap).
  - If `--json` is supplied, emits a JSON document containing date, summary totals, and an array of raw model objects with full precision costs.
  - Otherwise, emits a styled Unicode table containing Source, Raw / Served Model, Tier / Alias, Canonical, Total Tkns, Cost USD (2 decimal places), Calls, and In / Out / CR / R breakdown.
  - Rows are grouped by Agent (Source Total Tkns DESC), then Canonical Model Total Tkns DESC, then Raw Variant Total Tkns DESC, separated by horizontal dividers.

## Core Storage API
- `UsageStore::load_raw_models_for_date(&self, date: NaiveDate) -> Result<Vec<RawModelUsageRow>>`
- `UsageStore::count_distinct_sessions_for_date(&self, date: NaiveDate) -> Result<i64>`
- Queries `usage_messages` for rows where `date = ?1`.
- Groups by `{CANONICAL_SOURCE_SQL}, COALESCE(NULLIF(served_model, ''), model_id), model_id`.
- Returns:
  - `source`: Canonical source name.
  - `raw_model_id`: Physical served model routing (from `served_model` or fallback `model_id`).
  - `tier_model_id`: Ingested model alias / tier (e.g. `gemini-3-8-flash-high`).
  - `canonical_model_id`: Canonical normalized model identifier (e.g. `gemini-3-8-flash`).
  - `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `reasoning_tokens`, `total_tokens`.
  - `cost_usd`: Sum of precomputed message costs.
  - `message_count`: Total message count.
  - `session_count`: Count of distinct sessions for this specific model variant.
- Distinct session totals across the whole date are queried independently via `count_distinct_sessions_for_date` to prevent multi-model session double counting.

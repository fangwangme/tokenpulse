# Raw Model Usage Inspection Spec

## Purpose
Allow inspection of un-normalized, raw model usage for a specified date (defaulting to today) via the CLI without disturbing canonical model normalization in the TUI or daily aggregations. This allows developers to observe model routing shifts, tier variations (e.g. `-high`, `-medium`, `-low`, `-thinking`), and internal snapshots.

## CLI Interface
- `tokenpulse --raw [--date <YYYY-MM-DD>] [--json]`
- `tokenpulse usage --raw [--date <YYYY-MM-DD>] [--json]`
- Behavior:
  - If `--date` is omitted, defaults to the current local date.
  - If `--json` is supplied, emits a JSON document containing date, summary totals, and an array of raw model objects.
  - Otherwise, emits an ASCII table containing Source, Raw Model, Canonical Model, Total Tokens, Cost USD, Calls, and In/Out/CR/R breakdown, sorted by Total Tokens descending.

## Core Storage API
- `UsageStore::load_raw_models_for_date(&self, date: NaiveDate) -> Result<Vec<RawModelUsageRow>>`
- Queries `usage_messages` for rows where `date = ?1`.
- Groups by `source, model_id`.
- Returns:
  - `source`: Canonical source name.
  - `model_id`: Raw model name as recorded during ingestion.
  - `canonical_model_id`: Canonical normalized model identifier.
  - `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `reasoning_tokens`, `total_tokens`.
  - `cost_usd`: Sum of precomputed message costs.
  - `message_count`: Total message count.
  - `session_count`: Count of distinct sessions.
- Sorted by `total_tokens DESC`.

# Usage Perf Latency Analysis

Date: 2026-05-23

## Summary

`tokenpulse usage` (text mode) takes 4.5s ~ 13.5s on a warm cache with 79k messages across 7 providers. The variance is driven primarily by DB contention and filesystem I/O. The two biggest cost centers are **Antigravity sync+parse** (26-36% of total) and the **post-ingestion load phase** (33-43% of total).

## Time Breakdown (Run 4 — clean warm run, 4,469ms total)

| Phase | Duration | % |
|---|---|---|
| stale_check × 7 | 67ms | 1.5% |
| **antigravity parse_sessions** | **1,168ms** | **26%** |
| antigravity ingest_upsert (4.7k msgs) | 437ms | 10% |
| **load_dashboard_days** | **522ms** | **12%** |
| **load_provider_summaries** | **477ms** | **11%** |
| **load_model_summaries** | **470ms** | **10%** |
| load_summary_counts | 325ms | 7% |
| gemini parse_sessions (103 msgs) | 278ms | 6% |
| codex parse+ingest (1.2k msgs) | 268ms | 6% |
| repair_zero_costs | 136ms | 3% |
| opencode parse+ingest (873 msgs) | 143ms | 3% |
| claude + copilot + pi | 175ms | 4% |

## Per-Provider Deep Dive

### Antigravity (26-60% of total, 1.0s–2.8s)

`parse_sessions` always triggers the full sync pipeline:
1. `detect_antigravity_connections()` — process discovery
2. RPC `GetAllCascadeTrajectories` — fetch session list
3. Per-session RPC `GetCascadeTrajectoryGeneratorMetadata` — fetch usage
4. `normalize_cached_antigravity_artifacts()` — model alias normalization against cache.db

The `since` filter on `pricing_day >= ?1` **IS working** — messages dropped from ~10k to ~4.6k after the fix in this branch. However, the sync step still runs unconditionally every invocation, regardless of whether the cache is already up-to-date.

**Root cause**: No mechanism to skip sync when the local cache.db is recent enough. The sync step itself takes 0.5–2s depending on Antigravity process availability and RPC latency.

### Load Phase (1.5–2.5s, 33-43% of total)

Four independent SQL queries each scan `daily_model_usage` (133 days × 51 models × 7 providers):

| Query | Time | Aggregation |
|---|---|---|
| `load_dashboard_days` | 522ms | date-level SUM |
| `load_provider_summaries` | 477ms | provider-level SUM |
| `load_model_summaries` | 470ms | model-level SUM |
| `load_summary_counts` | 325ms | COUNT(DISTINCT) from `usage_messages` |

These share the same base data but open separate connections and run separate scans.

**Root cause**: Each query independently scans `daily_model_usage`. They could share a single scan with multi-level aggregation in SQLite.

### Ingest Per-Message Cost

| Provider | Messages | Ingest Time | Per-msg |
|---|---|---|---|
| antigravity | 4,700 | 437ms | 0.09ms |
| codex | 1,170 | 220ms | 0.19ms |
| opencode | 873 | 98ms | 0.11ms |

Each message triggers: `ensure_pricing_snapshot()` (cache lookup or INSERT into `daily_pricing_snapshots`) + `INSERT ... ON CONFLICT DO UPDATE` into `usage_messages`. Within the same `(date, provider, model)` group, the pricing snapshot is checked per-message instead of being cached per-transaction.

### repair_zero_costs (0.1–1.5s, 3-14% of total)

All 9 runs returned `repaired=0`, yet the query still scans `usage_messages` for `cost_usd <= 0 AND total_tokens > 0`. The `has_zero_cost_repairs_pending()` guard works but the SQLite full-table scan to find zero matches on 79k rows still has I/O cost that varies with DB state.

### Gemini File Scanning (0.3–1.5s, 6-14%)

| Run | parse_sessions | Variance |
|---|---|---|
| Best | 278ms | — |
| Worst | 1,477ms | ×5.3 |

Only 103 messages across 9 sessions, but `WalkDir` + `metadata().modified()` on `~/.gemini/tmp` (potentially 1200+ files) is sensitive to macOS filesystem cache state. The `since` filter reduces the *parsed* count to recent files but still stats every file to check `modified()` timestamps.

### stale_check Variance (40–580ms)

`source_has_stale_parser_version()` for 7 providers queries `usage_messages` with `WHERE source = ? AND parser_version != ? LIMIT 1`. On a busy DB (concurrent writes), SQLite read-lock contention inflates this from ~40ms to ~580ms.

## Optimization Recommendations

### P0 — High Impact, Low Risk

1. **Conditional Antigravity sync** (`antigravity.rs:76-90`)
   - Before calling `sync_antigravity_with_options()`, check if the last sync was recent (e.g. read `MAX(last_modified_ms)` from cache.db and compare with a configurable staleness threshold).
   - Expected: **-1.0s to -2.0s**

2. **Merge 4 load queries into 1** (`store.rs:592-792`)
   - Run a single `SELECT date, source, provider_id, model_id, input, output, ... FROM daily_model_usage WHERE ...` and compute dashboard days, provider summaries, model summaries, and summary counts in Rust in a single pass.
   - Expected: **-1.0s to -1.5s**

### P1 — Medium Impact

3. **Cache pricing snapshot per transaction** (`store.rs:172-227`)
   - In `ingest_messages()`, track which `(date, provider_id, model_id)` combos have already been snapshotted within the current transaction. Skip `ensure_pricing_snapshot()` for repeats.
   - Expected: **-0.2s to -0.4s**

4. **Faster repair_zero_costs guard** (`store.rs:525-529`)
   - Replace the full `COUNT(*)` scan in `has_zero_cost_repairs_pending()` with a metadata-driven approach: track `has_pending_repairs` flag per source in a lightweight table updated during ingest.
   - Expected: **-0.1s to -1.0s** (eliminates variance)

### P2 — Lower Impact

5. **Gemini file list caching** (`gemini.rs:203-222`)
   - Before `WalkDir`, read a cached file list (JSON or SQLite) keyed by the last modified time of the directory. Only re-scan if directory mtime changed.
   - Expected: **-0.2s to -1.0s**

6. **stale_check parallelization** (`usage.rs:70-87`)
   - The 7 `source_has_stale_parser_version()` calls are sequential. Run them in parallel with `rayon` (only need read-only DB access).
   - Expected: **-0.03s to -0.5s**

## Raw Data

Log file: `~/.local/share/tokenpulse/log/usage-2026-05-23.log`

| Run | Time | Mode | Antigravity Parse | Gemini Parse | Total |
|---|---|---|---|---|---|
| 1 | 12:15:28 | text | 2,771ms | 1,477ms | 13,450ms |
| 2 | 12:24:38 | text | 1,689ms | 409ms | 5,849ms |
| 3 | 12:27:07 | text | 1,092ms | 334ms | 4,936ms |
| 4 | 12:28:46 | text | 1,168ms | 278ms | 4,469ms |
| 5 | 12:28:58 | text | 1,121ms | 300ms | 4,490ms |
| 6 | 12:30:28 | text | 1,729ms | 1,268ms | 10,650ms |
| 7 | 12:32:26 | text | 2,549ms | 454ms | 7,424ms |
| 8 | 12:34:46 | TUI | 1,875ms | 1,087ms | 9,975ms |
| 9 | 13:55:13 | text | 1,641ms | 322ms | 4,906ms |
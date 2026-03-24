# Upstream Session Status And Dashboard Research

Date: 2026-03-22

## Goal

Understand how the upstream projects:

1. scan local history
2. turn that history into status or usage data
3. model GitHub-style dashboard data

Repositories inspected locally:

- `/tmp/tokscale`
- `/tmp/CodexBar`
- `/tmp/openusage`
- `/tmp/antigravity-trajectory-extractor`

---

## Summary

The projects solve different layers:

- `tokscale` is the best reference for local scanning, normalized usage messages, daily contributions, and GitHub-style graph data.
- `CodexBar` is the best reference for incremental file scanning, durable scan cache, and compact daily usage snapshots.
- `openusage` is the best reference for provider-facing status lines, not for low-level local history parsing.
- `antigravity-trajectory-extractor` is useful for Antigravity local discovery and session identity, not token accounting.

---

## 1. `tokscale`

Relevant files:

- `crates/tokscale-core/src/scanner.rs`
- `crates/tokscale-core/src/sessions/*.rs`
- `crates/tokscale-core/src/aggregator.rs`
- `crates/tokscale-core/src/lib.rs`
- `packages/frontend/src/lib/utils.ts`

### How it scans

- Uses explicit per-client roots and filename patterns.
- Scans multiple roots per provider when needed.
- Handles Codex `sessions`, `archived_sessions`, and headless roots.
- Handles OpenCode SQLite plus legacy JSON sources.
- Uses parallel directory traversal and parallel aggregation.

### How it produces usage/status data

- Normalizes parsed records into a shared `UnifiedMessage` shape.
- Aggregates messages by day into `DailyContribution`.
- Produces a top-level `GraphResult` with:
  - `meta`
  - `summary`
  - `years`
  - `contributions`

### How it supports GitHub-style dashboard data

- Treats daily contributions as a first-class output.
- Fills missing days in frontend utilities.
- Groups days into week columns.
- Recomputes intensity after filtering.
- Supports heatmap-style calendar views and wrapped/year views.

### Main lesson for TokenPulse

- Build a daily contribution layer as a stable backend contract.
- Do not make the dashboard depend directly on raw parser output.

---

## 2. `CodexBar`

Relevant files:

- `Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift`
- `Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+Claude.swift`
- `Sources/CodexBarCore/Vendored/CostUsage/CostUsageCache.swift`
- `Sources/CodexBarCore/CostUsageFetcher.swift`
- `Sources/CodexBarCore/WidgetSnapshot.swift`
- `Sources/CodexBar/UsageStore+WidgetSnapshot.swift`

### How it scans

- Maintains a durable cache file per provider.
- Stores per-file scan metadata:
  - `mtimeUnixMs`
  - `size`
  - `days`
  - `parsedBytes`
  - `lastModel`
  - `lastTotals`
  - `sessionId`
- Uses append-only incremental rescans if the file grew and continuation state is valid.
- Falls back to a full reparse when incremental continuity is unsafe.

### How it produces usage/status data

- Rebuilds day-level usage from file cache contributions.
- Converts daily report data into `CostUsageTokenSnapshot`.
- Exposes compact fields like:
  - most recent day tokens/cost
  - last 30 days tokens/cost
  - daily usage list

### How it supports dashboard-like presentation

- Persists `WidgetSnapshot` entries containing:
  - quota windows
  - credits
  - token summary
  - daily usage points
- Uses daily usage arrays for small charts/widgets.

### Main lesson for TokenPulse

- Introduce persistent scan-state caching before the history grows further.
- Store day-level file contribution deltas so rescans can add/subtract safely.

---

## 3. `openusage`

Relevant files:

- `docs/plugins/api.md`
- `plugins/claude/plugin.js`
- `plugins/codex/plugin.js`
- `plugins/antigravity/plugin.js`

### How it scans

- For Claude and Codex, it does not implement the local scanner itself.
- It calls `ctx.host.ccusage.query(...)` to get daily usage.
- The host normalizes only the top-level shape to `{ daily: [...] }`.

### How it produces status data

- Each provider returns:
  - `plan`
  - `lines[]`
- Line kinds are:
  - `text`
  - `progress`
  - `badge`
- Claude and Codex plugins convert daily usage into compact status lines:
  - `Today`
  - `Yesterday`
  - `Last 30 Days`
- Quota/current usage windows come from provider APIs or local auth/session material.

### How it supports dashboard concepts

- Mostly through provider cards and per-provider daily summaries.
- It is not the main reference for a full contribution heatmap.

### Main lesson for TokenPulse

- Use a normalized provider-card/status contract for live quota views.
- Do not use `openusage` as the primary reference for historical parser architecture.

---

## 4. `antigravity-trajectory-extractor`

Relevant files:

- `src/antigravity_trajectory/extractor.py`

### How it scans

- Reads local state DB:
  - `~/Library/Application Support/Antigravity/User/globalStorage/state.vscdb`
- Reads cached conversation candidates:
  - `~/.gemini/antigravity/conversations/*.pb`
- Discovers running `language_server` processes via `ps` and `lsof`.

### How it produces session/status data

- Builds session identity from:
  - `cascade_id`
  - `trajectory_id`
  - workspace paths
  - title/summary
  - last modified time
- Uses local cache for candidate discovery.
- Uses live local RPC for authoritative conversation lookup.

### How it supports dashboard concepts

- It does not provide token usage/cost aggregation.
- Its value is discovery and identity, not daily usage totals.

### Main lesson for TokenPulse

- Treat Antigravity as a discovery-and-identity problem first.
- Do not assume this repo already solves token accounting.

---

## Cross-Project Conclusions

### For local scanning

- `tokscale` is the best shape reference.
- `CodexBar` is the best performance and incremental-cache reference.

### For provider status output

- `openusage` is the cleanest provider-card/status-line reference.

### For GitHub-style dashboard data

- `tokscale` is the clearest reference because it has a dedicated `DailyContribution` and `GraphResult` model.

### For Antigravity support

- `antigravity-trajectory-extractor` should inform source discovery and session identity only.

---

## Recommended Borrowing Strategy For TokenPulse

1. Borrow `tokscale`'s contribution-style data model.
2. Borrow `CodexBar`'s incremental file cache strategy.
3. Borrow `openusage`'s provider status normalization.
4. Borrow `antigravity-trajectory-extractor`'s local Antigravity discovery path.

## Mapping To TokenPulse Tabs

If TokenPulse is organized around `GitHub`, `By Day`, and `By Model`, the upstream references map cleanly:

- `GitHub`
  - primary reference: `tokscale`
  - secondary reference: `CodexBar` daily snapshot shape
- `By Day`
  - primary reference: `tokscale` daily contribution and graph data
  - secondary reference: `openusage` today/yesterday/last-30-days status summaries
- `By Model`
  - primary reference: `tokscale` client/model aggregation
  - secondary reference: existing TokenPulse provider/model summaries

This is a useful split because it avoids forcing one upstream to solve all product layers.

## Bottom Line

If TokenPulse needs one upstream for dashboard data, use `tokscale`.

If TokenPulse needs one upstream for scan efficiency, use `CodexBar`.

If TokenPulse needs one upstream for live provider card output, use `openusage`.

If TokenPulse needs Antigravity history support, start from `antigravity-trajectory-extractor` for discovery, not for cost accounting.

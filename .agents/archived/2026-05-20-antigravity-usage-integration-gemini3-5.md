# Antigravity Usage Tracking Integration

Integrate Antigravity usage and session data parsing into TokenPulse. We build a process discovery and sync mechanism inside the CLI usage flow to fetch trajectories and cache them to local JSONL files, then parse them into the ledger.

## Problem Diagnosed

1. **ID Mismatch on RPC queries**:
   - `GetAllCascadeTrajectories` returns a flat object map where the keys are the actual conversation/cascade IDs (e.g., `04a78b10-...`), and the values contain `trajectoryId` (e.g., `bbd4d9a6-...`).
   - The sync logic was parsing `session_id` by looking up the `trajectoryId` field inside the object.
   - When calling downstream RPC `GetCascadeTrajectoryGeneratorMetadata`, the server expects `cascadeId` but was receiving `trajectoryId`, leading to a `500: trajectory not found` error.
   - This caused the sync to fail silently for all sessions, displaying `synced sessions this run: 0` and failing to ingest any Antigravity usage data.

2. **Fix**:
   - Update `sync_antigravity` so that in `extract_trajectory_entries`, if `key` is present, it is preferred as the cascade/session ID.
   - Fall back to the object fields only if the `key` is empty.

## Proposed Changes

### tokenpulse-core

#### [MODIFY] [antigravity.rs](file:///Users/fangwang/project/tools/tokenpulse/.worktrees/feat-antigravity-usage/tokenpulse-core/src/usage/antigravity.rs)
- Modify ID mapping loop in `sync_antigravity` to prioritize `key` (cascade ID) over internal object fields:
  ```rust
  let session_id = if !key.is_empty() {
      key
  } else {
      item.get("cascadeId")
          .or_else(|| item.get("trajectoryId"))
          .or_else(|| item.get("id"))
          .or_else(|| item.get("sessionId"))
          .and_then(Value::as_str)
          .map(String::from)
          .unwrap_or_default()
  };
  ```

## Verification Plan

### Automated Tests
- Run `cargo test` in `tokenpulse-core` and `tokenpulse-cli`.

### Manual Verification
- Run `cargo run -- usage -p antigravity` to verify automatic process discovery, sync, caching, and parsing.
- Verify that `synced sessions this run` matches active sessions and data shows up in output or TUI.

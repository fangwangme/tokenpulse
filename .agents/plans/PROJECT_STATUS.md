# Project Status

## Current State

- TokenPulse is a Rust workspace with `tokenpulse-core` and `tokenpulse-cli`.
- The CLI includes usage analytics and quota views backed by SQLite via `rusqlite`.
- The TUI uses `ratatui` and `crossterm`.
- Repository structure is being migrated from shared worktree state to worktree-local `.local/` state.
- Manual worktrees are expected under `.worktrees/`.

## Active Structure Notes

- Rust build artifacts are configured through `.cargo/config.toml` to use `.local/target/`.
- CI and release workflows use `.local/target/` and `.local/release/`.
- `.local/` and `.worktrees/` are local-only directories.
- `.agents/` contains project notes, plans, archived context, and this project status file.

## Change Log

### 2026-04-28

- Simplified worktree structure around `.worktrees/` and `.local/`.
- Removed the old shared-state convention from active project configuration.
- Updated `.agents/` to use `notes/`, `plans/`, `archived/`, and project status tracking.

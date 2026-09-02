# Global Claude Code Rules

## Project Structure
- Work in a non-`main` git worktree for normal development
- Only modify `main` directly when the user explicitly authorizes template or repository-structure maintenance
- Manual worktrees live under `.worktrees/`
- Worktree-local state lives under `.local/`
- Shared specs live under `docs/specs/`
- Agent notes, plans, archives, and project status live under `.agents/`

## Conventions
- Keep code clean and minimal
- Follow existing patterns
- Ask before making big changes
- User-visible changes directly bump the version in `Cargo.toml` and record their
  `CHANGELOG.md` section under `## [X.Y.Z] - YYYY-MM-DD` in the same PR, so that
  merging the PR allows immediate tagging and releasing without extra steps.

## Review Focus
- CI format failures must be checked at the workflow/toolchain level, not just
  with local `cargo fmt`. In particular, verify `.github/workflows/*.yml`
  bootstraps a real Rust toolchain and that formatting uses
  `cargo fmt --all -- --check`.
## Architecture
- **Workspace**: `tokenpulse-core` (library) + `tokenpulse-cli` (binary)
- **TUI**: ratatui 0.29 + crossterm 0.28 (with mouse capture)
- **Data**: SQLite via rusqlite (bundled) — `usage.db` (usage ledger) and
  `tokenpulse.db` (quota cache + observation history)
- **Logging**: tracing to a daily rotating file under
  `~/.local/share/tokenpulse/log/`; never stdout, which would corrupt the TUI
- **Tests**: 284 passing

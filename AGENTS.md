# Global Claude Code Rules

## Project Structure
- Work in the current non-`main` git worktree for development
- Never make changes from a checked-out `main` branch worktree
- If the current worktree is on `main`, stop and switch to or create a non-`main` worktree first
- Shared resources in `.shared/`
- Build outputs to `.shared/dist/` or `.shared/release/`
- Rust build artifacts in `.shared/target/` (via `.cargo/config.toml`)
  - In worktrees, symlink: `target → .shared/target`

## Conventions
- Keep code clean and minimal
- Follow existing patterns
- Ask before making big changes

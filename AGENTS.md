# Global Claude Code Rules

## Project Structure
- Work in `.worktrees/tokenpulse-dev/` for development
- Shared resources in `.shared/`
- Build outputs to `.shared/dist/` or `.shared/release/`
- Rust build artifacts in `.shared/target/` (via `.cargo/config.toml`)
  - In worktrees, symlink: `target → .shared/target`

## Conventions
- Keep code clean and minimal
- Follow existing patterns
- Ask before making big changes

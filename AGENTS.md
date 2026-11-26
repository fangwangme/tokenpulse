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
- Add a comment only when the reason or constraint is not obvious from the
  code; do not narrate what the code already says, and delete stale comments
- Ask before making big changes
- User-visible changes directly bump `internal/version/version.go` and record
  their `CHANGELOG.md` section under `## [X.Y.Z] - YYYY-MM-DD` in the same PR,
  so that merging the PR allows immediate tagging and releasing without extra
  steps.
- Never put CI skip markers (`[skip ci]` and the like) in commit messages.
  Squash merges copy every commit message into the merge commit on `main`,
  where the marker skips the push CI and the Release run for a tag on that
  commit. Doc-only pushes to `main` are already skipped by CI's
  `paths-ignore`; a doc-only push to a PR branch simply runs CI.

## Test Environment
- Every run from a worktree sets `TOKENPULSE_ENV=test`,
  `TOKENPULSE_DATA_DIR=<worktree>/.local/data/tokenpulse` and
  `TOKENPULSE_CONFIG_PATH=<worktree>/.local/data/tokenpulse/config.toml`; the
  binary shows `<version> (test)` and refuses to run in test mode without a
  data directory (`docs/specs/go-rewrite.md`).
- Seed the data directory with copies of the main data files: SQLite through
  `sqlite3 -readonly <src> ".backup '<dst>'"`, the config and
  `keeper_state.json` with `cp`. After copying, keep the worktree development
  defaults: set `display.refresh_quota = false` and
  `display.auto_refresh_secs = 900` in the copied config (the production file
  may carry different explicit values). Never point a worktree run at
  `~/.local/share/tokenpulse`.
- Quota fetches read the agents' real credentials and call the providers'
  read-only usage endpoints; that is allowed. Keeper pings spend real quota:
  never run one from a worktree unless the user asks for that run.
- Test `serve` uses ports `14120`/`18444` and must not touch the production
  Tailscale Serve mapping on `8444`.

## Review Focus
- CI format failures must be checked at the workflow/toolchain level, not just
  with local `gofmt`. In particular, verify `.github/workflows/*.yml`
  bootstraps a real Go toolchain and that formatting checks `gofmt -l .`.
- Run `go vet ./...` and `go test ./...`; the web app is built with Node before
  the binary so the embedded assets are current.
## Architecture
- **Language**: Go 1.27, one static binary; module
  `github.com/fangwangme/tokenpulse`
- **CLI/TUI**: Cobra-style command dispatch in `internal/cli`; Bubble Tea +
  Lip Gloss dashboard in `internal/tui`
- **Remote**: `tokenpulse serve` in `internal/remote` owns scanning and quota
  refreshes, serves the HTTP API, the local Unix socket and the embedded Solid +
  TypeScript web app in `web/`
- **Data**: SQLite via modernc.org/sqlite (pure Go) — `usage.db` (ledger and
  price periods) and `tokenpulse.db` (quota cache + observation history)
- **Logging**: rotating file under `~/.local/share/tokenpulse/log/`; never
  stdout, which would corrupt the TUI

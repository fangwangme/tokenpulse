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

## Current Status (tui-opt branch)

### Completed Improvements
- **Copilot naming**: Displays as "GITHUB COPILOT" instead of "UNKNOWN"
- **Dynamic billing period**: Copilot uses actual calendar month calculation instead of flat 30 days
- **Quota overview vs detail**: Overview tab shows top 3 windows; detail tabs show all
- **Expected progress marker**: Gauge widget shows `▏` marker at theoretical expected usage
- **Fixed gauge alignment**: All gauges within a provider card have aligned progress bars
- **Full-width bar chart**: Auto-fills terminal width with Y-axis labels and sub-cell precision
- **Agent/Provider distinction**: Clear separation - Agent (Claude Code, Codex, etc.) vs Provider (Anthropic, OpenAI, etc.)
- **Full agent names**: Legend uses "Claude Code" instead of "CLA", etc.
- **Per-column semantic colors**: Token=green, Cost=gold, Messages=blue throughout tables
- **Scrollable top models**: All models shown with scroll support
- **Activity mouse click**: Click on activity heatmap cells to select a day
- **Day detail by agent**: Activity day detail groups models by agent with cost rollup
- **60-day default chart**: Overview chart shows last 60 days token usage by model company
- **Overview summary cards**: Today/week/month/total cost cards with mini trend
- **Overview metric toggle**: `t`/`c` switches token vs cost chart
- **Models quick filter**: `/` filters model table rows by model/provider/agent
- **Row selection scrolling**: Overview models, Models, and Daily move selected rows before scrolling the viewport
- **Daily token trend**: Wide Daily table shows a 7-day token sparkline column
- **Usage JSON output**: `tokenpulse usage --json` emits script-friendly summary JSON

### Architecture
- **Workspace**: `tokenpulse-core` (library) + `tokenpulse-cli` (binary)
- **TUI**: ratatui 0.29 + crossterm 0.28 (with mouse capture)
- **Data**: SQLite via rusqlite (bundled)
- **Tests**: 133 passing

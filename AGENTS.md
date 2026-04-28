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
- **Overview metric toggle**: `t`/`c` switches token vs cost chart
- **Models quick filter**: `/` filters model table rows by model/provider/agent
- **Row selection scrolling**: Overview models, Models, and Daily move selected rows before scrolling the viewport
- **Daily token trend**: Wide Daily table shows a 7-day token sparkline column
- **Usage JSON output**: `tokenpulse usage --json` emits script-friendly summary JSON
- **Auto-refresh in quota TUI**: configurable intervals (1/2/5/10/15 min, default 5 min); `a` key cycles live; countdown shown in footer
- **`r` key to refresh**: shown in footer for all tabs; both quota and usage TUI support manual refresh
- **Models table polish**: `%` cost column added; sort arrow embedded inside column header width
- **GitHub-style heatmap quartiles**: equal-count quartile thresholds; uniform-value fallback uses [0.80, 0.90, 0.95, 1.0]×v
- **Overview cards removed**: freed space for more model rows; cost stats (Today/Week/Month) moved to Daily and Activity tabs
- **Daily tab global stats**: summary bar shows Today/This Week/This Month cost alongside period stats
- **Activity global stats**: Range Overview shows Today/Week/Month/All-time cost below window stats

### Architecture
- **Workspace**: `tokenpulse-core` (library) + `tokenpulse-cli` (binary)
- **TUI**: ratatui 0.29 + crossterm 0.28 (with mouse capture)
- **Data**: SQLite via rusqlite (bundled)
- **Tests**: 145 passing

# TUI Module - Detailed Design

## Overview

Fancy terminal dashboard using `ratatui` + `crossterm`. Two modes: quota view and usage view.

## Architecture

```
tui/
├── mod.rs              # App state, event loop, mode switching
├── theme.rs            # color palette, provider colors, model detection
├── widgets/
│   ├── mod.rs
│   ├── gauge.rs        # gradient progress bars with percentage + labels
│   ├── barchart.rs     # stacked bar charts (provider breakdown)
│   ├── heatmap.rs      # contribution-calendar heatmap
│   └── trend.rs        # compact sparklines
└── views/
    ├── mod.rs
    ├── quota.rs        # quota dashboard layout
    └── usage.rs        # usage dashboard with 4 tabs
```

## Color Theme

Provider-specific colors for visual distinction:

```rust
pub struct Theme {
    pub bg: Color,              // Dark background
    pub fg: Color,              // Primary text
    pub dim: Color,             // Secondary text
    pub border: Color,          // Box borders
    pub accent: Color,          // Highlights

    // Agent colors
    pub claude: Color,          // #FB923C (orange)
    pub codex: Color,           // #34D399 (emerald)
    pub opencode: Color,        // #818CF8 (indigo)
    pub gemini: Color,          // #60A5FA (blue)
    pub pi: Color,              // #F472B6 (pink)
    pub antigravity: Color,     // #C084FC (purple)
    pub copilot: Color,         // #A3E635 (lime)

    // Gauge gradient (low → high usage)
    pub gauge_low: Color,       // Green
    pub gauge_mid: Color,       // Yellow
    pub gauge_high: Color,      // Red

    // Heatmap palettes by metric family
    pub token_heatmap: [Color; 5],
    pub cost_heatmap: [Color; 5],
}
```

### Model Color Detection

The `model_color()` method detects provider from model name and assigns a fixed color:

| Pattern                     | Provider  | Color          |
| --------------------------- | --------- | -------------- |
| claude, sonnet, opus, haiku | Anthropic | Orange #FB923C |
| gpt, o1, o3, o4             | OpenAI    | Green #34D399  |
| gemini                      | Google    | Blue #60A5FA   |
| deepseek                    | DeepSeek  | Cyan #06B6D4   |
| grok                        | xAI       | Yellow #EAB308 |
| llama, meta                 | Meta      | Indigo #6366F1 |
| nvidia, nemotron            | Nvidia    | Green #76B900  |
| mistral, codestral          | Mistral   | Orange #FF731D |
| qwen                        | Qwen      | Purple #5940FF |

## Quota View Layout

Each window uses a progress bar on top and a detail line
`<countdown> … used … remaining … <pace>` directly below it, with a blank row
between consecutive windows. Compact cards collapse to a single bar + percentage
+ reset countdown.

```
┌──────────────────────────────────────────────────────────────────┐
│  Header: "TokenPulse - Quota" + timestamp                        │
├──────────────────────────────────────────────────────────────────┤
│  ┌─ Antigravity ────────────────────────────────────────────┐    │
│  │  Gemini (5h)  ████████▏░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │    │
│  │    3h 12m   used 42.00%  remaining 58.00%  On track      │    │
│  │                                                          │    │
│  │  Gemini (7d)  ███▏░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │    │
│  │    4d 6h    used 18.00%  remaining 82.00%  12% under pace │    │
│  └──────────────────────────────────────────────────────────┘    │
│  ┌─ Copilot ────────────────────────────────────────────────┐    │
│  │  Completions  ███████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │    │
│  │    29d      used 25.00%  remaining 75.00%  On track      │    │
│  └──────────────────────────────────────────────────────────┘    │
├──────────────────────────────────────────────────────────────────┤
│  Footer: q quit │ r refresh │ j/k scroll                         │
└──────────────────────────────────────────────────────────────────┘
```

## Usage View Layout

7 tabs switchable with ←/→:

### Tab 1: Overview
- Top: stacked bar chart switchable between daily tokens and daily cost, last 60 days, colored by model company
- Bottom: row-selectable top models table with visible scroll hint, cost percentage, and wider model/agent columns

### Tab 2: Models
- Full sortable table: #, Model, Agent, Tokens, Cost, %, Messages
- Quick text filter with `/`
- Models colored by detected company family (`OpenAI`, `Google`, `Anthropic`, `Others`)
- Numeric columns use semantic colors so `Cost`, `Tokens`, and `Msgs` stand out separately
- Sort by cost (c), tokens (t), or date (d)
- `%` follows the active sort basis: cost share for cost/date sort, token share for token sort

### Tab 3: Daily
- Top: summary bar with Today, This Week, This Month, period cost, tokens, messages, and sessions
- Bottom: daily table with today highlighted
- Daily numeric columns use distinct colors (`Tokens`, `Cost`, `Input`, `Output`, `Cache R`, `Cache W`, `Msgs`); cache read/write headers abbreviate to `CR`/`CW` on narrow terminals
- Wide terminals include a 7-day token trend column
- Sorted by date (most recent first) or cost/tokens

### Tab 4: Activity
- GitHub-style calendar layout with GitHub-green cost cells and Kaggle-blue token cells
- Solid-cell coloring uses five buckets at 20/40/60/80% of the visible window peak, without texture patterns in low activity cells
- 2 switchable metrics: total tokens and cost
- 3 window modes: past 26 weeks, past 52 weeks, past 365 days
- Narrow terminals clip to the most recent visible weeks instead of merging multiple dates into one cell
- Mouse-clickable cells — click any day to select it and see drill-down
- Clickable legend cells — click an intensity level to show its current value range
- The heatmap surface is theme-invariant, using soft gray background and cell border colors in both app themes so low activity levels remain visible and intensity direction stays consistent
- Drill-down: select any day to see token summary (with cache read/write split as `CR`/`CW`), agent totals, and per-agent model cost breakdown
- Selected-day panel supports scroll when the detail list is taller than the viewport, with a dedicated bottom-row scroll hint so the final token-detail line is not overwritten
- Streak tracking: current streak and longest streak

### Tab 5: Quota
- Live quota usage monitoring.
- Displays rate limits (e.g. Session 5h, Weekly 7d) as a progress bar with an expected-progress marker on top and a detail line directly below it (reset countdown, used/remaining percentages colored by the remaining balance, and an at-current-rate pace indicator that is omitted once the window is exhausted), with a blank row separating consecutive windows. Compact cards collapse to a single bar + percentage + reset countdown.
- Codex quota cards show each available manual rate-limit reset credit with its expiry when the reset-credit endpoint returns them, ordered by earliest expiry and collapsing later-expiring rows only when the terminal height is too constrained.
- The bar fill reflects remaining balance or used amount depending on the active display mode.

### Tab 6: Keeper
- Scheduled activation engine for Claude Code, Codex, and Antigravity — keeps sessions warm and anchors rate-limit windows at a chosen time.
- Disabled by default: every ping spends real quota, so the master switch (`keeper_engine` in Settings) must be turned on explicitly.
- Header bar shows the master switch state and the path to the config file, where wakeup times, models, prompts, and commands are edited.
- One card per agent with two independent switches, the next trigger time, the configured model, and the status of the last (or in-flight) run:
  - **5h daily wakeup** — fires at `daily_wakeup_time` (default `10:30`). A missed run is only caught up within 2 hours of that time, so launching the TUI late in the day does not fire a stale ping at the wrong hour.
  - **Weekly auto-sync** — fires 1 minute after the provider's weekly quota reset, read from the live quota snapshot.
- Last-fired dates persist to `keeper_state.json` beside the config, so restarting the TUI does not re-fire a trigger that already ran.
- Bottom panel is a scrollable execution stream: timestamp, agent, trigger, status, model, prompt, command, and reply. It is seeded from the `keeper_executions` table on launch and holds the newest 50 runs; the database keeps every run.
- Only one ping per agent runs at a time; a second trigger is skipped while one is in flight.

### Tab 7: Settings
- Live settings configuration panel for the application.
- Shows the read-only TokenPulse package version near the config file path.
- Configurable settings include:
  - `quota_display_mode` (toggle between `used` and `remaining` credit balance)
  - `show_empty_providers` (true / false)
  - `show_account` (true / false)
  - `auto_refresh_interval` (unified auto-refresh interval for quota + usage: 0, 1, 2, 5, 10, 15 min; 0 = disabled)
  - `theme` (cycle through auto / dark / light)
  - `scan_antigravity` (toggle active Antigravity session scanning and alias synchronization: true / false)
  - `refresh_quota` (enable / disable quota balance refresh on startup, auto-refresh, and manual `r`: true / false, default true)
  - `keeper_engine` (master switch for the Keeper tab's scheduled pings: true / false, default false)
  - Individual provider enabled/disabled switches
- Controls: use Up/Down (`j`/`k`) to navigate settings, and Space or Enter to cycle/toggle the selected setting.
- The same values are readable and writable outside the TUI with `tokenpulse config show` and `tokenpulse config set KEY=VALUE`.

### Source Filter Overlay
- Press `s` on any tab to open provider filter popup
- Toggle individual providers with space/enter
- Toggle all with `a`
- Close with `s` or `Esc`
- Filters apply to all views (chart, models, daily)

## Key Bindings

| Key                   | Action                                   |
| --------------------- | ---------------------------------------- |
| `q` / `Esc`           | Quit (close overlay if open)             |
| `←` / `→` / `h` / `l` | Switch tabs                              |
| `Tab` / `Shift+Tab`   | Next/previous tab                        |
| `j` / `↓`             | Move selected row down / next day        |
| `k` / `↑`             | Move selected row up / previous day      |
| `c`                   | Cost sort/metric, or overview cost chart |
| `t`                   | Token sort/metric, or overview token chart |
| `d`                   | Sort by date                             |
| `/`                   | Open Models quick filter                 |
| `Ctrl+L`              | Clear Models quick filter                |
| `s`                   | Open/close source filter overlay         |
| `w`                   | Cycle activity window (26w/52w/365d)     |
| `n`                   | Jump to today/now (Daily/Activity)       |
| `PgUp` / `PgDn`       | Scroll selected-day detail (Activity)    |
| `a`                   | Toggle all sources (in filter overlay)   |
| `Space` / `Enter`     | Toggle source (in filter overlay)        |
| `b`                   | Cycle and save theme (auto/dark/light)   |
| `?`                   | Open page help overlay                   |

Keeper tab only (`←` / `→` still switch tabs; agent cards are selected with `↑` / `↓` / `Tab`):

| Key         | Action                                        |
| ----------- | --------------------------------------------- |
| `1` / `d`   | Toggle the selected agent's 5h daily wakeup   |
| `2` / `w`   | Toggle the selected agent's weekly auto-sync  |
| `p`         | Run an immediate test ping for the selected agent |
| Mouse wheel | Scroll the execution stream                   |

## Event Loop

```rust
loop {
    terminal.draw(|f| {
        render_dashboard(f, size, &dashboard, &summary, &state, &theme);
        if state.show_source_filter {
            render_source_filter_overlay(f, size, &state, &theme);
        }
    })?;

    if event::poll(Duration::from_millis(100))? {
        match event::read()? {
            Event::Key(key) => {
                if state.show_source_filter {
                    handle_filter_keys(key);
                } else {
                    handle_page_keys(key);
                }
            }
            _ => {}
        }
    }
}
```

Non-blocking event loop. Data is fetched at startup and can be reloaded in place with `r`, with transient footer feedback for refresh progress and errors.

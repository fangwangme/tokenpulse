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

```
┌─────────────────────────────────────────────────────┐
│  Header: "TokenPulse - Quota" + timestamp           │
├─────────────────────────────────────────────────────┤
│  ┌─ Claude ──────────────────────────────────────┐  │
│  │  [gauge] Session   ████████░░░░ 42%  3h 12m   │  │
│  │  [gauge] Weekly    ███░░░░░░░░░ 18%  4d 6h    │  │
│  │  [gauge] Sonnet    ██████░░░░░░ 48%  4d 6h    │  │
│  │  [text]  Credits   $12.40 / $100.00            │  │
│  └───────────────────────────────────────────────┘  │
│  ┌─ Copilot ────────────────────────────────────┐  │
│  │  [gauge] Completions ███████░░░ 25%  29d      │  │
│  │  [gauge] Premium     ██░░░░░░░░ 10%  29d      │  │
│  └───────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────┤
│  Footer: q quit │ r refresh │ j/k scroll            │
└─────────────────────────────────────────────────────┘
```

## Usage View Layout

4 tabs switchable with ←/→:

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
- Daily numeric columns use distinct colors (`Tokens`, `Cost`, `Input`, `Output`, `Cache`, `Msgs`)
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
- The heatmap surface stays light in both app themes so low activity levels remain visible and intensity direction stays consistent
- Drill-down: select any day to see token summary, agent totals, and per-agent model cost breakdown
- Selected-day panel supports scroll when the detail list is taller than the viewport, with a dedicated bottom-row scroll hint so the final token-detail line is not overwritten
- Streak tracking: current streak and longest streak

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
| `T`                   | Jump to today (Daily/Activity)           |
| `PgUp` / `PgDn`       | Scroll selected-day detail (Activity)    |
| `a`                   | Toggle all sources (in filter overlay)   |
| `Space` / `Enter`     | Toggle source (in filter overlay)        |
| `b`                   | Cycle and save theme (auto/dark/light)   |
| `?`                   | Open page help overlay                   |

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

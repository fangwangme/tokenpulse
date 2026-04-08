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
│   ├── sparkline.rs    # mini trend charts (daily cost over time)
│   ├── barchart.rs     # stacked bar charts (provider breakdown)
│   ├── heatmap.rs      # GitHub-style contribution heatmap
│   └── table.rs        # styled tables with alternating rows
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

    // Provider colors
    pub claude: Color,          // #D97706 (amber)
    pub codex: Color,           // #10B981 (emerald)
    pub opencode: Color,        // #6366F1 (indigo)
    pub gemini: Color,          // #3B82F6 (blue)
    pub pi: Color,              // #EC4899 (pink)
    pub antigravity: Color,     // #F59E0B (yellow)
    pub copilot: Color,         // #8B5CF6 (purple)

    // Gauge gradient (low → high usage)
    pub gauge_low: Color,       // Green
    pub gauge_mid: Color,       // Yellow
    pub gauge_high: Color,      // Red

    // Heatmap palettes (GitHub-style green)
    pub token_heatmap: [Color; 5],
    pub cost_heatmap: [Color; 5],
    pub count_heatmap: [Color; 5],
}
```

### Model Color Detection

The `model_color()` method detects provider from model name and assigns a fixed color:

| Pattern                     | Provider  | Color          |
| --------------------------- | --------- | -------------- |
| claude, sonnet, opus, haiku | Anthropic | Coral #DA7756  |
| gpt, o1, o3, o4             | OpenAI    | Green #10B981  |
| gemini                      | Google    | Blue #3B82F6   |
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
- Top: stacked bar chart (daily tokens, last 60 days, colored by provider)
- Bottom: top 10 models by cost with provider-colored dots

### Tab 2: Models
- Full sortable table: #, Model, Provider, Tokens, Cost, Messages
- Models colored by detected provider family
- Sort by cost (c), tokens (t), or date (d)

### Tab 3: Daily
- Top: summary cards (total cost, tokens, messages, sessions)
- Bottom: daily table with today highlighted
- Sorted by date (most recent first) or cost/tokens

### Tab 4: Heatmap
- GitHub-style contribution graph (green palette)
- 7 switchable metrics: total tokens, cost, input, output, cache, messages, sessions
- 3 window modes: 26 weeks, 52 weeks, selected year
- Drill-down: select any day to see provider breakdown, token composition, top models
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
| `j` / `↓`             | Scroll down / next day (heatmap)         |
| `k` / `↑`             | Scroll up / previous day (heatmap)       |
| `c`                   | Sort by cost / set cost metric           |
| `t`                   | Sort by tokens / set total tokens metric |
| `d`                   | Sort by date                             |
| `s`                   | Open/close source filter overlay         |
| `w`                   | Cycle heatmap window (26w/52w/year)      |
| `i` / `o` / `x`       | Input/output/cache metrics (heatmap)     |
| `m` / `n`             | Messages/sessions metrics (heatmap)      |
| `a`                   | Toggle all sources (in filter overlay)   |
| `Space` / `Enter`     | Toggle source (in filter overlay)        |

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

Non-blocking event loop. Data is fetched once at startup.

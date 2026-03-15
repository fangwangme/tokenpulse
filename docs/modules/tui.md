# TUI Module - Detailed Design

## Overview

Fancy terminal dashboard using `ratatui` + `crossterm`. Two modes: quota view and usage view.

## Architecture

```
tui/
├── mod.rs              # App state, event loop, mode switching
├── theme.rs            # color palette, styles, gradients
├── widgets/
│   ├── mod.rs
│   ├── gauge.rs        # gradient progress bars with percentage + labels
│   ├── sparkline.rs    # mini trend charts (daily cost over time)
│   ├── barchart.rs     # stacked/grouped bar charts
│   └── table.rs        # styled tables with alternating rows
└── views/
    ├── mod.rs
    ├── quota.rs        # quota dashboard layout
    └── usage.rs        # usage dashboard with tabs
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
    pub pi: Color,              // #EC4899 (pink)

    // Gauge gradient (low → high usage)
    pub gauge_low: Color,       // Green
    pub gauge_mid: Color,       // Yellow
    pub gauge_high: Color,      // Red
}
```

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
│  ┌─ Codex ───────────────────────────────────────┐  │
│  │  [gauge] Session   █████████████ 67%  1h 45m  │  │
│  │  [gauge] Weekly    ████████░░░░ 31%  5d 2h    │  │
│  │  [text]  Credits   $45.20 (unlimited)          │  │
│  └───────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────┤
│  Footer: q quit │ r refresh │ j/k scroll            │
└─────────────────────────────────────────────────────┘
```

### Gauge Widget

Custom gradient gauge:
- 0-50%: green gradient
- 50-75%: yellow gradient
- 75-100%: red gradient
- Shows: `[label] [bar] [percent] [time until reset]`

## Usage View Layout

4 tabs switchable with ←/→:

### Tab 1: Overview
- Top: stacked bar chart (daily cost, last 14 days, colored by provider)
- Middle: provider breakdown (horizontal bars with cost)
- Bottom: summary stats (total cost, active days, avg/day)

### Tab 2: Daily
- Full-width table: Date | Provider | Input | Output | Cache | Cost
- Sorted by date descending
- Alternating row colors

### Tab 3: Models
- Bar chart: cost per model
- Table: model details (provider, tokens, cost, % of total)

### Tab 4: Sessions
- Table: session list with timestamp, provider, model, duration, tokens, cost
- Sorted by most recent

## Key Bindings

| Key | Action |
|---|---|
| `q` / `Esc` | Quit |
| `r` | Re-fetch / re-parse data |
| `j` / `↓` | Scroll down |
| `k` / `↑` | Scroll up |
| `←` / `→` | Switch tabs (usage view) |
| `Tab` | Next tab |
| `1-4` | Jump to tab |

## Event Loop

```rust
loop {
    terminal.draw(|f| app.render(f))?;

    if event::poll(Duration::from_millis(100))? {
        match event::read()? {
            Event::Key(key) => app.handle_key(key),
            Event::Resize(w, h) => app.resize(w, h),
            _ => {}
        }
    }

    if app.should_quit { break; }
}
```

Non-blocking event loop. Data is fetched once at startup. Press `r` to manually re-fetch.

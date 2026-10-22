# TUI Theme and Background Passthrough

## Background Behavior

The TUI uses `ratatui` and `crossterm`. To allow host terminal styling (native background colors, terminal themes, window transparency, and background blur) to pass through cleanly, the base dashboard background does not enforce a solid opaque RGB color block across the terminal viewport.

- `Theme.bg` is `Color::Reset` for both dark and light theme modes.
- When drawing the root dashboard (`render_dashboard`), no solid full-viewport background widget is rendered during normal operation. The viewport preserves default cell background attributes, allowing the terminal emulator's background to show through.
- When an active perimeter animation or recovery notification pulse is present, temporary glow accents may tint or outline the designated boundary or background areas as required by the notification system.
- Explicit visual styling on foreground elements, borders, active tabs, selected rows, status badges, and heatmap grid cells remain active with their designated colors.

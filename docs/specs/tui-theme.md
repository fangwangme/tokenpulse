# TUI Theme and Background Passthrough

## Background Behavior

The TUI uses Bubble Tea and Lip Gloss. To allow host terminal styling (native background colors, terminal themes, window transparency, and background blur) to pass through cleanly, the base dashboard background does not enforce a solid opaque RGB color block across the terminal viewport.

- `Theme.bg` is `Color::Reset` for both dark and light theme modes.
- When drawing the root dashboard (`render_dashboard`), no solid full-viewport background widget is rendered during normal operation. The viewport preserves default cell background attributes, allowing the terminal emulator's background to show through.
- When an active perimeter animation or recovery notification pulse is present, temporary glow accents may tint or outline the designated boundary or background areas as required by the notification system.
- Explicit visual styling on foreground elements, borders, active tabs, selected rows, status badges, and heatmap grid cells remain active with their designated colors.

## Auto Theme Detection And Refresh

When `display.theme` is `auto` and no manual `b` override is active, the TUI resolves dark or light mode using the 0.6.16 precedence:

1. `TOKENPULSE_THEME` (`light` or `dark`, case-insensitive).
2. Terminal background luminance via an OSC 11 query once at startup when attached to a TTY (never from inside the Bubble Tea event loop or non-TTY sessions).
3. `COLORFGBG` (ANSI background index: `<= 6` or `8` is dark, otherwise light).
4. `TERM_BACKGROUND` (`light` or `dark`, case-insensitive).
5. OS appearance: on Linux, Freedesktop Portal `org.freedesktop.appearance` `color-scheme` via `busctl`, falling back to `gsettings get org.gnome.desktop.interface color-scheme`; on macOS, `defaults read -g AppleInterfaceStyle`. Default to dark if unavailable.

While running in `auto` mode without a manual override or explicit `TOKENPULSE_THEME` / `TERM_BACKGROUND` override, the TUI periodically polls OS appearance (skipping OSC 11). The first reading is only a baseline; the active theme changes when a later reading differs from the previous one, so a startup result from a higher-precedence source (such as OSC 11 on a dark terminal under a light OS) is kept until the OS actually switches.

## Light-Mode Contrast

In light mode, company colors (`anthropic`, `openai`, `google`, `other`), metric highlight colors (Output, Cache R/W), and Quota pace markers use dark/high-contrast foregrounds against light terminal backgrounds rather than dark-mode pastels or pure white.

package tui

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"strings"
	"time"

	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/config"
)

func rgb(r, g, b int) lipgloss.Color {
	return lipgloss.Color(fmt.Sprintf("#%02x%02x%02x", r, g, b))
}

// tuiTheme is the port of the 0.6.16 Rust theme (theme.rs). Both modes keep a
// reset background so the terminal's own background shows through.
type tuiTheme struct {
	mode int // 0 dark, 1 light

	fg, dim, border, accent, accentSoft, onAccent lipgloss.Color
	pillBg, costColor, outputColor, cacheColor    lipgloss.Color

	claude, codex, opencode, gemini, pi, antigravity, copilot lipgloss.Color

	gaugeLow, gaugeMid, gaugeHigh lipgloss.Color

	tokenHeatmap [5]lipgloss.Color
	costHeatmap  [5]lipgloss.Color
	heatmapBg    lipgloss.Color
	emptyHeatmap lipgloss.Color
	selectedBg   lipgloss.Color
	todayBg      lipgloss.Color
}

func newTheme(mode int) tuiTheme {
	if mode == 1 {
		return lightTheme()
	}
	return darkTheme()
}

func darkTheme() tuiTheme {
	return tuiTheme{
		mode:        0,
		fg:          rgb(232, 238, 247),
		dim:         rgb(144, 158, 181),
		border:      rgb(58, 76, 99),
		accent:      rgb(45, 212, 191),
		accentSoft:  rgb(125, 211, 252),
		onAccent:    rgb(4, 18, 24),
		pillBg:      rgb(30, 41, 59),
		costColor:   rgb(250, 204, 21), // Mobile Web figCost (dark)
		outputColor: rgb(167, 139, 250),
		cacheColor:  rgb(251, 146, 60),

		claude:      rgb(251, 146, 60),
		codex:       rgb(52, 211, 153),
		opencode:    rgb(129, 140, 248),
		gemini:      rgb(96, 165, 250),
		pi:          rgb(244, 114, 182),
		antigravity: rgb(96, 165, 250),
		copilot:     rgb(163, 230, 53),

		gaugeLow:  rgb(52, 211, 153),
		gaugeMid:  rgb(250, 204, 21),
		gaugeHigh: rgb(248, 113, 113),

		tokenHeatmap: [5]lipgloss.Color{
			rgb(184, 236, 255), rgb(128, 224, 255), rgb(32, 190, 255), rgb(0, 148, 209), rgb(0, 92, 134),
		},
		costHeatmap: [5]lipgloss.Color{
			rgb(155, 233, 168), rgb(64, 196, 99), rgb(48, 161, 78), rgb(33, 110, 57), rgb(14, 68, 41),
		},
		heatmapBg:    rgb(235, 237, 240),
		emptyHeatmap: rgb(235, 237, 240),
		selectedBg:   rgb(51, 65, 85),
		todayBg:      rgb(22, 78, 99),
	}
}

func lightTheme() tuiTheme {
	theme := darkTheme()
	theme.mode = 1
	theme.fg = rgb(15, 23, 42)
	theme.dim = rgb(71, 85, 105)
	theme.border = rgb(148, 163, 184)
	theme.accent = rgb(13, 148, 136)
	theme.accentSoft = rgb(3, 105, 161)
	theme.onAccent = rgb(255, 255, 255)
	theme.pillBg = rgb(226, 232, 240)
	theme.costColor = rgb(161, 98, 7) // Mobile Web figCost (light), distinct from Claude terracotta
	theme.outputColor = rgb(109, 40, 217)
	theme.cacheColor = rgb(194, 65, 12)
	theme.claude = rgb(194, 65, 12)
	theme.codex = rgb(4, 120, 87)
	theme.opencode = rgb(67, 56, 202)
	theme.gemini = rgb(29, 78, 216)
	theme.pi = rgb(190, 24, 93)
	theme.antigravity = rgb(29, 78, 216)
	theme.copilot = rgb(77, 124, 15)
	theme.gaugeLow = rgb(4, 120, 87)
	theme.gaugeMid = rgb(180, 83, 9)
	theme.gaugeHigh = rgb(185, 28, 28)
	theme.selectedBg = rgb(203, 213, 225)
	theme.todayBg = rgb(196, 181, 253)
	return theme
}

func (t tuiTheme) providerColor(source string) lipgloss.Color {
	switch source {
	case "claude":
		return t.claude
	case "codex":
		return t.codex
	case "opencode":
		return t.opencode
	case "gemini":
		return t.gemini
	case "pi":
		return t.pi
	case "antigravity":
		return t.antigravity
	case "copilot":
		return t.copilot
	default:
		return t.fg
	}
}

func (t tuiTheme) companyColor(company string) lipgloss.Color {
	if t.mode == 1 {
		switch company {
		case "openai":
			return rgb(4, 120, 87)
		case "google":
			return rgb(29, 78, 216)
		case "anthropic":
			return rgb(194, 65, 12)
		default:
			return rgb(126, 34, 206)
		}
	}
	switch company {
	case "openai":
		return rgb(52, 211, 153)
	case "google":
		return rgb(96, 165, 250)
	case "anthropic":
		return rgb(251, 146, 60)
	default:
		return rgb(168, 85, 247)
	}
}

// gaugeColor is quota.md's status color: danger when exhausted or ahead of
// pace, warn when the level is warn or the pace is on track, good otherwise.
func (t tuiTheme) gaugeColor(level string, pace string) lipgloss.Color {
	if level == "exhausted" || pace == "ahead" {
		return t.gaugeHigh
	}
	if level == "warn" || pace == "on_track" {
		return t.gaugeMid
	}
	return t.gaugeLow
}

func resolveTheme(cfg config.Config, override int) tuiTheme {
	return resolveThemeWithAuto(cfg, override, -1)
}

func resolveThemeWithAuto(cfg config.Config, override, autoMode int) tuiTheme {
	if override >= 0 {
		return newTheme(override)
	}
	switch cfg.Display.Theme {
	case config.ThemeLight:
		return lightTheme()
	case config.ThemeDark:
		return darkTheme()
	default:
		if mode, ok := detectEnvThemeOverride(); ok {
			return newTheme(mode)
		}
		if autoMode >= 0 {
			return newTheme(autoMode)
		}
		return newTheme(detectAutoThemeMode())
	}
}

func detectEnvThemeOverride() (int, bool) {
	if v := strings.ToLower(strings.TrimSpace(os.Getenv("TOKENPULSE_THEME"))); v == "light" {
		return 1, true
	} else if v == "dark" {
		return 0, true
	}
	return 0, false
}

func hasExplicitTermBackground() bool {
	if _, ok := detectEnvThemeOverride(); ok {
		return true
	}
	v := strings.ToLower(strings.TrimSpace(os.Getenv("TERM_BACKGROUND")))
	return v == "light" || v == "dark"
}

func detectAutoThemeMode() int {
	if mode, ok := detectEnvThemeOverride(); ok {
		return mode
	}
	if mode, ok := parseColorFGBG(os.Getenv("COLORFGBG")); ok {
		return mode
	}
	if v := strings.ToLower(strings.TrimSpace(os.Getenv("TERM_BACKGROUND"))); v == "light" {
		return 1
	} else if v == "dark" {
		return 0
	}
	if mode, ok := detectSystemAppearance(); ok {
		return mode
	}
	return 0
}

// detectStartupThemeMode runs once before Bubble Tea takes over stdin so it
// can query the terminal background safely via lipgloss (OSC 11) when attached
// to a TTY.
func detectStartupThemeMode() int {
	if mode, ok := detectEnvThemeOverride(); ok {
		return mode
	}
	if isTerminal(os.Stdin) && isTerminal(os.Stdout) {
		if lipgloss.HasDarkBackground() {
			return 0
		}
		return 1
	}
	return detectAutoThemeMode()
}

func isTerminal(f *os.File) bool {
	info, err := f.Stat()
	return err == nil && (info.Mode()&os.ModeCharDevice) != 0
}

func parseColorFGBG(value string) (int, bool) {
	value = strings.TrimSpace(value)
	if value == "" {
		return 0, false
	}
	parts := strings.Split(value, ";")
	bg, err := strconv.Atoi(strings.TrimSpace(parts[len(parts)-1]))
	if err != nil {
		return 0, false
	}
	if bg <= 6 || bg == 8 {
		return 0, true
	}
	return 1, true
}

func parseOSC11ThemeResponse(raw []byte) (int, bool) {
	text := string(raw)
	idx := strings.Index(text, "rgb:")
	if idx < 0 {
		return 0, false
	}
	rest := text[idx+4:]
	if end := strings.IndexAny(rest, "\x07\x1b"); end >= 0 {
		rest = rest[:end]
	}
	parts := strings.Split(rest, "/")
	if len(parts) != 3 {
		return 0, false
	}
	r, okR := parseOSCColorComponent(parts[0])
	g, okG := parseOSCColorComponent(parts[1])
	b, okB := parseOSCColorComponent(parts[2])
	if !okR || !okG || !okB {
		return 0, false
	}
	// Relative luminance threshold matching 0.6.16 theme.rs.
	luminance := 0.2126*float64(r) + 0.7152*float64(g) + 0.0722*float64(b)
	if luminance >= 128 {
		return 1, true
	}
	return 0, true
}

func parseOSCColorComponent(hex string) (int, bool) {
	hex = strings.TrimSpace(hex)
	if len(hex) == 0 || len(hex) > 4 {
		return 0, false
	}
	val, err := strconv.ParseUint(hex, 16, 32)
	if err != nil {
		return 0, false
	}
	maxVal := (uint64(1) << (uint(len(hex)) * 4)) - 1
	return int((val*255 + maxVal/2) / maxVal), true
}

func parseBusctlColorScheme(stdout string) (int, bool) {
	fields := strings.Fields(stdout)
	if len(fields) == 0 {
		return 0, false
	}
	switch fields[len(fields)-1] {
	case "1":
		return 0, true
	case "2":
		return 1, true
	default:
		return 0, false
	}
}

func parseGsettingsColorScheme(stdout string) (int, bool) {
	trimmed := strings.Trim(strings.TrimSpace(stdout), "'\"")
	switch strings.ToLower(trimmed) {
	case "prefer-dark":
		return 0, true
	case "prefer-light":
		return 1, true
	default:
		return 0, false
	}
}

// detectSystemAppearance queries the OS appearance without touching stdin,
// making it safe to call periodically while Bubble Tea owns the terminal.
func detectSystemAppearance() (int, bool) {
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	switch runtime.GOOS {
	case "darwin":
		out, err := exec.CommandContext(ctx, "defaults", "read", "-g", "AppleInterfaceStyle").Output()
		if err == nil && strings.EqualFold(strings.TrimSpace(string(out)), "dark") {
			return 0, true
		}
		return 1, true
	case "linux":
		out, err := exec.CommandContext(ctx, "busctl", "--user", "--timeout=1", "call",
			"org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop",
			"org.freedesktop.portal.Settings", "Read", "ss",
			"org.freedesktop.appearance", "color-scheme").Output()
		if err == nil {
			if mode, ok := parseBusctlColorScheme(string(out)); ok {
				return mode, true
			}
		}
		out, err = exec.CommandContext(ctx, "gsettings", "get", "org.gnome.desktop.interface", "color-scheme").Output()
		if err == nil {
			if mode, ok := parseGsettingsColorScheme(string(out)); ok {
				return mode, true
			}
		}
		return 0, false
	default:
		return 0, false
	}
}

func (t tuiTheme) heatmapPalette(metric int) [5]lipgloss.Color {
	if metric == 1 {
		return t.costHeatmap
	}
	return t.tokenHeatmap
}

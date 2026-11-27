package tui

import (
	"fmt"

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
		mode:       0,
		fg:         rgb(232, 238, 247),
		dim:        rgb(144, 158, 181),
		border:     rgb(58, 76, 99),
		accent:     rgb(45, 212, 191),
		accentSoft: rgb(125, 211, 252),
		onAccent:   rgb(4, 18, 24),

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
	if override >= 0 {
		return newTheme(override)
	}
	switch cfg.Display.Theme {
	case config.ThemeLight:
		return lightTheme()
	default:
		return darkTheme()
	}
}

func (t tuiTheme) heatmapPalette(metric int) [5]lipgloss.Color {
	if metric == 1 {
		return t.costHeatmap
	}
	return t.tokenHeatmap
}

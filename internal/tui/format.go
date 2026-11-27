package tui

import (
	"fmt"
	"strings"
	"time"

	"github.com/charmbracelet/lipgloss"

	money "github.com/fangwangme/tokenpulse/internal/format"
)

// truncate keeps a string within width runes; width 0 returns an empty string
// so a zero-width column cannot push the next column over.
func truncate(value string, width int) string {
	if width <= 0 {
		return ""
	}
	runes := []rune(value)
	if len(runes) <= width {
		return value
	}
	if width == 1 {
		return string(runes[:1])
	}
	return string(runes[:width-1]) + "…"
}

func padRight(value string, width int) string {
	value = truncate(value, width)
	if gap := width - lipgloss.Width(value); gap > 0 {
		return value + strings.Repeat(" ", gap)
	}
	return value
}

func padLeft(value string, width int) string {
	value = truncate(value, width)
	if gap := width - lipgloss.Width(value); gap > 0 {
		return strings.Repeat(" ", gap) + value
	}
	return value
}

func formatCompact(value int64) string {
	abs := value
	if abs < 0 {
		abs = -abs
	}
	switch {
	case abs >= 1_000_000_000:
		return fmt.Sprintf("%.2f B", float64(value)/1_000_000_000)
	case abs >= 1_000_000:
		return fmt.Sprintf("%.2f M", float64(value)/1_000_000)
	case abs >= 1_000:
		return fmt.Sprintf("%.2f K", float64(value)/1_000)
	default:
		return fmt.Sprintf("%.2f  ", float64(value))
	}
}

func formatIntCommas(value int64) string {
	raw := fmt.Sprintf("%d", value)
	negative := strings.HasPrefix(raw, "-")
	if negative {
		raw = raw[1:]
	}
	var parts []string
	for len(raw) > 3 {
		parts = append([]string{raw[len(raw)-3:]}, parts...)
		raw = raw[:len(raw)-3]
	}
	parts = append([]string{raw}, parts...)
	out := strings.Join(parts, ",")
	if negative {
		return "-" + out
	}
	return out
}

func formatCost(value float64) string { return money.Cost(value) }

func percentDeltaText(current float64, prior *float64) string {
	if prior == nil || *prior <= 0 {
		return "—"
	}
	pct := (current - *prior) / *prior * 100
	if pct >= 0 {
		return fmt.Sprintf("%+.1f%%↑", pct)
	}
	return fmt.Sprintf("%+.1f%%↓", pct)
}

func percentDeltaColor(theme tuiTheme, current float64, prior *float64) lipgloss.Color {
	if prior == nil || *prior <= 0 {
		return theme.dim
	}
	if current > *prior {
		return theme.gaugeHigh
	}
	if current < *prior {
		return theme.gaugeLow
	}
	return theme.dim
}

func cacheHitPercent(cacheRead, tokens int64) string {
	if tokens <= 0 {
		return "0.00%"
	}
	value := float64(cacheRead) / float64(tokens) * 100
	if value < 0 {
		value = 0
	}
	if value > 100 {
		value = 100
	}
	return fmt.Sprintf("%.2f%%", value)
}

func displayAgentName(agent string) string {
	switch agent {
	case "claude":
		return "Claude Code"
	case "codex":
		return "Codex"
	case "copilot":
		return "Copilot CLI"
	case "opencode":
		return "OpenCode"
	case "gemini":
		return "Gemini CLI"
	case "pi":
		return "PI"
	case "antigravity":
		return "Antigravity"
	default:
		return agent
	}
}

func displaySourceList(agents []string) string {
	if len(agents) == 0 {
		return "Unknown"
	}
	seen := map[string]bool{}
	var names []string
	for _, agent := range agents {
		name := displayAgentName(agent)
		if seen[name] {
			continue
		}
		seen[name] = true
		names = append(names, name)
	}
	return strings.Join(names, ", ")
}

func formatScanned(ms int64) string {
	if ms == 0 {
		return "never"
	}
	return time.UnixMilli(ms).Format("15:04")
}

func formatResetDuration(d time.Duration) string {
	minutes := int(d.Minutes())
	if minutes < 0 {
		minutes = 0
	}
	hours := minutes / 60
	days := hours / 24
	switch {
	case minutes > 24*60:
		return fmt.Sprintf("%dd %dh", days, hours%24)
	case hours > 0:
		return fmt.Sprintf("%dh %dm", hours, minutes%60)
	default:
		return fmt.Sprintf("%dm", minutes)
	}
}

func compactDuration(ms int64) string {
	if ms <= 0 {
		return ""
	}
	units := []struct {
		ms     int64
		suffix string
	}{
		{24 * 60 * 60 * 1000, "D"},
		{60 * 60 * 1000, "H"},
		{60 * 1000, "M"},
		{1000, "S"},
	}
	for _, unit := range units {
		if ms%unit.ms == 0 {
			return fmt.Sprintf("%d%s", ms/unit.ms, unit.suffix)
		}
	}
	return fmt.Sprintf("%dMS", ms)
}

// gaugeFill clamps a quota percentage to a bar width, so out-of-range
// provider data cannot draw a negative or oversized bar.
func gaugeFill(percent float64, width int) int {
	if width <= 0 || !(percent > 0) {
		return 0
	}
	if percent > 100 {
		percent = 100
	}
	return int(percent/100*float64(width) + 0.5)
}

func sparklineText(values []int64) string {
	var max int64
	for _, value := range values {
		if value > max {
			max = value
		}
	}
	var b strings.Builder
	for _, value := range values {
		if max == 0 {
			b.WriteString("▁")
			continue
		}
		levels := []rune("▁▂▃▄▅▆▇█")
		index := int(float64(value)/float64(max)*7 + 0.5)
		if index < 0 {
			index = 0
		}
		if index > 7 {
			index = 7
		}
		b.WriteRune(levels[index])
	}
	return b.String()
}

func parseDate(value string) (time.Time, error) {
	return time.ParseInLocation("2006-01-02", value, time.Local)
}

func dateString(value time.Time) string { return value.Format("2006-01-02") }

func startOfToday() time.Time {
	now := time.Now()
	return time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.Local)
}

func mondayOf(day time.Time) time.Time {
	offset := (int(day.Weekday()) + 6) % 7
	return time.Date(day.Year(), day.Month(), day.Day(), 0, 0, 0, 0, time.Local).AddDate(0, 0, -offset)
}

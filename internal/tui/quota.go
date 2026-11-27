package tui

import (
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/config"
	money "github.com/fangwangme/tokenpulse/internal/format"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/quota"
)

func (m *model) drawQuota(c *canvas, area rect) {
	theme := m.theme()
	if len(m.snapshots) == 0 {
		c.box(area.x, area.y, area.w, area.h, "", textStyle{}, fgStyle(theme.border))
		message := "No quota data available for enabled providers"
		if m.quotaLoading {
			message = "Loading quota…"
		}
		c.set(area.x+2, area.y+1, truncate(message, max(area.w-4, 1)), fgStyle(theme.dim))
		return
	}
	if len(m.snapshots) == 1 {
		maxHeight := 14
		if len(m.snapshots[0].Windows) > 4 {
			maxHeight = 16
		}
		height := min(maxHeight, area.h)
		height = max(height, min(8, area.h))
		m.drawQuotaCard(c, rect{x: area.x, y: area.y, w: area.w, h: height}, m.snapshots[0])
		return
	}

	var small, tall []provider.QuotaSnapshot
	for _, snapshot := range m.snapshots {
		if len(snapshot.Windows) <= 4 {
			small = append(small, snapshot)
		} else {
			tall = append(tall, snapshot)
		}
	}
	if len(small) == 0 || len(tall) == 0 {
		maxHeight := 16
		if len(tall) == 0 {
			maxHeight = 13
		}
		m.drawQuotaGrid(c, area, m.snapshots, maxHeight)
		return
	}
	columns := 1
	if area.w >= 110 {
		columns = 2
	}
	smallRows := (len(small) + columns - 1) / columns
	tallRows := (len(tall) + columns - 1) / columns
	smallHeight := min(smallRows*11, max(area.h-tallRows*9, 0))
	tallHeight := max(area.h-smallHeight, 0)
	if smallHeight < 3 {
		smallHeight = min(11, area.h)
		tallHeight = max(area.h-smallHeight, 0)
	}
	m.drawQuotaGrid(c, rect{x: area.x, y: area.y, w: area.w, h: smallHeight}, small, 11)
	if tallHeight > 0 {
		m.drawQuotaGrid(c, rect{x: area.x, y: area.y + smallHeight, w: area.w, h: tallHeight}, tall, 16)
	}
}

func (m *model) drawQuotaGrid(c *canvas, area rect, snapshots []provider.QuotaSnapshot, maxRowHeight int) {
	if len(snapshots) == 0 || area.h <= 0 || area.w <= 0 {
		return
	}
	columns := 1
	if area.w >= 110 {
		columns = 2
	}
	rows := (len(snapshots) + columns - 1) / columns
	rowHeight := min(max(area.h/rows, 1), maxRowHeight)
	for row := 0; row < rows; row++ {
		y := area.y + row*rowHeight
		if y >= area.bottom() {
			break
		}
		height := min(rowHeight, area.bottom()-y)
		columnsArea := splitH(rect{x: area.x, y: y, w: area.w, h: height}, columns)
		for column := 0; column < columns; column++ {
			index := row*columns + column
			if index >= len(snapshots) {
				break
			}
			m.drawQuotaCard(c, columnsArea[column], snapshots[index])
		}
	}
}

func (m *model) drawQuotaCard(c *canvas, area rect, snapshot provider.QuotaSnapshot) {
	theme := m.theme()
	if area.w < 4 || area.h <= 0 {
		return
	}
	if area.h < 3 {
		m.drawCompactQuotaRow(c, area, snapshot)
		return
	}
	c.box(area.x, area.y, area.w, area.h, " "+quota.DisplayName(snapshot.Provider)+" ", boldStyle(theme.providerColor(snapshot.Provider)), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 || inner.w <= 0 {
		return
	}
	y := inner.y

	if message := m.quotaErrors[snapshot.Provider]; message != "" && y < inner.bottom() {
		c.set(inner.x, y, truncate(message, inner.w), fgStyle(theme.gaugeHigh))
		y++
	}
	if len(snapshot.Windows) == 0 && y < inner.bottom() {
		c.set(inner.x, y, "No quota data", fgStyle(theme.dim))
		return
	}

	cfg := m.configValue()
	accountDisplay := cfg.Display.AccountDisplay
	hasPlan := snapshot.Plan != "" && accountDisplay.ShowsPlan()
	hasAccount := snapshot.Account != "" && accountDisplay.ShowsAccount()
	if (hasPlan || hasAccount) && y < inner.bottom() {
		var spans []string
		if hasPlan {
			spans = append(spans, "PLAN: "+displayPlanName(snapshot.Plan))
		}
		if hasAccount {
			spans = append(spans, snapshot.Account)
		}
		c.set(inner.x, y, truncate(strings.Join(spans, " · "), inner.w), fgStyle(theme.fg))
		y++
	}

	windows := quota.OrderWindows(snapshot.Windows)
	if len(windows) == 0 {
		return
	}
	tags := windowTags(windows)

	fixed := 0
	hasCredits := snapshot.Credits != nil || snapshot.ExtraUsage != nil
	if hasCredits {
		fixed++
	}
	available := inner.bottom() - y - fixed
	if available <= 0 {
		available = 1
	}
	linesPerWindow := 2
	compact := available < len(windows)*2
	if compact {
		linesPerWindow = 1
	}
	maxWindows := max(available/linesPerWindow, 1)
	if len(windows) > maxWindows {
		windows = windows[:maxWindows]
		tags = tags[:maxWindows]
	}

	displayMode := cfg.Display.QuotaDisplayMode
	for index, window := range windows {
		if y >= inner.bottom() {
			break
		}
		tag := tags[index]
		if compact {
			reset := ""
			if window.ResetsAt != nil {
				reset = formatResetDuration(time.Until(*window.ResetsAt))
			}
			m.drawQuotaGaugeLine(c, rect{x: inner.x, y: y, w: inner.w, h: 1}, window, tag, displayMode, true, reset)
			y++
			continue
		}
		m.drawQuotaGaugeLine(c, rect{x: inner.x, y: y, w: inner.w, h: 1}, window, tag, displayMode, false, "")
		y++
		if y < inner.bottom() {
			m.drawQuotaDetailLine(c, rect{x: inner.x, y: y, w: inner.w, h: 1}, window, tag, displayMode)
			y++
		}
		if index+1 < len(windows) && y < inner.bottom()-fixed {
			y++
		}
	}

	if hasCredits && y < inner.bottom() {
		if snapshot.ExtraUsage != nil {
			c.set(inner.x, y, truncate(creditText("Extra usage", "Extra balance", snapshot.ExtraUsage, displayMode), inner.w), fgStyle(theme.dim))
			y++
		}
		if snapshot.Credits != nil && y < inner.bottom() {
			c.set(inner.x, y, truncate(creditText("Credits", "Balance", snapshot.Credits, displayMode), inner.w), fgStyle(theme.dim))
			y++
		}
	}
	m.drawResetCredits(c, rect{x: inner.x, y: y, w: inner.w, h: inner.bottom() - y}, snapshot)
}

func (m *model) drawCompactQuotaRow(c *canvas, area rect, snapshot provider.QuotaSnapshot) {
	theme := m.theme()
	name := quota.DisplayName(snapshot.Provider)
	c.setClip(area.x, area.y, area.right(), name, boldStyle(theme.providerColor(snapshot.Provider)))
	x := area.x + len([]rune(name)) + 2
	if message := m.quotaErrors[snapshot.Provider]; message != "" {
		c.setClip(x, area.y, area.right(), message, fgStyle(theme.gaugeHigh))
		return
	}
	windows := quota.OrderWindows(snapshot.Windows)
	if len(windows) == 0 {
		c.setClip(x, area.y, area.right(), "No quota data", fgStyle(theme.dim))
		return
	}
	cfg := m.configValue()
	tags := windowTags(windows)
	if area.h >= 2 {
		reset := ""
		if windows[0].ResetsAt != nil {
			reset = formatResetDuration(time.Until(*windows[0].ResetsAt))
		}
		m.drawQuotaGaugeLine(c, rect{x: area.x, y: area.y + 1, w: area.w, h: 1}, windows[0], tags[0], cfg.Display.QuotaDisplayMode, true, reset)
		return
	}
	var parts []string
	for i, w := range windows {
		shown := w.UsedPercent
		if cfg.Display.QuotaDisplayMode == config.QuotaRemaining {
			shown = clampFloat(100-shown, 0, 100)
		}
		parts = append(parts, fmt.Sprintf("%s %.0f%%", tags[i], shown))
	}
	c.setClip(x, area.y, area.right(), strings.Join(parts, " · "), fgStyle(theme.fg))
}

func (m *model) drawQuotaGaugeLine(c *canvas, area rect, window provider.RateWindow, tag string, displayMode config.QuotaDisplayMode, compact bool, reset string) {
	theme := m.theme()
	if area.w <= 0 {
		return
	}
	shown := window.UsedPercent
	if displayMode == config.QuotaRemaining {
		shown = 100 - shown
	}
	if shown < 0 {
		shown = 0
	}
	if shown > 100 {
		shown = 100
	}
	pace := quota.ComputePace(window, time.Now())
	paceKind := ""
	if pace != nil {
		paceKind = pace.Kind
	}
	level := quota.Level(window.UsedPercent)
	color := theme.gaugeColor(level, paceKind)

	var expected *float64
	if pace != nil {
		value := pace.ExpectedPercent
		if displayMode == config.QuotaRemaining {
			value = 100 - value
		}
		value = clampFloat(value, 0, 100)
		expected = &value
	}

	if compact {
		trailing := fmt.Sprintf("%3.0f%%", shown)
		if reset != "" && window.UsedPercent > 0 && window.UsedPercent < 100 {
			trailing += " " + reset
		}
		label := truncate(tag, max(area.w-22, 3)) + ": "
		barWidth := area.w - len([]rune(label)) - len([]rune(trailing)) - 1
		if barWidth < 1 {
			c.set(area.x, area.y, truncate(label+trailing, area.w), fgStyle(color))
			return
		}
		c.set(area.x, area.y, label, boldStyle(theme.fg))
		drawGauge(c, area.x+len([]rune(label)), area.y, barWidth, shown, expected, color, theme)
		c.set(area.x+len([]rune(label))+barWidth+1, area.y, trailing, fgStyle(color))
		return
	}
	drawGauge(c, area.x, area.y, area.w, shown, expected, color, theme)
}

func drawGauge(c *canvas, x, y, width int, percent float64, expected *float64, color lipgloss.Color, theme tuiTheme) {
	if width <= 0 {
		return
	}
	filled := gaugeFill(percent, width)
	marker := -1
	if expected != nil {
		marker = int(*expected/100*float64(width) + 0.5)
	}
	for i := 0; i < width; i++ {
		char := "░"
		style := fgStyle(theme.border)
		if i < filled {
			char = "█"
			style = fgStyle(color)
		}
		if i == marker && i >= filled {
			char = "▏"
			style = boldStyle(theme.fg)
		}
		c.set(x+i, y, char, style)
	}
}

func (m *model) drawQuotaDetailLine(c *canvas, area rect, window provider.RateWindow, tag string, displayMode config.QuotaDisplayMode) {
	theme := m.theme()
	if area.w <= 0 {
		return
	}
	x := area.x
	c.set(x, area.y, tag+": ", boldStyle(theme.fg))
	x += len([]rune(tag)) + 2
	level := quota.Level(window.UsedPercent)
	pace := quota.ComputePace(window, time.Now())
	paceKind := ""
	if pace != nil {
		paceKind = pace.Kind
	}
	color := theme.gaugeColor(level, paceKind)
	fullyReset := window.UsedPercent <= 0
	if !fullyReset && window.ResetsAt != nil {
		reset := "reset " + formatResetDuration(time.Until(*window.ResetsAt))
		c.set(x, area.y, reset, fgStyle(color))
		x += len([]rune(reset)) + 3
	}
	remaining := max(100-window.UsedPercent, 0)
	used := fmt.Sprintf("used %.0f%%", window.UsedPercent)
	c.set(x, area.y, used, fgStyle(color))
	x += len(used) + 3
	remainingText := fmt.Sprintf("remaining %.0f%%", remaining)
	c.set(x, area.y, remainingText, fgStyle(color))
	x += len(remainingText)
	if pace != nil {
		text, paceColor := paceText(theme, pace)
		x += 3
		c.set(x, area.y, truncate(text, max(area.right()-x, 0)), fgStyle(paceColor))
	}
}

func paceText(theme tuiTheme, pace *quota.Pace) (string, lipgloss.Color) {
	switch pace.Kind {
	case "ahead":
		text := fmt.Sprintf("+%.0f%% pace", pace.Deficit)
		if pace.EtaMs != nil {
			text += " | eta " + formatResetDuration(time.Duration(*pace.EtaMs)*time.Millisecond)
		}
		return text, theme.gaugeHigh
	case "behind":
		return fmt.Sprintf("%.0f%% under pace", -pace.Deficit), theme.gaugeLow
	default:
		return "On track", theme.gaugeMid
	}
}

func windowTags(windows []provider.RateWindow) []string {
	const fiveHours = int64(5 * 60 * 60 * 1000)
	const sevenDays = int64(7 * 24 * 60 * 60 * 1000)
	type parts struct {
		period string
		family string
	}
	raw := make([]parts, len(windows))
	maxPeriod := 0
	for index, window := range windows {
		period := ""
		switch {
		case window.PeriodMs != nil && *window.PeriodMs == fiveHours:
			period = "5H"
		case window.PeriodMs != nil && *window.PeriodMs == sevenDays:
			period = "WEEKLY"
		case window.PeriodMs != nil && *window.PeriodMs > 0:
			period = compactDuration(*window.PeriodMs)
		default:
			period = strings.ToUpper(labelWithoutParenthetical(window.Label))
		}
		family := strings.ToUpper(window.Family)
		if family == period {
			family = ""
		}
		raw[index] = parts{period: period, family: family}
		if family != "" && len([]rune(period)) > maxPeriod {
			maxPeriod = len([]rune(period))
		}
	}
	out := make([]string, len(raw))
	for index, part := range raw {
		if part.family == "" {
			out[index] = part.period
			continue
		}
		out[index] = fmt.Sprintf("%*s · %s", -maxPeriod, part.period, part.family)
	}
	return out
}

func labelWithoutParenthetical(label string) string {
	trimmed := strings.TrimSpace(label)
	if index := strings.LastIndex(trimmed, " ("); index > 0 && strings.HasSuffix(trimmed, ")") {
		return strings.TrimSpace(trimmed[:index])
	}
	return trimmed
}

func creditText(usedLabel, balanceLabel string, credits *provider.CreditInfo, displayMode config.QuotaDisplayMode) string {
	if credits == nil {
		return ""
	}
	if displayMode == config.QuotaRemaining {
		if credits.Limit == nil {
			return balanceLabel + " " + formatCreditAmount(credits.Currency, credits.Used)
		}
		remaining := max(*credits.Limit-credits.Used, 0)
		percent := 0.0
		if *credits.Limit > 0 {
			percent = clampFloat(remaining / *credits.Limit * 100, 0, 100)
		}
		return fmt.Sprintf("%s %s / %s (%.0f%%)", balanceLabel,
			formatCreditAmount(credits.Currency, remaining),
			formatCreditAmount(credits.Currency, *credits.Limit), percent)
	}
	if credits.Limit == nil {
		return usedLabel + " " + formatCreditAmount(credits.Currency, credits.Used)
	}
	percent := 0.0
	if *credits.Limit > 0 {
		percent = clampFloat(credits.Used / *credits.Limit * 100, 0, 999)
	}
	return fmt.Sprintf("%s %s / %s (%.0f%%)", usedLabel,
		formatCreditAmount(credits.Currency, credits.Used),
		formatCreditAmount(credits.Currency, *credits.Limit), percent)
}

func formatCreditAmount(currency string, value float64) string {
	switch currency {
	case "USD":
		return money.Cost(value)
	case "":
		return fmt.Sprintf("%.2f", value)
	default:
		return fmt.Sprintf("%s%.2f", currency, value)
	}
}

func (m *model) drawResetCredits(c *canvas, area rect, snapshot provider.QuotaSnapshot) {
	theme := m.theme()
	if area.h < 2 || len(snapshot.RateLimitResetCredits) == 0 {
		return
	}
	credits := append([]provider.RateLimitResetCredit(nil), snapshot.RateLimitResetCredits...)
	sort.SliceStable(credits, func(i, j int) bool {
		left, right := credits[i], credits[j]
		if left.ExpiresAt == nil {
			return false
		}
		if right.ExpiresAt == nil {
			return true
		}
		return left.ExpiresAt.Before(*right.ExpiresAt)
	})
	c.set(area.x, area.y, "Banked reset  Expiration", boldStyle(theme.fg))
	maxRows := area.h - 1
	if len(credits) <= maxRows {
		for index, credit := range credits {
			c.set(area.x, area.y+1+index, resetCreditLine(index, credit), fgStyle(theme.dim))
		}
		return
	}
	if maxRows == 1 {
		c.set(area.x, area.y+1, resetCreditLine(0, credits[0]), fgStyle(theme.dim))
		return
	}
	visible := maxRows - 1
	for index := 0; index < visible; index++ {
		c.set(area.x, area.y+1+index, resetCreditLine(index, credits[index]), fgStyle(theme.dim))
	}
	c.set(area.x, area.y+1+visible, fmt.Sprintf("+%d more reset credits", len(credits)-visible), fgStyle(theme.dim))
}

func resetCreditLine(index int, credit provider.RateLimitResetCredit) string {
	expiry := "expiry unknown"
	if credit.ExpiresAt != nil {
		expiry = credit.ExpiresAt.Local().Format("2006-01-02 15:04")
	}
	return fmt.Sprintf("#%d %s", index+1, expiry)
}

func displayPlanName(plan string) string {
	runes := []rune(strings.TrimSpace(plan))
	if len(runes) == 0 {
		return ""
	}
	return strings.ToUpper(string(runes[0])) + string(runes[1:])
}

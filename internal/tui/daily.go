package tui

import (
	"time"

	"github.com/charmbracelet/lipgloss"
)

func (m *model) drawDaily(c *canvas, area rect) {
	theme := m.theme()
	barHeight := m.drawRangeBar(c, area)
	area = rect{x: area.x, y: area.y + barHeight, w: area.w, h: area.h - barHeight}
	if area.h < 2 {
		return
	}
	c.box(area.x, area.y, area.w, area.h, " Daily Breakdown ", boldStyle(theme.accent), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 || inner.w <= 0 {
		return
	}
	if m.reportErr != "" {
		c.set(inner.x, inner.y, truncate(m.reportErr, inner.w), fgStyle(theme.gaugeHigh))
		return
	}
	days := m.sortedDays()
	if len(days) == 0 {
		if m.reportLoading {
			c.set(inner.x, inner.y, "Loading…", fgStyle(theme.dim))
		} else {
			c.set(inner.x, inner.y, "No daily data", fgStyle(theme.dim))
		}
		return
	}

	today := dateString(startOfToday())
	showWow := inner.w >= 100
	showDetailCols := inner.w >= 80
	cacheSplitWide := inner.w >= 90
	cacheWidth := 7
	cacheRHeader, cacheWHeader := "CR", "CW"
	if cacheSplitWide {
		cacheWidth = 10
		cacheRHeader, cacheWHeader = "Cache R", "Cache W"
	}
	cacheColor := rgb(251, 146, 60)

	values := m.dailyValueMap()
	dateWidth := 15
	wowWidth := 11
	x := inner.x
	c.set(x, inner.y, padRight("Date"+sortIndicator(m.sortField == sortDate, m.sortAsc), dateWidth), boldStyle(theme.accentSoft))
	x += dateWidth + 1
	c.set(x, inner.y, padLeft("Tokens"+sortIndicator(m.sortField == sortTokens, m.sortAsc), 10), boldStyle(theme.codex))
	x += 11
	c.set(x, inner.y, padLeft("Cost"+sortIndicator(m.sortField == sortCost, m.sortAsc), 10), boldStyle(theme.gaugeMid))
	x += 11
	if showDetailCols {
		c.set(x, inner.y, padLeft("Input", 10), boldStyle(theme.gemini))
		x += 11
		c.set(x, inner.y, padLeft("Output", 10), boldStyle(rgb(167, 139, 250)))
		x += 11
		c.set(x, inner.y, padLeft(cacheRHeader, cacheWidth), boldStyle(cacheColor))
		x += cacheWidth + 1
		c.set(x, inner.y, padLeft(cacheWHeader, cacheWidth), boldStyle(cacheColor))
		x += cacheWidth + 1
	}
	c.set(x, inner.y, padLeft("Msgs", 8), boldStyle(theme.gemini))
	x += 9
	if showWow {
		header := "Token vs 7d"
		if m.dailyMetric == metricCost {
			header = "Cost vs 7d"
		}
		c.set(x, inner.y, padLeft(header, wowWidth), boldStyle(theme.dim))
	}

	visible := max(inner.h-1, 0)
	selected := clampInt(m.dailySel, 0, len(days)-1)
	offset := normalizeScroll(m.dailyOff, selected, visible, len(days))
	m.dailyOff = offset

	for index := 0; index < visible; index++ {
		rowIndex := offset + index
		if rowIndex >= len(days) {
			break
		}
		day := days[rowIndex]
		y := inner.y + 1 + index
		selectedRow := rowIndex == selected
		rowBg := lipgloss.Color("")
		isToday := day.Date == today
		if isToday {
			rowBg = theme.todayBg
		}
		if selectedRow {
			rowBg = theme.selectedBg
		}
		col := func(color lipgloss.Color, bold bool) textStyle {
			return textStyle{fg: color, bg: rowBg, bold: bold}
		}
		dateText := "  " + day.Date
		if m.sortField == sortDate {
			if parsed, err := parseDate(day.Date); err == nil && parsed.Weekday() == time.Monday {
				dateText = "┄ " + day.Date
			}
		}
		x = inner.x
		c.set(x, y, padRight(dateText, dateWidth), col(theme.accentSoft, isToday))
		x += dateWidth + 1
		c.set(x, y, padLeft(formatCompact(day.Tokens), 10), col(theme.codex, false))
		x += 11
		c.set(x, y, padLeft(formatCost(day.Cost), 10), col(theme.gaugeMid, false))
		x += 11
		if showDetailCols {
			c.set(x, y, padLeft(formatCompact(day.Input), 10), col(theme.gemini, false))
			x += 11
			c.set(x, y, padLeft(formatCompact(day.Output), 10), col(rgb(167, 139, 250), false))
			x += 11
			c.set(x, y, padLeft(formatCompact(day.CacheRead), cacheWidth), col(cacheColor, false))
			x += cacheWidth + 1
			c.set(x, y, padLeft(formatCompact(day.CacheWrite), cacheWidth), col(cacheColor, false))
			x += cacheWidth + 1
		}
		c.set(x, y, padLeft(formatCompact(day.Messages), 8), col(theme.gemini, false))
		x += 9
		if showWow {
			priorDate := ""
			if parsed, err := parseDate(day.Date); err == nil {
				priorDate = dateString(parsed.AddDate(0, 0, -7))
			}
			var prior *float64
			if value, ok := values[priorDate]; ok {
				prior = &value
			}
			current := float64(day.Tokens)
			if m.dailyMetric == metricCost {
				current = day.Cost
			}
			text := percentDeltaText(current, prior)
			c.set(x, y, padLeft(text, wowWidth), col(percentDeltaColor(theme, current, prior), false))
		}
	}
}

// dailyValueMap combines the range report with the earlier-dates report so a
// row can compare against a date before the range start.
func (m *model) dailyValueMap() map[string]float64 {
	values := map[string]float64{}
	for _, day := range m.prior.Days {
		value := float64(day.Tokens)
		if m.dailyMetric == metricCost {
			value = day.Cost
		}
		values[day.Date] = value
	}
	for _, day := range m.report.Days {
		value := float64(day.Tokens)
		if m.dailyMetric == metricCost {
			value = day.Cost
		}
		values[day.Date] = value
	}
	return values
}

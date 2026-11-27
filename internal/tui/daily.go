package tui

import (
	"fmt"
	"sort"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/report"
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
	values := m.dailyValueMap()
	dateWidth := 15
	wowWidth := 11
	headerStyle := func(active bool) textStyle {
		if active {
			return boldStyle(theme.accent)
		}
		return boldStyle(theme.dim)
	}
	x := inner.x
	c.setClip(x, inner.y, inner.right(), padRight("Date"+sortIndicator(m.sortField == sortDate, m.sortAsc), dateWidth), headerStyle(m.sortField == sortDate))
	x += dateWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft("Tokens"+sortIndicator(m.sortField == sortTokens, m.sortAsc), 10), headerStyle(m.sortField == sortTokens))
	x += 11
	c.setClip(x, inner.y, inner.right(), padLeft("Cost"+sortIndicator(m.sortField == sortCost, m.sortAsc), 10), headerStyle(m.sortField == sortCost))
	x += 11
	if showDetailCols {
		c.setClip(x, inner.y, inner.right(), padLeft("Input", 10), boldStyle(theme.dim))
		x += 11
		c.setClip(x, inner.y, inner.right(), padLeft("Output", 10), boldStyle(theme.dim))
		x += 11
		c.setClip(x, inner.y, inner.right(), padLeft(cacheRHeader, cacheWidth), boldStyle(theme.dim))
		x += cacheWidth + 1
		c.setClip(x, inner.y, inner.right(), padLeft(cacheWHeader, cacheWidth), boldStyle(theme.dim))
		x += cacheWidth + 1
	}
	c.setClip(x, inner.y, inner.right(), padLeft("Msgs", 8), boldStyle(theme.dim))
	x += 9
	if showWow {
		header := "Token vs 7d"
		if m.dailyMetric == metricCost {
			header = "Cost vs 7d"
		}
		c.setClip(x, inner.y, inner.right(), padLeft(header, wowWidth), boldStyle(theme.dim))
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
		if rowBg != "" {
			c.fill(inner.x, y, inner.w, 1, textStyle{bg: rowBg})
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
		c.setClip(x, y, inner.right(), padRight(dateText, dateWidth), col(theme.fg, isToday || selectedRow))
		x += dateWidth + 1
		c.setClip(x, y, inner.right(), padLeft(formatCompact(day.Tokens), 10), col(theme.fg, selectedRow || m.sortField == sortTokens))
		x += 11
		c.setClip(x, y, inner.right(), padLeft(formatCost(day.Cost), 10), col(theme.costColor, selectedRow || m.sortField == sortCost))
		x += 11
		if showDetailCols {
			c.setClip(x, y, inner.right(), padLeft(formatCompact(day.Input), 10), col(theme.fg, selectedRow))
			x += 11
			c.setClip(x, y, inner.right(), padLeft(formatCompact(day.Output), 10), col(theme.fg, selectedRow))
			x += 11
			c.setClip(x, y, inner.right(), padLeft(formatCompact(day.CacheRead), cacheWidth), col(theme.fg, selectedRow))
			x += cacheWidth + 1
			c.setClip(x, y, inner.right(), padLeft(formatCompact(day.CacheWrite), cacheWidth), col(theme.fg, selectedRow))
			x += cacheWidth + 1
		}
		c.setClip(x, y, inner.right(), padLeft(formatCompact(day.Messages), 8), col(theme.dim, selectedRow))
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
			c.setClip(x, y, inner.right(), padLeft(text, wowWidth), col(percentDeltaColor(theme, current, prior), false))
		}
	}
}

func (m *model) drawDayDetail(c *canvas, area rect) {
	theme := m.theme()
	detail := m.dayDetail
	box := popup(area, min(106, area.w-4), min(26, area.h-2))
	c.box(box.x, box.y, box.w, box.h, " Day "+detail.Date+" ", boldStyle(theme.companyColor("other")), fgStyle(theme.border))
	inner := box.inner()
	var lines []string
	cacheWriteTotal := "—"
	if detail.Totals.CacheWrite > 0 {
		cacheWriteTotal = formatCompact(detail.Totals.CacheWrite)
	}
	lines = append(lines,
		fmt.Sprintf("Cost %s   Tokens %s   Messages %s   Sessions %s",
			formatCost(detail.Totals.Cost), formatCompact(detail.Totals.Tokens),
			formatIntCommas(detail.Totals.Messages), formatIntCommas(detail.Totals.Sessions)),
		fmt.Sprintf("Input %s   Output %s   Cache read %s   Cache write %s",
			formatCompact(detail.Totals.Input), formatCompact(detail.Totals.Output),
			formatCompact(detail.Totals.CacheRead), cacheWriteTotal),
		"",
	)
	for _, agent := range detail.Agents {
		lines = append(lines, fmt.Sprintf("%s  %s", displayAgentName(agent.Agent), formatCost(agent.Totals.Cost)))
		for _, model := range agent.Models {
			cw := "—"
			if model.CacheWrite > 0 {
				cw = formatCompact(model.CacheWrite)
			}
			lines = append(lines, fmt.Sprintf("  %-30s %10s  %9s  CR %9s  CW %9s  Hit %s",
				truncate(model.Model, 30), formatCost(model.Cost), formatCompact(model.Tokens),
				formatCompact(model.CacheRead), cw, cacheHitPercent(model.CacheRead, model.Tokens)))
		}
	}
	lines = append(lines, "", "Esc closes")
	start := clampInt(m.detailScroll, 0, max(len(lines)-inner.h, 0))
	m.detailScroll = start
	for index := 0; index < inner.h && start+index < len(lines); index++ {
		c.set(inner.x, inner.y+index, truncate(lines[start+index], inner.w), fgStyle(theme.fg))
	}
}

func (m *model) handleDailyKey(key string) (tea.Model, tea.Cmd) {
	switch key {
	case "c", "t", "d":
		toggleSort(key, &m.sortField, &m.sortAsc)
		m.dailySel, m.dailyOff = 0, 0
	case "v":
		if m.dailyMetric == metricTokens {
			m.dailyMetric = metricCost
		} else {
			m.dailyMetric = metricTokens
		}
	case "down", "j":
		if m.dailySel < len(m.report.Days)-1 {
			m.dailySel++
		}
	case "up", "k":
		if m.dailySel > 0 {
			m.dailySel--
		}
	case "enter":
		days := m.sortedDays()
		if len(days) == 0 {
			return m, nil
		}
		date := days[clampInt(m.dailySel, 0, len(days)-1)].Date
		return m, m.loadDayDetailCmd(date)
	}
	return m, nil
}

func (m *model) loadDayDetailCmd(date string) tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	agents := m.selectedAgents()
	key := m.dayDetailKey(date)
	m.detailSeq++
	seq := m.detailSeq
	return func() tea.Msg {
		value, err := client.UsageDay(date, agents)
		if err != nil {
			return toastMsg{"Day: " + err.Error()}
		}
		return dayDetailMsg{key: key, date: date, seq: seq, value: value}
	}
}

func (m *model) sortedDays() []report.DaySummary {
	days := append([]report.DaySummary(nil), m.report.Days...)
	sortDays(days, m.sortField, m.sortAsc)
	return days
}

func sortDays(days []report.DaySummary, field int, ascending bool) {
	if len(days) < 2 {
		return
	}
	sort.SliceStable(days, func(i, j int) bool {
		left, right := days[i], days[j]
		var less, equal bool
		switch field {
		case sortCost:
			less = left.Cost < right.Cost
			equal = left.Cost == right.Cost
		case sortTokens:
			less = left.Tokens < right.Tokens
			equal = left.Tokens == right.Tokens
		default:
			less = left.Date < right.Date
			equal = left.Date == right.Date
		}
		if equal {
			return left.Date < right.Date
		}
		if ascending {
			return less
		}
		return !less && !equal
	})
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

package tui

import (
	"fmt"
	"sort"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/report"
)

func (m *model) drawActivity(c *canvas, area rect) {
	if area.h <= 0 || area.w <= 0 {
		return
	}
	barHeight := m.drawRangeBar(c, area)
	body := rect{x: area.x, y: area.y + barHeight, w: area.w, h: area.h - barHeight}
	if body.h <= 0 {
		return
	}
	heatHeight := min(12, max(body.h/2, 9))
	if heatHeight > body.h {
		heatHeight = body.h
	}
	heat := rect{x: body.x, y: body.y, w: body.w, h: heatHeight}
	info := rect{x: body.x, y: body.y + heatHeight, w: body.w, h: body.h - heatHeight}
	m.drawHeatmap(c, heat)
	if info.h <= 0 {
		return
	}
	if info.w >= 100 && info.h >= 10 {
		panels := splitHFixed(info, 40, max(info.w-40, 0))
		m.drawActivitySummary(c, panels[0])
		m.drawActivityDetail(c, panels[1])
	} else {
		summaryHeight := min(15, info.h)
		m.drawActivitySummary(c, rect{x: info.x, y: info.y, w: info.w, h: summaryHeight})
		if info.h > summaryHeight {
			m.drawActivityDetail(c, rect{x: info.x, y: info.y + summaryHeight, w: info.w, h: info.h - summaryHeight})
		}
	}
}

func (m *model) drawHeatmap(c *canvas, area rect) {
	theme := m.theme()
	c.box(area.x, area.y, area.w, area.h, " Usage Activity - Past 365 Days ", boldStyle(theme.accent), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 || inner.w <= 0 {
		return
	}
	fromText, toText, ok := m.activityWindow()
	if m.activityLoaded {
		fromText, toText = m.activity.Range.From, m.activity.Range.To
	} else if !ok {
		fromText, toText = m.report.Range.From, m.report.Range.To
	}
	from, err1 := parseDate(fromText)
	to, err2 := parseDate(toText)
	if err1 != nil || err2 != nil {
		c.set(inner.x, inner.y, "No activity data", fgStyle(theme.dim))
		return
	}
	if m.activityErr != "" {
		c.set(inner.x, inner.y, truncate(m.activityErr, inner.w), fgStyle(theme.gaugeHigh))
		return
	}

	values := map[string]float64{}
	for _, day := range m.activity.Days {
		if m.activityMetric == metricCost {
			values[day.Date] = day.Cost
		} else {
			values[day.Date] = float64(day.Tokens)
		}
	}
	var maxValue float64
	for _, value := range values {
		if value > maxValue {
			maxValue = value
		}
	}
	thresholds := [4]float64{maxValue * 0.2, maxValue * 0.4, maxValue * 0.6, maxValue * 0.8}
	palette := theme.heatmapPalette(m.activityMetric)

	gridX := inner.x + 5
	gridY := inner.y
	gridWidth := inner.w - 5
	if inner.h >= 9 {
		gridY = inner.y + 1
	}
	if gridWidth < 2 || inner.h < 7 {
		if !m.activityLoading {
			c.set(inner.x, inner.y, "No activity data", fgStyle(theme.dim))
		}
		return
	}
	start := mustMonday(from)
	totalDays := int(to.Sub(start).Hours()/24) + 1
	totalWeeks := max((totalDays+6)/7, 1)
	displayCols := max(min(gridWidth/2, totalWeeks), 1)
	firstWeek := max(totalWeeks-displayCols, 0)

	weekdays := []string{"Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"}
	for row, label := range weekdays {
		c.set(inner.x, gridY+row, label, fgStyle(theme.dim))
	}

	// Month labels: first day of each month inside the visible window. A label
	// that lands on the previous label is shifted right, as 0.6.16 does.
	type monthLabel struct {
		col  int
		text string
	}
	var labels []monthLabel
	lastMonth := -1
	visibleStart := start.AddDate(0, 0, firstWeek*7)
	for day := visibleStart; inner.h >= 9 && !day.After(to); day = day.AddDate(0, 0, 1) {
		if day.Before(from) || lastMonth == int(day.Month()) {
			continue
		}
		labels = append(labels, monthLabel{
			col:  int(day.Sub(visibleStart).Hours()/24) / 7,
			text: day.Format("Jan"),
		})
		lastMonth = int(day.Month())
	}
	lastEnd := gridX - 2
	for _, label := range labels {
		x := gridX + label.col*2
		if x <= lastEnd {
			x = lastEnd + 1
		}
		if x+len(label.text) > gridX+displayCols*2 {
			continue
		}
		c.set(x, inner.y, label.text, fgStyle(theme.dim))
		lastEnd = x + len(label.text)
	}

	for week := 0; week < displayCols; week++ {
		for row := 0; row < 7; row++ {
			date := start.AddDate(0, 0, (firstWeek+week)*7+row)
			x := gridX + week*2
			y := gridY + row
			if date.Before(from) || date.After(to) {
				c.set(x, y, "  ", fgStyle(theme.emptyHeatmap))
				continue
			}
			value := values[dateString(date)]
			color := emptyHeatColor(value, maxValue, thresholds, palette, theme)
			if m.activitySel == dateString(date) {
				c.set(x, y, " ◆", textStyle{fg: readableForeground(color), bg: color, bold: true})
			} else {
				c.set(x, y, "██", textStyle{fg: color, bg: theme.heatmapBg})
			}
		}
	}

	footerY := inner.y + inner.h - 1
	if footerY > gridY+7 {
		x := inner.x
		text := fmt.Sprintf("Date Range  %s → %s    ", fromText, toText)
		c.set(x, footerY, text, fgStyle(theme.dim))
		x += len([]rune(text))
		c.set(x, footerY, "low ", fgStyle(theme.dim))
		x += 4
		for _, color := range palette {
			c.set(x, footerY, "██", textStyle{fg: color, bg: theme.heatmapBg})
			x += 2
		}
		c.set(x, footerY, " high", fgStyle(theme.dim))
	}
}

func mustMonday(day time.Time) time.Time {
	return mondayOf(day)
}

func emptyHeatColor(value, maxValue float64, thresholds [4]float64, palette [5]lipgloss.Color, theme tuiTheme) lipgloss.Color {
	if value <= 0 || maxValue <= 0 {
		return theme.emptyHeatmap
	}
	switch {
	case value < thresholds[0]:
		return palette[0]
	case value < thresholds[1]:
		return palette[1]
	case value < thresholds[2]:
		return palette[2]
	case value < thresholds[3]:
		return palette[3]
	default:
		return palette[4]
	}
}

func readableForeground(background lipgloss.Color) lipgloss.Color {
	// Heatmap cells are saturated; dark text keeps the selected diamond legible.
	return rgb(15, 23, 42)
}

func (m *model) drawActivitySummary(c *canvas, area rect) {
	theme := m.theme()
	c.box(area.x, area.y, area.w, area.h, " Range Overview ", boldStyle(theme.accentSoft), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 {
		return
	}
	days := m.activitySummaryDays()
	active := 0
	for _, day := range days {
		if day.Tokens > 0 {
			active++
		}
	}
	current, longest := streakValues(days, dateString(startOfToday()))
	lines := []struct {
		label string
		value string
	}{
		{"Active Days", fmt.Sprintf("%d days", active)},
		{"Streak", fmt.Sprintf("%d/%d cur/best", current, longest)},
	}
	y := inner.y
	for _, line := range lines {
		c.set(inner.x, y, padRight(line.label, 12), fgStyle(theme.dim))
		c.set(inner.x+12, y, line.value, boldStyle(theme.fg))
		y++
	}
	if y < inner.y+inner.h {
		y++
	}
	if y < inner.y+inner.h {
		c.set(inner.x, y, padRight("Period", 12), fgStyle(theme.dim))
		c.set(inner.x+13, y, padRight("Tokens", 10), boldStyle(theme.codex))
		c.set(inner.x+24, y, "Cost", boldStyle(theme.gaugeMid))
		y++
	}
	rows := m.activityPeriodRows()
	rolling := map[string]bool{"Last 7 days": true, "Last 30 days": true}
	order := []string{"Today", "Week", "Month", "Year", "Last 7 days", "Last 30 days"}
	if y+len(order) > inner.y+inner.h {
		order = []string{"Today", "Week", "Month", "Year", "Last 7 days"}
	}
	if y+len(order) > inner.y+inner.h {
		order = []string{"Today", "Week", "Month", "Year"}
	}
	for _, label := range order {
		if rolling[label] && y+1 > inner.y+inner.h-1 {
			continue
		}
		if y >= inner.y+inner.h {
			break
		}
		row := rows[label]
		c.set(inner.x, y, padRight(label, 12), fgStyle(theme.dim))
		c.set(inner.x+13, y, padLeft(formatCompact(row.tokens), 10), fgStyle(theme.codex))
		c.set(inner.x+24, y, formatCost(row.cost), fgStyle(theme.gaugeMid))
		y++
	}
}

type periodRow struct {
	tokens int64
	cost   float64
}

func (m *model) activitySummaryDays() []report.DaySummary { return m.activity.Days }

func (m *model) activityPeriodRows() map[string]periodRow {
	today := startOfToday()
	monday := mondayOf(today)
	rows := map[string]periodRow{}
	add := func(label string, match func(day time.Time) bool) {
		var row periodRow
		for _, day := range m.activity.Days {
			parsed, err := parseDate(day.Date)
			if err != nil {
				continue
			}
			if match(parsed) {
				row.tokens += day.Tokens
				row.cost += day.Cost
			}
		}
		rows[label] = row
	}
	add("Today", func(day time.Time) bool { return day.Equal(today) })
	add("Week", func(day time.Time) bool { return !day.Before(monday) && !day.After(today) })
	add("Month", func(day time.Time) bool {
		return day.Year() == today.Year() && day.Month() == today.Month() && !day.After(today)
	})
	add("Year", func(day time.Time) bool { return day.Year() == today.Year() && !day.After(today) })
	add("Last 7 days", func(day time.Time) bool { return !day.Before(today.AddDate(0, 0, -6)) && !day.After(today) })
	add("Last 30 days", func(day time.Time) bool { return !day.Before(today.AddDate(0, 0, -29)) && !day.After(today) })
	return rows
}

func streakValues(days []report.DaySummary, today string) (int, int) {
	values := map[string]bool{}
	var dates []string
	for _, day := range days {
		if day.Tokens > 0 {
			values[day.Date] = true
			dates = append(dates, day.Date)
		}
	}
	if len(dates) == 0 {
		return 0, 0
	}
	sort.Strings(dates)
	best, run := 0, 0
	previous := ""
	for _, date := range dates {
		if previous != "" {
			if prev, err := parseDate(previous); err == nil && dateString(prev.AddDate(0, 0, 1)) == date {
				run++
			} else {
				run = 1
			}
		} else {
			run = 1
		}
		if run > best {
			best = run
		}
		previous = date
	}
	current := 0
	if cursor, err := parseDate(today); err == nil {
		for values[dateString(cursor)] {
			current++
			cursor = cursor.AddDate(0, 0, -1)
		}
	}
	return current, best
}

func (m *model) drawActivityDetail(c *canvas, area rect) {
	theme := m.theme()
	c.box(area.x, area.y, area.w, area.h, " Selected Day ", boldStyle(theme.companyColor("other")), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 || inner.w <= 0 {
		return
	}
	if m.activitySel == "" {
		c.set(inner.x, inner.y, "No selected day", fgStyle(theme.dim))
		return
	}
	var summary report.DaySummary
	for _, day := range m.activity.Days {
		if day.Date == m.activitySel {
			summary = day
			break
		}
	}
	totals := summary.Totals
	if m.activityDetail != nil && m.activityDetail.Date == m.activitySel {
		totals = m.activityDetail.Totals
	}
	panels := []rect{inner}
	if inner.w >= 76 && inner.h >= 5 {
		panels = splitHFixed(inner, 34, max(inner.w-34, 0))
	} else if inner.h >= 6 {
		panels = []rect{
			{x: inner.x, y: inner.y, w: inner.w, h: 2},
			{x: inner.x, y: inner.y + 2, w: inner.w, h: inner.h - 2},
		}
	}
	left := panels[0]
	overview := []string{
		m.activitySel + "  " + activityMetricText(m.activityMetric, totals),
		fmt.Sprintf("Cost %8s  Tokens %8s", formatCost(totals.Cost), formatCompact(totals.Tokens)),
		fmt.Sprintf("In   %8s  Out    %8s", formatCompact(totals.Input), formatCompact(totals.Output)),
		fmt.Sprintf("CR   %8s  CW     %8s", formatCompact(totals.CacheRead), formatCompact(totals.CacheWrite)),
		fmt.Sprintf("Msgs %8s  Sess   %8s", formatIntCommas(totals.Messages), formatIntCommas(totals.Sessions)),
	}
	for index, line := range overview {
		if index >= left.h {
			break
		}
		c.set(left.x, left.y+index, truncate(line, left.w), fgStyle(theme.fg))
	}
	if len(panels) < 2 {
		return
	}
	right := panels[1]
	if right.h <= 0 {
		return
	}
	var lines []string
	lines = append(lines, "Agent / Model Cost")
	if m.activityDetailError != "" {
		lines = append(lines, m.activityDetailError)
	} else if m.activityDetail == nil || m.activityDetail.Date != m.activitySel {
		lines = append(lines, "Loading…")
	} else {
		for _, agent := range m.activityDetail.Agents {
			lines = append(lines, fmt.Sprintf("%s   %s", displayAgentName(agent.Agent), formatCost(agent.Totals.Cost)))
			for _, model := range agent.Models {
				lines = append(lines, fmt.Sprintf("  %-30s %9s  T %8s I %8s O %8s CR %8s CW %8s",
					truncate(model.Model, 30), formatCost(model.Cost), formatCompact(model.Tokens),
					formatCompact(model.Input), formatCompact(model.Output),
					formatCompact(model.CacheRead), formatCompact(model.CacheWrite)))
			}
		}
	}
	start := clampInt(m.activityDetailOff, 0, max(len(lines)-right.h, 0))
	for index := 0; index < right.h && start+index < len(lines); index++ {
		line := lines[start+index]
		color := theme.fg
		if index == 0 && start == 0 {
			color = theme.accentSoft
		}
		c.set(right.x, right.y+index, truncate(line, right.w), fgStyle(color))
	}
}

func activityMetricText(metric int, totals report.Totals) string {
	if metric == metricCost {
		return formatCost(totals.Cost)
	}
	return formatCompact(totals.Tokens)
}

func (m *model) handleActivityKey(key string) (tea.Model, tea.Cmd) {
	switch key {
	case "t":
		m.activityMetric = metricTokens
	case "c":
		m.activityMetric = metricCost
	case "up", "k":
		m.moveActivitySelection(-1)
		return m, m.ensureActivityDetail()
	case "down", "j":
		m.moveActivitySelection(1)
		return m, m.ensureActivityDetail()
	case "n":
		m.activitySel = dateString(startOfToday())
		return m, m.ensureActivityDetail()
	case "[":
		if m.activityDetailOff > 0 {
			m.activityDetailOff--
		}
	case "]":
		m.activityDetailOff++
	}
	return m, nil
}

func (m *model) moveActivitySelection(direction int) {
	days := m.activity.Days
	if len(days) == 0 {
		return
	}
	index := -1
	for i, day := range days {
		if day.Date == m.activitySel {
			index = i
			break
		}
	}
	if index < 0 {
		index = 0
	}
	index = clampInt(index+direction, 0, len(days)-1)
	m.activitySel = days[index].Date
	m.activityDetailOff = 0
}

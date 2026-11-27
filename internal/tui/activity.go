package tui

import (
	"fmt"
	"math"
	"sort"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/report"
)

const heatmapLegendBlocks = 5

type styledSpan struct {
	text  string
	style textStyle
}

func drawSpans(c *canvas, x, y, maxRight int, spans []styledSpan) {
	cur := x
	for _, s := range spans {
		if cur >= maxRight {
			return
		}
		c.setClip(cur, y, maxRight, s.text, s.style)
		cur += len([]rune(s.text))
	}
}

func (m *model) drawActivity(c *canvas, area rect) {
	if area.h <= 0 || area.w <= 0 {
		return
	}
	barHeight := m.drawRangeBar(c, area)
	body := rect{x: area.x, y: area.y + barHeight, w: area.w, h: area.h - barHeight}
	if body.h <= 0 {
		return
	}
	heat := heatmapCardArea(body)
	info := rect{x: body.x, y: body.y + heat.h, w: body.w, h: body.h - heat.h}
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

func (m *model) heatmapBounds() (time.Time, time.Time, string, string, bool) {
	fromText, toText, ok := m.activityWindow()
	if m.activityLoaded {
		fromText, toText = m.activity.Range.From, m.activity.Range.To
	} else if !ok {
		fromText, toText = m.report.Range.From, m.report.Range.To
	}
	from, err1 := parseDate(fromText)
	to, err2 := parseDate(toText)
	if err1 != nil || err2 != nil {
		return time.Time{}, time.Time{}, "", "", false
	}
	return from, to, fromText, toText, true
}

func heatmapCardArea(body rect) rect {
	height := min(12, max(body.h/2, 9))
	if height > body.h {
		height = body.h
	}
	return rect{x: body.x, y: body.y, w: body.w, h: height}
}

func activityHeatArea(bodyArea rect) rect {
	return heatmapCardArea(rect{x: bodyArea.x, y: bodyArea.y + 1, w: bodyArea.w, h: bodyArea.h - 1})
}

func (m *model) heatmapDateAt(bodyArea rect, col, row int) (string, bool) {
	if bodyArea.h <= 1 || bodyArea.w <= 0 {
		return "", false
	}
	inner := activityHeatArea(bodyArea).inner()
	if inner.h < 7 || inner.w < 7 {
		return "", false
	}
	from, to, _, _, ok := m.heatmapBounds()
	if !ok {
		return "", false
	}
	gridX := inner.x + 5
	gridY := inner.y
	if inner.h >= 9 {
		gridY = inner.y + 1
	}
	gridWidth := inner.w - 5
	start := mondayOf(from)
	totalDays := int(to.Sub(start).Hours()/24) + 1
	totalWeeks := max((totalDays+6)/7, 1)
	displayCols := max(min(gridWidth/2, totalWeeks), 1)
	firstWeek := max(totalWeeks-displayCols, 0)

	if row < gridY || row >= gridY+7 {
		return "", false
	}
	if col < gridX || col >= gridX+displayCols*2 {
		return "", false
	}
	week := (col - gridX) / 2
	weekday := row - gridY
	date := start.AddDate(0, 0, (firstWeek+week)*7+weekday)
	if date.Before(from) || date.After(to) {
		return "", false
	}
	return dateString(date), true
}

func (m *model) drawHeatmap(c *canvas, area rect) {
	theme := m.theme()
	c.box(area.x, area.y, area.w, area.h, " Usage Activity - Past 365 Days ", boldStyle(theme.accent), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 || inner.w <= 0 {
		return
	}
	from, to, fromText, toText, ok := m.heatmapBounds()
	if !ok {
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
	start := mondayOf(from)
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

	if text, legendX, footerY, ok := heatmapFooterLayout(inner, fromText, toText); ok {
		c.setClip(inner.x, footerY, inner.right(), text, fgStyle(theme.dim))
		x := legendX
		for _, color := range palette {
			c.setClip(x, footerY, inner.right(), "██", textStyle{fg: color, bg: theme.heatmapBg})
			x += 2
		}
		c.setClip(x, footerY, inner.right(), " high", fgStyle(theme.dim))
		x += len(" high")
		if m.activityLegend >= 0 && m.activityLegend < heatmapLegendBlocks {
			low, high := activityLegendRange(m.activityLegend, maxValue, thresholds)
			gap := "    "
			if inner.w < 92 {
				gap = "  "
			}
			label := fmt.Sprintf("%sLevel %d: %s - %s", gap, m.activityLegend+1,
				formatHeatmapValue(low, m.activityMetric), formatHeatmapValue(high, m.activityMetric))
			c.setClip(x, footerY, inner.right(), label, boldStyle(theme.accent))
		}
	}
}

func heatmapFooterLayout(inner rect, fromText, toText string) (text string, legendX, y int, ok bool) {
	y = inner.y + inner.h - 1
	gridY := inner.y
	if inner.h >= 9 {
		gridY = inner.y + 1
	}
	if y <= gridY+7 {
		return "", 0, 0, false
	}
	if inner.w < 92 {
		text = fmt.Sprintf("%s → %s  low ", fromText, toText)
	} else {
		text = fmt.Sprintf("Date Range  %s → %s    low ", fromText, toText)
	}
	return text, inner.x + len([]rune(text)), y, true
}

func activityLegendRange(level int, maxValue float64, thresholds [4]float64) (float64, float64) {
	low := 0.0
	if level > 0 {
		low = thresholds[level-1]
	}
	high := maxValue
	if level < len(thresholds) {
		high = thresholds[level]
	}
	return low, high
}

func formatHeatmapValue(value float64, metric int) string {
	if metric == metricCost {
		return formatCost(value)
	}
	return formatCompact(int64(math.Round(value)))
}

func (m *model) heatmapLegendAt(bodyArea rect, col, row int) (int, bool) {
	if bodyArea.h <= 1 || bodyArea.w <= 0 || m.activityErr != "" {
		return 0, false
	}
	inner := activityHeatArea(bodyArea).inner()
	if inner.h < 7 || inner.w < 7 {
		return 0, false
	}
	_, _, fromText, toText, ok := m.heatmapBounds()
	if !ok {
		return 0, false
	}
	_, legendX, footerY, ok := heatmapFooterLayout(inner, fromText, toText)
	if !ok || row != footerY || col < legendX || col >= min(legendX+heatmapLegendBlocks*2, inner.right()) {
		return 0, false
	}
	return (col - legendX) / 2, true
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

type rangeOverviewRow struct {
	label   string
	tokens  int64
	cost    float64
	rolling bool
}

func (m *model) drawActivitySummary(c *canvas, area rect) {
	theme := m.theme()
	c.box(area.x, area.y, area.w, area.h, " Range Overview ", boldStyle(theme.accent), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 {
		return
	}
	days := m.activity.Days
	active := 0
	for _, day := range days {
		if day.Tokens > 0 {
			active++
		}
	}
	current, longest := streakValues(days, dateString(startOfToday()))
	overviewRows := m.activityPeriodRows()

	headerLines := 4 // Active Days, Streak, blank spacer, table header
	useSpacer := true
	if headerLines+len(overviewRows) > inner.h {
		useSpacer = false
		headerLines = 3
	}
	// Drop rolling rows first when the card is short so calendar + Total/Average/Max stay visible.
	for headerLines+len(overviewRows) > inner.h {
		dropped := false
		for i := len(overviewRows) - 1; i >= 0; i-- {
			if overviewRows[i].rolling {
				overviewRows = append(overviewRows[:i], overviewRows[i+1:]...)
				dropped = true
				break
			}
		}
		if !dropped {
			break
		}
	}

	y := inner.y
	drawSpans(c, inner.x, y, inner.right(), []styledSpan{
		{padRight("Active Days", 12), fgStyle(theme.dim)},
		{fmt.Sprintf("%d days", active), boldStyle(theme.fg)},
	})
	y++
	if y < inner.bottom() {
		drawSpans(c, inner.x, y, inner.right(), []styledSpan{
			{padRight("Streak", 12), fgStyle(theme.dim)},
			{fmt.Sprintf("%d/%d", current, longest), boldStyle(theme.fg)},
			{" cur/best", fgStyle(theme.dim)},
		})
		y++
	}
	if useSpacer && y < inner.bottom() {
		y++
	}
	if y < inner.bottom() {
		c.set(inner.x, y, padRight("Period", 12), fgStyle(theme.dim))
		c.set(inner.x+13, y, padLeft("Tokens", 10), boldStyle(theme.dim))
		c.set(inner.x+24, y, "Cost", boldStyle(theme.dim))
		y++
	}
	for _, row := range overviewRows {
		if y >= inner.bottom() {
			break
		}
		c.set(inner.x, y, padRight(row.label, 12), fgStyle(theme.dim))
		c.set(inner.x+13, y, padLeft(formatCompact(row.tokens), 10), fgStyle(theme.fg))
		c.set(inner.x+24, y, formatCost(row.cost), fgStyle(theme.costColor))
		y++
	}
}

func (m *model) activityPeriodRows() []rangeOverviewRow {
	today := startOfToday()
	monday := mondayOf(today)
	last7 := today.AddDate(0, 0, -6)
	last30 := today.AddDate(0, 0, -29)

	type accum struct {
		label   string
		rolling bool
		tokens  int64
		cost    float64
	}
	windows := []accum{
		{label: "Today"},
		{label: "Week"},
		{label: "Last 7 days", rolling: true},
		{label: "Month"},
		{label: "Last 30 days", rolling: true},
		{label: "Year"},
		{label: "Total"},
	}
	var maxTokens int64
	var maxCost float64
	for _, day := range m.activity.Days {
		parsed, err := parseDate(day.Date)
		if err != nil {
			continue
		}
		since := func(start time.Time) bool { return !parsed.Before(start) && !parsed.After(today) }
		hits := [7]bool{
			parsed.Equal(today),
			since(monday),
			since(last7),
			parsed.Year() == today.Year() && parsed.Month() == today.Month() && !parsed.After(today),
			since(last30),
			parsed.Year() == today.Year() && !parsed.After(today),
			true,
		}
		for i, hit := range hits {
			if hit {
				windows[i].tokens += day.Tokens
				windows[i].cost += day.Cost
			}
		}
		if day.Tokens > maxTokens {
			maxTokens = day.Tokens
		}
		if day.Cost > maxCost {
			maxCost = day.Cost
		}
	}

	rows := make([]rangeOverviewRow, 0, len(windows)+2)
	for _, w := range windows {
		rows = append(rows, rangeOverviewRow{label: w.label, tokens: w.tokens, cost: w.cost, rolling: w.rolling})
	}
	total := windows[len(windows)-1]
	dayCount := float64(max(len(m.activity.Days), 1))
	rows = append(rows,
		rangeOverviewRow{label: "Average", tokens: int64(math.Round(float64(total.tokens) / dayCount)), cost: total.cost / dayCount},
		rangeOverviewRow{label: "Max", tokens: maxTokens, cost: maxCost},
	)
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
	c.box(area.x, area.y, area.w, area.h, " Selected Day ", boldStyle(theme.accent), fgStyle(theme.border))
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
	var overview [][]styledSpan
	if left.w >= 34 && left.h >= 5 {
		overview = [][]styledSpan{
			{
				{m.activitySel, boldStyle(theme.fg)},
				{"  ", fgStyle(theme.fg)},
				{activityMetricText(m.activityMetric, totals), boldStyle(theme.fg)},
			},
			{
				{"Cost ", fgStyle(theme.dim)},
				{padLeft(formatCost(totals.Cost), 8), fgStyle(theme.costColor)},
				{"  Tokens ", fgStyle(theme.dim)},
				{padLeft(formatCompact(totals.Tokens), 8), fgStyle(theme.fg)},
			},
			{
				{"In   ", fgStyle(theme.dim)},
				{padLeft(formatCompact(totals.Input), 8), fgStyle(theme.fg)},
				{"  Out    ", fgStyle(theme.dim)},
				{padLeft(formatCompact(totals.Output), 8), fgStyle(theme.fg)},
			},
			{
				{"CR   ", fgStyle(theme.dim)},
				{padLeft(formatCompact(totals.CacheRead), 8), fgStyle(theme.fg)},
				{"  CW     ", fgStyle(theme.dim)},
				{padLeft(formatCompact(totals.CacheWrite), 8), fgStyle(theme.fg)},
			},
			{
				{"Msgs ", fgStyle(theme.dim)},
				{padLeft(formatIntCommas(totals.Messages), 8), fgStyle(theme.fg)},
				{"  Sess   ", fgStyle(theme.dim)},
				{padLeft(formatIntCommas(totals.Sessions), 8), fgStyle(theme.fg)},
			},
		}
	} else {
		overview = [][]styledSpan{
			{
				{m.activitySel, boldStyle(theme.fg)},
				{"  ", fgStyle(theme.fg)},
				{activityMetricText(m.activityMetric, totals), boldStyle(theme.fg)},
				{"  Cost ", fgStyle(theme.dim)},
				{formatCost(totals.Cost), fgStyle(theme.costColor)},
			},
			{
				{"T ", fgStyle(theme.dim)},
				{formatCompact(totals.Tokens), fgStyle(theme.fg)},
				{"  In ", fgStyle(theme.dim)},
				{formatCompact(totals.Input), fgStyle(theme.fg)},
				{"  Out ", fgStyle(theme.dim)},
				{formatCompact(totals.Output), fgStyle(theme.fg)},
				{"  CR ", fgStyle(theme.dim)},
				{formatCompact(totals.CacheRead), fgStyle(theme.fg)},
				{"  CW ", fgStyle(theme.dim)},
				{formatCompact(totals.CacheWrite), fgStyle(theme.fg)},
			},
		}
	}
	for index, spans := range overview {
		if index >= left.h {
			break
		}
		drawSpans(c, left.x, left.y+index, left.right(), spans)
	}
	if len(panels) < 2 {
		return
	}
	right := panels[1]
	if right.h <= 0 || right.w <= 0 {
		return
	}

	var lines [][]styledSpan
	lines = append(lines, []styledSpan{{"Agent / Model Cost", boldStyle(theme.dim)}})
	costWidth := max(min(10, max(right.w-12, 8)), 8)
	modelWidth := min(28, max(right.w-costWidth-3, 10))
	modelWidthInner := max(modelWidth-2, 8)
	if m.activityDetailError != "" {
		lines = append(lines, []styledSpan{{m.activityDetailError, fgStyle(theme.gaugeHigh)}})
	} else if m.activityDetail == nil || m.activityDetail.Date != m.activitySel {
		lines = append(lines, []styledSpan{{"Loading…", fgStyle(theme.dim)}})
	} else {
		for _, agent := range m.activityDetail.Agents {
			lines = append(lines, []styledSpan{
				{padRight(truncate(displayAgentName(agent.Agent), modelWidth), modelWidth), boldStyle(theme.fg)},
				{" ", fgStyle(theme.fg)},
				{padLeft(formatCost(agent.Totals.Cost), costWidth), boldStyle(theme.costColor)},
				{"  T ", fgStyle(theme.dim)},
				{formatCompact(agent.Totals.Tokens), boldStyle(theme.fg)},
			})
			for _, model := range agent.Models {
				lines = append(lines, []styledSpan{
					{"  ", fgStyle(theme.fg)},
					{padRight(truncate(model.Model, modelWidthInner), modelWidthInner), fgStyle(theme.companyColor(model.Company))},
					{" ", fgStyle(theme.fg)},
					{padLeft(formatCost(model.Cost), costWidth), fgStyle(theme.costColor)},
				})
				lines = append(lines, []styledSpan{
					{"    T ", fgStyle(theme.dim)},
					{padLeft(formatCompact(model.Tokens), 8), fgStyle(theme.dim)},
					{" I ", fgStyle(theme.dim)},
					{padLeft(formatCompact(model.Input), 8), fgStyle(theme.dim)},
					{" O ", fgStyle(theme.dim)},
					{padLeft(formatCompact(model.Output), 8), fgStyle(theme.dim)},
					{" CR ", fgStyle(theme.dim)},
					{padLeft(formatCompact(model.CacheRead), 8), fgStyle(theme.dim)},
					{" CW ", fgStyle(theme.dim)},
					{padLeft(formatCompact(model.CacheWrite), 8), fgStyle(theme.dim)},
				})
			}
		}
	}

	visible := right.h
	showHint := len(lines) > right.h && right.h > 1
	if showHint {
		visible = right.h - 1
	}
	start := clampInt(m.activityDetailOff, 0, max(len(lines)-visible, 0))
	m.activityDetailOff = start
	for index := 0; index < visible && start+index < len(lines); index++ {
		drawSpans(c, right.x, right.y+index, right.right(), lines[start+index])
	}
	if showHint {
		up, down := " ", " "
		if start > 0 {
			up = "↑"
		}
		if start+visible < len(lines) {
			down = "↓"
		}
		hint := fmt.Sprintf("%s%s detail %d-%d / %d", up, down, start+1, min(start+visible, len(lines)), len(lines))
		hintX := max(right.right()-len([]rune(hint)), right.x)
		c.setClip(hintX, right.y+right.h-1, right.right(), hint, fgStyle(theme.dim))
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
		m.activityLegend = -1
	case "c":
		m.activityMetric = metricCost
		m.activityLegend = -1
	case "up", "k":
		// m.activity.Days is newest-first; earlier dates sit higher on the heatmap.
		m.moveActivitySelection(1)
		return m, m.ensureActivityDetail()
	case "down", "j":
		m.moveActivitySelection(-1)
		return m, m.ensureActivityDetail()
	case "n":
		m.activitySel = dateString(startOfToday())
		return m, m.ensureActivityDetail()
	}
	return m, nil
}

func (m *model) moveActivitySelection(direction int) {
	days := m.activity.Days
	// days is newest-first; pos is the selection's slot, so a selected date
	// without usage (an empty heatmap cell) steps to its nearest neighbour.
	pos := len(days)
	for i, day := range days {
		if day.Date <= m.activitySel {
			pos = i
			break
		}
	}
	index := pos - 1
	if pos < len(days) && days[pos].Date == m.activitySel {
		index = pos + direction
	} else if direction > 0 {
		index = pos
	}
	if index < 0 || index >= len(days) {
		return
	}
	m.activitySel = days[index].Date
	m.activityDetailOff = 0
}

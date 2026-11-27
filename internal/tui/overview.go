package tui

import (
	"fmt"
	"math"
	"sort"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/report"
)

const (
	chartYAxisWidth = 7
	segmentOther    = "other"
)

type chartSegment struct {
	name  string
	value float64
}

type chartDay struct {
	date     string
	total    float64
	segments []chartSegment
}

func (m *model) drawOverview(c *canvas, area rect) {
	if area.h <= 0 || area.w <= 0 {
		return
	}
	barHeight := m.drawRangeBar(c, area)
	body := rect{x: area.x, y: area.y + barHeight, w: area.w, h: area.h - barHeight}
	if body.h <= 0 {
		return
	}
	if m.reportErr != "" {
		c.setClip(body.x, body.y, body.right(), m.reportErr, fgStyle(m.theme().gaugeHigh))
		return
	}

	period := m.reportLoaded && m.report.Range.From != m.report.Range.To
	kpiHeight := 1
	if period && m.report.Comparison != nil {
		kpiHeight = 2
	}
	kpiHeight = min(kpiHeight, body.h)
	kpi := rect{x: body.x, y: body.y, w: body.w, h: kpiHeight}
	m.drawOverviewKPI(c, kpi)
	rest := rect{x: body.x, y: body.y + kpiHeight, w: body.w, h: body.h - kpiHeight}
	if rest.h <= 0 {
		return
	}

	singleDay := m.reportLoaded && m.report.Range.From == m.report.Range.To
	if singleDay {
		m.drawTopModels(c, rest)
		return
	}
	chartHeight := max(rest.h*3/5, 8)
	if rest.h-chartHeight < 4 && rest.h >= 7 {
		chartHeight = rest.h - 4
	}
	chartHeight = min(max(chartHeight, 4), rest.h)
	chartArea := rect{x: rest.x, y: rest.y, w: rest.w, h: chartHeight}
	modelsArea := rect{x: rest.x, y: rest.y + chartHeight, w: rest.w, h: rest.h - chartHeight}
	m.drawOverviewChart(c, chartArea)
	if modelsArea.h > 0 {
		m.drawTopModels(c, modelsArea)
	}
}

func (m *model) drawOverviewKPI(c *canvas, area rect) {
	theme := m.theme()
	if area.h < 1 {
		return
	}
	period := m.reportLoaded && m.report.Range.From != m.report.Range.To
	comparison := m.report.Comparison
	if !period {
		comparison = nil
	}
	y := area.y
	x := area.x

	c.set(x, y, "Cost ", fgStyle(theme.dim))
	x += len("Cost ")
	cost := formatCost(m.report.Totals.Cost)
	c.set(x, y, cost, boldStyle(theme.costColor))
	x += len([]rune(cost))
	if comparison != nil {
		prior := comparison.Totals.Cost
		change := percentDeltaText(m.report.Totals.Cost, &prior)
		x += 2
		c.set(x, y, change, boldStyle(percentDeltaColor(theme, m.report.Totals.Cost, &prior)))
		x += len([]rune(change))
	}
	x += 3
	c.set(x, y, "Tokens ", fgStyle(theme.dim))
	x += len("Tokens ")
	tokens := formatCompact(m.report.Totals.Tokens)
	c.set(x, y, tokens, boldStyle(theme.codex))
	x += len([]rune(tokens))
	if comparison != nil {
		prior := float64(comparison.Totals.Tokens)
		current := float64(m.report.Totals.Tokens)
		change := percentDeltaText(current, &prior)
		x += 2
		c.set(x, y, change, boldStyle(percentDeltaColor(theme, current, &prior)))
		x += len([]rune(change))
	}
	x += 3
	c.set(x, y, "Sessions ", fgStyle(theme.dim))
	x += len("Sessions ")
	sessions := formatIntCommas(m.report.Totals.Sessions)
	c.set(x, y, sessions, boldStyle(theme.fg))
	x += len([]rune(sessions))
	if comparison != nil {
		prior := float64(comparison.Totals.Sessions)
		current := float64(m.report.Totals.Sessions)
		change := percentDeltaText(current, &prior)
		x += 2
		c.set(x, y, change, boldStyle(percentDeltaColor(theme, current, &prior)))
	}

	if area.h >= 2 && comparison != nil {
		y++
		text := "vs " + comparison.From + " → " + comparison.To
		c.setClip(area.x, y, area.right(), text, fgStyle(theme.dim))
	}
}

func (m *model) drawOverviewChart(c *canvas, area rect) {
	if area.w < 12 || area.h < 4 {
		return
	}
	theme := m.theme()
	inner := area.inner()
	if inner.h < 4 {
		return
	}
	chartArea := rect{x: inner.x, y: inner.y, w: inner.w, h: inner.h - 1}
	legendY := inner.y + inner.h - 1

	chartWidth := chartArea.w - (chartYAxisWidth + 1)
	data := m.chartSeries(max(chartWidth, 1))
	days := len(data)
	// Each day gets an equal slot across the full width; slots of 3+ columns
	// keep one blank column so neighbouring bars stay distinguishable.
	barWidth := max(chartWidth/max(days, 1), 1)
	fillWidth := barWidth
	if barWidth >= 3 {
		fillWidth = barWidth - 1
	}
	metricName := "Token Usage"
	if m.overviewMetric == metricCost {
		metricName = "Cost Usage"
	}
	title := fmt.Sprintf(" %s (%d days) ", metricName, days)
	c.box(area.x, area.y, area.w, area.h, title, boldStyle(theme.accent), fgStyle(theme.border))
	if days == 0 {
		if !m.reportLoading {
			c.set(chartArea.x, chartArea.y, "No usage data", fgStyle(theme.dim))
		}
		return
	}

	chartHeight := chartArea.h - 1
	if chartHeight < 1 {
		return
	}
	var maxValue float64
	for _, day := range data {
		if day.total > maxValue {
			maxValue = day.total
		}
	}
	if maxValue <= 0 {
		if !m.reportLoading {
			c.set(inner.x, inner.y, "No usage data", fgStyle(theme.dim))
		}
		return
	}

	ticks := max(min(chartHeight, 4), 2)
	for tick := 0; tick < ticks; tick++ {
		value := maxValue * float64(ticks-tick) / float64(ticks)
		row := chartArea.y + tick*chartHeight/ticks
		label := strings.TrimRight(formatYAxis(value, m.overviewMetric), " ")
		start := chartArea.x + chartYAxisWidth - lipgloss.Width(label)
		if start < chartArea.x {
			start = chartArea.x
		}
		c.set(start, row, label, fgStyle(theme.dim))
	}

	chartX := chartArea.x + chartYAxisWidth
	totalBarsWidth := len(data) * barWidth
	offset := 0
	if chartWidth > totalBarsWidth {
		offset = (chartWidth - totalBarsWidth) / 2
	}
	startX := chartX + offset

	for index, day := range data {
		barX := startX + index*barWidth
		if barX+barWidth > chartX+chartWidth {
			break
		}
		if day.total <= 0 {
			continue
		}
		heightEighths := max(1, int(math.Round(day.total/maxValue*float64(chartHeight*8))))
		fullRows := heightEighths / 8
		partial := heightEighths % 8
		segments := day.segments
		if len(segments) == 0 {
			segments = []chartSegment{{name: segmentOther, value: day.total}}
		}
		sort.SliceStable(segments, func(i, j int) bool {
			if segments[i].value == segments[j].value {
				return segments[i].name < segments[j].name
			}
			return segments[i].value > segments[j].value
		})
		units := allocateSegmentUnits(segments, day.total, heightEighths)
		var ranges []segmentRange
		allocated := 0
		for segmentIndex, segment := range segments {
			start := allocated
			allocated += min(units[segmentIndex], max(heightEighths-allocated, 0))
			ranges = append(ranges, segmentRange{
				start: start,
				end:   allocated,
				color: theme.companyColor(segment.name),
			})
		}
		for row := 0; row < fullRows; row++ {
			y := chartArea.y + chartHeight - 1 - row
			c.set(barX, y, strings.Repeat("█", fillWidth), fgStyle(dominantBandColor(ranges, row*8, row*8+8)))
		}
		if partial > 0 && fullRows < chartHeight {
			y := chartArea.y + chartHeight - 1 - fullRows
			blocks := []rune("▁▂▃▄▅▆▇█")
			c.set(barX, y, strings.Repeat(string(blocks[partial-1]), fillWidth), fgStyle(dominantBandColor(ranges, fullRows*8, fullRows*8+partial)))
		}
	}

	axisY := chartArea.y + chartHeight
	labels := make([]string, len(data))
	for i, day := range data {
		labels[i] = day.date[5:]
	}
	drawXAxis(c, startX, totalBarsWidth, barWidth, len(data), axisY, labels, theme)

	legend := []struct {
		name  string
		label string
	}{
		{"anthropic", "Anthropic"},
		{"openai", "OpenAI"},
		{"google", "Google"},
		{"other", "Others"},
	}
	x := inner.x
	for _, entry := range legend {
		text := "● " + entry.label
		c.set(x, legendY, text, fgStyle(theme.companyColor(entry.name)))
		x += len([]rune(text)) + 2
	}
}

func (m *model) chartSeries(limit int) []chartDay {
	if !m.reportLoaded || limit <= 0 {
		return nil
	}
	from, err := parseDate(m.report.Range.From)
	if err != nil {
		return nil
	}
	to, err := parseDate(m.report.Range.To)
	if err != nil {
		return nil
	}
	start := to.AddDate(0, 0, -(limit - 1))
	if start.Before(from) {
		start = from
	}
	byDate := make(map[string]report.DaySummary, len(m.report.Days))
	for _, day := range m.report.Days {
		byDate[day.Date] = day
	}
	var out []chartDay
	for day := start; !day.After(to); day = day.AddDate(0, 0, 1) {
		date := dateString(day)
		summary := byDate[date]
		total := float64(summary.Tokens)
		if m.overviewMetric == metricCost {
			total = summary.Cost
		}
		entry := chartDay{date: date, total: total}
		if len(summary.Companies) > 0 {
			for _, company := range []string{"anthropic", "openai", "google", segmentOther} {
				comp, ok := summary.Companies[company]
				if !ok {
					continue
				}
				value := float64(comp.Tokens)
				if m.overviewMetric == metricCost {
					value = comp.Cost
				}
				if value > 0 {
					entry.segments = append(entry.segments, chartSegment{name: company, value: value})
				}
			}
		}
		out = append(out, entry)
	}
	return out
}

type segmentRange struct {
	start, end int
	color      lipgloss.Color
}

func dominantBandColor(ranges []segmentRange, start, end int) lipgloss.Color {
	best := -1
	var color lipgloss.Color
	for _, segment := range ranges {
		overlap := min(end, segment.end) - max(start, segment.start)
		if overlap > best {
			best = overlap
			color = segment.color
		}
	}
	return color
}

func allocateSegmentUnits(segments []chartSegment, total float64, units int) []int {
	allocated := make([]int, len(segments))
	if total <= 0 || units <= 0 || len(segments) == 0 {
		return allocated
	}
	type remainder struct {
		index int
		value float64
	}
	var remainders []remainder
	used := 0
	for index, segment := range segments {
		exact := segment.value / total * float64(units)
		whole := max(int(math.Floor(exact)), 0)
		allocated[index] = whole
		used += whole
		remainders = append(remainders, remainder{index: index, value: exact - float64(whole)})
	}
	sort.SliceStable(remainders, func(i, j int) bool {
		if remainders[i].value == remainders[j].value {
			return remainders[i].index < remainders[j].index
		}
		return remainders[i].value > remainders[j].value
	})
	for i := 0; i < units-used && i < len(remainders); i++ {
		allocated[remainders[i].index]++
	}
	return allocated
}

func drawXAxis(c *canvas, chartX, totalBarsWidth, barWidth, bars int, y int, labels []string, theme tuiTheme) {
	labelWidth := 5
	if len(labels) == 0 || bars == 0 || totalBarsWidth < labelWidth {
		return
	}
	maxTicks := clampInt(1+max(totalBarsWidth-1, 0)/(labelWidth+3), 2, 6)
	ticks := min(maxTicks, bars)
	lastEnd := -1
	for tick := 0; tick < ticks; tick++ {
		index := 0
		if ticks > 1 {
			index = int(math.Round(float64(tick) * float64(bars-1) / float64(ticks-1)))
		}
		label := labels[min(index, len(labels)-1)]
		width := len([]rune(label))
		center := float64(chartX) + (float64(index)+0.5)*float64(barWidth)
		start := clampInt(int(math.Round(center-float64(width)/2)), chartX, chartX+totalBarsWidth-width)
		if start <= lastEnd {
			continue
		}
		c.set(start, y, label, fgStyle(theme.dim))
		lastEnd = start + width
	}
}

func formatYAxis(value float64, metric int) string {
	if metric == metricCost {
		switch {
		case value >= 100:
			return "$" + formatIntCommas(int64(math.Round(value)))
		case value >= 10:
			return fmt.Sprintf("$%.1f", value)
		case value >= 0.01:
			return fmt.Sprintf("$%.2f", value)
		default:
			return "$0"
		}
	}
	return formatCompact(int64(math.Round(value)))
}

func (m *model) overviewModels() []report.ModelSummary {
	models := append([]report.ModelSummary(nil), m.report.Models...)
	value := func(entry report.ModelSummary) float64 {
		if m.overviewMetric == metricCost {
			return entry.Cost
		}
		return float64(entry.Tokens)
	}
	sort.SliceStable(models, func(i, j int) bool {
		if value(models[i]) == value(models[j]) {
			if models[i].Cost == models[j].Cost {
				return models[i].Model < models[j].Model
			}
			return models[i].Cost > models[j].Cost
		}
		return value(models[i]) > value(models[j])
	})
	return models
}

func (m *model) handleOverviewKey(key string) (tea.Model, tea.Cmd) {
	switch key {
	case "t":
		if m.overviewMetric != metricTokens {
			m.overviewMetric = metricTokens
			m.overviewSel, m.overviewOff = 0, 0
		}
	case "c":
		if m.overviewMetric != metricCost {
			m.overviewMetric = metricCost
			m.overviewSel, m.overviewOff = 0, 0
		}
	case "down", "j":
		if m.overviewSel < len(m.report.Models)-1 {
			m.overviewSel++
		}
	case "up", "k":
		if m.overviewSel > 0 {
			m.overviewSel--
		}
	case "enter":
		models := m.overviewModels()
		if len(models) == 0 {
			return m, nil
		}
		entry := models[clampInt(m.overviewSel, 0, len(models)-1)]
		return m, m.loadModelDetailCmd(entry.Model)
	}
	return m, nil
}

func (m *model) drawTopModels(c *canvas, area rect) {
	theme := m.theme()
	models := m.overviewModels()
	totalRows := len(models)
	inner := area.inner()
	hintSpace := 1
	if inner.h <= 2 {
		hintSpace = 0
	}
	dataRows := max(inner.h-1-hintSpace, 0)
	selected := clampInt(m.overviewSel, 0, max(totalRows-1, 0))
	offset := normalizeScroll(m.overviewOff, selected, dataRows, totalRows)
	m.overviewOff = offset
	title := fmt.Sprintf(" Top Models (%d) %s ", totalRows, scrollHintWindow(offset, dataRows, totalRows))
	c.box(area.x, area.y, area.w, area.h, title, boldStyle(theme.accentSoft), fgStyle(theme.border))
	if inner.h <= 0 {
		return
	}
	if totalRows == 0 {
		c.set(inner.x, inner.y, "No model data", fgStyle(theme.dim))
		return
	}
	pctWidth, costWidth, tokensWidth := 7, 10, 9
	fixed := tokensWidth + costWidth + pctWidth + 4
	modelWidth, agentWidth := calculateModelAgentWidths(sortedModelRows(models), max(inner.w-fixed, 0))
	shareField := sortTokens
	if m.overviewMetric == metricCost {
		shareField = sortCost
	}
	shareTotal := modelShareTotal(models, shareField)

	x := inner.x
	c.setClip(x, inner.y, inner.right(), padRight(truncate("Model", modelWidth), modelWidth), boldStyle(theme.dim))
	x += modelWidth + 1
	c.setClip(x, inner.y, inner.right(), padRight(truncate("Agent", agentWidth), agentWidth), boldStyle(theme.dim))
	x += agentWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft("Tokens", tokensWidth), boldStyle(theme.dim))
	x += tokensWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft("Cost", costWidth), boldStyle(theme.dim))
	x += costWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft("%", pctWidth), boldStyle(theme.dim))

	for index := 0; index < dataRows; index++ {
		rowIndex := offset + index
		if rowIndex >= totalRows {
			break
		}
		entry := models[rowIndex]
		y := inner.y + 1 + index
		rowBg := lipgloss.Color("")
		selectedRow := rowIndex == selected
		if selectedRow {
			rowBg = theme.selectedBg
			c.fill(inner.x, y, inner.w, 1, textStyle{bg: rowBg})
		}
		style := func(color lipgloss.Color, bold bool) textStyle {
			return textStyle{fg: color, bg: rowBg, bold: bold}
		}
		x = inner.x
		c.setClip(x, y, inner.right(), padRight(truncate(entry.Model, modelWidth), modelWidth), style(theme.companyColor(entry.Company), selectedRow))
		x += modelWidth + 1
		c.setClip(x, y, inner.right(), padRight(truncate(displaySourceList(entry.Agents), agentWidth), agentWidth), style(theme.dim, selectedRow))
		x += agentWidth + 1
		c.setClip(x, y, inner.right(), padLeft(formatCompact(entry.Tokens), tokensWidth), style(theme.fg, selectedRow))
		x += tokensWidth + 1
		c.setClip(x, y, inner.right(), padLeft(formatCost(entry.Cost), costWidth), style(theme.costColor, selectedRow))
		x += costWidth + 1
		c.setClip(x, y, inner.right(), padLeft(fmt.Sprintf("%.2f%%", modelShare(entry, shareTotal, shareField)), pctWidth), style(theme.dim, selectedRow))
	}
	if hintSpace == 1 {
		drawn := min(dataRows, max(totalRows-offset, 0))
		hintY := inner.y + 1 + drawn
		if hintY < inner.bottom() {
			c.set(inner.x, hintY, truncate(scrollHint(offset, dataRows, totalRows), inner.w), fgStyle(theme.dim))
		}
	}
}

func scrollHintWindow(offset, visible, total int) string {
	if total == 0 {
		return ""
	}
	start := offset + 1
	end := min(offset+visible, total)
	if end < start {
		end = start
	}
	return fmt.Sprintf("%d-%d", start, end)
}

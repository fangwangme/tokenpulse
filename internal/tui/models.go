package tui

import (
	"fmt"
	"sort"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/report"
)

func (m *model) drawRangeBar(c *canvas, area rect) int {
	if area.h <= 0 || area.w <= 0 {
		return 0
	}
	theme := m.theme()
	y := area.y
	if m.currentRangeTab() {
		x := area.x
		for index, preset := range presets {
			label := fmt.Sprintf("%d %s", index+1, preset)
			style := fgStyle(theme.dim)
			if preset == m.preset {
				style = textStyle{fg: theme.onAccent, bg: theme.accent, bold: true}
				label = " " + label + " "
				c.fill(x, y, len([]rune(label)), 1, style)
			}
			c.set(x, y, label, style)
			x += len([]rune(label)) + 2
		}
		from, to := m.report.Range.From, m.report.Range.To
		if !m.reportLoaded {
			if m.preset == "custom" {
				from, to = m.from, m.to
			} else {
				from, to = "…", "…"
			}
		}
		x += 2
		dates := from + " → " + to
		c.set(x, y, dates, fgStyle(theme.fg))
		x += len([]rune(dates))
		if m.report.ScannedAt != 0 {
			scanned := "  scanned " + formatScanned(m.report.ScannedAt)
			c.set(x, y, scanned, fgStyle(theme.dim))
		}
		if m.customOpen && area.h > 1 {
			m.drawCustomEntry(c, rect{x: area.x, y: area.y + 1, w: area.w, h: 1})
			return 2
		}
		return 1
	}
	if m.tab == tabActivity {
		from, to, ok := m.activityWindow()
		if !ok && m.activityLoaded {
			from, to = m.activity.Range.From, m.activity.Range.To
		}
		label := "Window "
		c.set(area.x, y, label, fgStyle(theme.dim))
		c.set(area.x+len(label), y, from+" → "+to, fgStyle(theme.fg))
		if m.activityErr != "" {
			c.set(area.x+len(label)+len(from)+len(to)+7, y, m.activityErr, fgStyle(theme.gaugeHigh))
		}
		return 1
	}
	return 0
}

func (m *model) drawCustomEntry(c *canvas, area rect) {
	theme := m.theme()
	x := area.x
	c.set(x, area.y, "From ", fgStyle(theme.dim))
	x += len("From ")
	fromStyle := fgStyle(theme.fg)
	if m.customField == 0 {
		fromStyle = textStyle{fg: theme.onAccent, bg: theme.accent, bold: true}
	}
	c.set(x, area.y, padRight(m.customFrom, 10), fromStyle)
	x += 10 + 3
	c.set(x, area.y, "To ", fgStyle(theme.dim))
	x += len("To ")
	toStyle := fgStyle(theme.fg)
	if m.customField == 1 {
		toStyle = textStyle{fg: theme.onAccent, bg: theme.accent, bold: true}
	}
	c.set(x, area.y, padRight(m.customTo, 10), toStyle)
	x += 10 + 3
	c.set(x, area.y, "enter apply · esc cancel", fgStyle(theme.dim))
	if m.customErr != "" {
		c.set(x+len("enter apply · esc cancel")+2, area.y, m.customErr, fgStyle(theme.gaugeHigh))
	}
}

func (m *model) handleOverviewKey(key string) (tea.Model, tea.Cmd) {
	switch key {
	case "t":
		m.overviewMetric = metricTokens
	case "c":
		m.overviewMetric = metricCost
	case "down", "j":
		if m.overviewSel < len(m.report.Models)-1 {
			m.overviewSel++
		}
	case "up", "k":
		if m.overviewSel > 0 {
			m.overviewSel--
		}
	}
	return m, nil
}

func (m *model) drawModels(c *canvas, area rect) {
	theme := m.theme()
	c.box(area.x, area.y, area.w, area.h, " Models ", boldStyle(theme.accent), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 || inner.w <= 0 {
		return
	}
	models := m.sortedModels()
	if len(models) == 0 {
		c.set(inner.x, inner.y, "No model data", fgStyle(theme.dim))
		return
	}

	totalWidth := inner.w
	rankWidth := 4
	tokensWidth := 9
	costWidth := 10
	inputWidth := 9
	outputWidth := 9
	cacheSplitWide := totalWidth >= 100
	cacheReadWidth := 7
	cacheWriteWidth := 7
	cacheRHeader, cacheWHeader, hitHeader := "CR", "CW", "H%"
	if cacheSplitWide {
		cacheReadWidth, cacheWriteWidth = 9, 9
		cacheRHeader, cacheWHeader, hitHeader = "Cache R", "Cache W", "Hit %"
	}
	hitWidth := 7
	pctWidth := 7
	msgWidth := 8
	showLast := totalWidth >= 120
	lastWidth := 0
	if showLast {
		lastWidth = 11
	}
	fixed := rankWidth + tokensWidth + costWidth + inputWidth + outputWidth +
		cacheReadWidth + cacheWriteWidth + hitWidth + pctWidth + msgWidth + lastWidth
	gaps := 11
	if showLast {
		gaps = 12
	}
	// Keep Model and Agent visible even when the fixed metric columns exceed
	// the row: the remaining columns clip at the right edge like 0.6.16.
	modelWidth, agentWidth := calculateModelAgentWidths(sortedModelRows(models), max(totalWidth-fixed-gaps, 23))
	shareTotal := modelShareTotal(models, m.sortField)

	x := inner.x
	c.set(x, inner.y, padRight("#", rankWidth), boldStyle(theme.dim))
	x += rankWidth + 1
	c.set(x, inner.y, padRight(truncate("Model", modelWidth), modelWidth), boldStyle(theme.accent))
	x += modelWidth + 1
	c.set(x, inner.y, padRight(truncate("Agent", agentWidth), agentWidth), boldStyle(theme.accentSoft))
	x += agentWidth + 1
	c.set(x, inner.y, padLeft("Tokens"+sortIndicator(m.sortField == sortTokens, m.sortAsc), tokensWidth), boldStyle(theme.codex))
	x += tokensWidth + 1
	c.set(x, inner.y, padLeft("Cost"+sortIndicator(m.sortField == sortCost, m.sortAsc), costWidth), boldStyle(theme.gaugeMid))
	x += costWidth + 1
	c.set(x, inner.y, padLeft("Input", inputWidth), boldStyle(theme.gemini))
	x += inputWidth + 1
	c.set(x, inner.y, padLeft("Output", outputWidth), boldStyle(rgb(167, 139, 250)))
	x += outputWidth + 1
	c.set(x, inner.y, padLeft(cacheRHeader, cacheReadWidth), boldStyle(rgb(251, 146, 60)))
	x += cacheReadWidth + 1
	c.set(x, inner.y, padLeft(cacheWHeader, cacheWriteWidth), boldStyle(rgb(251, 146, 60)))
	x += cacheWriteWidth + 1
	c.set(x, inner.y, padLeft(hitHeader, hitWidth), boldStyle(rgb(251, 146, 60)))
	x += hitWidth + 1
	c.set(x, inner.y, padLeft("%", pctWidth), boldStyle(theme.gemini))
	x += pctWidth + 1
	c.set(x, inner.y, padLeft("Msgs", msgWidth), boldStyle(theme.gemini))
	x += msgWidth + 1
	if showLast {
		c.set(x, inner.y, padLeft("Last"+sortIndicator(m.sortField == sortDate, m.sortAsc), lastWidth), boldStyle(theme.dim))
	}

	visible := max(inner.h-2, 0)
	selected := clampInt(m.modelsSel, 0, len(models)-1)
	offset := normalizeScroll(m.modelsOff, selected, visible, len(models))
	m.modelsOff = offset

	for index := 0; index < visible; index++ {
		rowIndex := offset + index
		if rowIndex >= len(models) {
			break
		}
		entry := models[rowIndex]
		y := inner.y + 1 + index
		selectedRow := rowIndex == selected
		rowBg := lipgloss.Color("")
		if selectedRow {
			rowBg = theme.selectedBg
		}
		col := func(color lipgloss.Color, bold bool) textStyle {
			return textStyle{fg: color, bg: rowBg, bold: bold}
		}
		x = inner.x
		c.set(x, y, padRight(fmt.Sprintf("%d", rowIndex+1), rankWidth), col(theme.dim, false))
		x += rankWidth + 1
		c.set(x, y, padRight(truncate(entry.Model, modelWidth), modelWidth), col(theme.companyColor(entry.Company), selectedRow))
		x += modelWidth + 1
		c.set(x, y, padRight(truncate(displaySourceList(entry.Agents), agentWidth), agentWidth), col(theme.accentSoft, selectedRow))
		x += agentWidth + 1
		c.set(x, y, padLeft(formatCompact(entry.Tokens), tokensWidth), col(theme.codex, selectedRow))
		x += tokensWidth + 1
		c.set(x, y, padLeft(formatCost(entry.Cost), costWidth), col(theme.gaugeMid, selectedRow))
		x += costWidth + 1
		c.set(x, y, padLeft(formatCompact(entry.Input), inputWidth), col(theme.gemini, selectedRow))
		x += inputWidth + 1
		c.set(x, y, padLeft(formatCompact(entry.Output), outputWidth), col(rgb(167, 139, 250), selectedRow))
		x += outputWidth + 1
		c.set(x, y, padLeft(formatCompact(entry.CacheRead), cacheReadWidth), col(rgb(251, 146, 60), selectedRow))
		x += cacheReadWidth + 1
		c.set(x, y, padLeft(formatCompact(entry.CacheWrite), cacheWriteWidth), col(rgb(251, 146, 60), selectedRow))
		x += cacheWriteWidth + 1
		c.set(x, y, padLeft(cacheHitPercent(entry.CacheRead, entry.Tokens), hitWidth), col(rgb(251, 146, 60), selectedRow))
		x += hitWidth + 1
		c.set(x, y, padLeft(fmt.Sprintf("%.1f%%", modelShare(entry, shareTotal, m.sortField)), pctWidth), col(theme.gemini, selectedRow))
		x += pctWidth + 1
		c.set(x, y, padLeft(formatCompact(entry.Messages), msgWidth), col(theme.gemini, selectedRow))
		x += msgWidth + 1
		if showLast {
			last := entry.LastUsed
			if last == "" {
				last = "n/a"
			}
			c.set(x, y, padLeft(last, lastWidth), col(theme.dim, selectedRow))
		}
	}
	hint := scrollHint(offset, visible, len(models))
	drawn := min(visible, max(len(models)-offset, 0))
	hintY := inner.y + 1 + drawn
	if hintY < inner.bottom() {
		c.set(inner.x, hintY, truncate(hint, inner.w), fgStyle(theme.dim))
	}
}

func sortIndicator(active, ascending bool) string {
	if !active {
		return ""
	}
	if ascending {
		return " ↑"
	}
	return " ↓"
}

func (m *model) handleModelsKey(key string) (tea.Model, tea.Cmd) {
	switch key {
	case "c", "t", "d":
		field := sortCost
		switch key {
		case "t":
			field = sortTokens
		case "d":
			field = sortDate
		}
		if m.sortField == field {
			m.sortAsc = !m.sortAsc
		} else {
			m.sortField = field
			m.sortAsc = false
		}
		m.modelsSel, m.modelsOff = 0, 0
	case "down", "j":
		if m.modelsSel < len(m.report.Models)-1 {
			m.modelsSel++
		}
	case "up", "k":
		if m.modelsSel > 0 {
			m.modelsSel--
		}
	case "enter":
		models := m.sortedModels()
		if len(models) == 0 {
			return m, nil
		}
		entry := models[clampInt(m.modelsSel, 0, len(models)-1)]
		return m, m.loadModelDetailCmd(entry.Model)
	}
	return m, nil
}

func (m *model) loadModelDetailCmd(name string) tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	key := m.rangeKey()
	from, to := m.report.Range.From, m.report.Range.To
	agents := m.selectedAgents()
	return func() tea.Msg {
		value, err := client.UsageModel(name, from, to, agents)
		return modelDetailMsg{key: key, model: name, purpose: "detail", value: value, err: err}
	}
}

func (m *model) drawModelDetail(c *canvas, area rect) {
	theme := m.theme()
	detail := m.modelDetail
	box := popup(area, min(90, area.w-4), min(24, area.h-2))
	c.fill(box.x, box.y, box.w, box.h, textStyle{})
	c.box(box.x, box.y, box.w, box.h, " "+detail.Model+" ", boldStyle(theme.companyColor(detail.Company)), fgStyle(theme.border))
	inner := box.inner()
	lines := []string{
		detail.Company + " · " + m.report.Range.From + " → " + m.report.Range.To,
		"",
		fmt.Sprintf("Cost %s   Tokens %s", formatCost(detail.Totals.Cost), formatCompact(detail.Totals.Tokens)),
		fmt.Sprintf("Input %s   Output %s   Cache read %s   Cache write %s",
			formatCompact(detail.Totals.Input), formatCompact(detail.Totals.Output),
			formatCompact(detail.Totals.CacheRead), formatCompact(detail.Totals.CacheWrite)),
		fmt.Sprintf("Messages %s   Sessions %s   Last used %s",
			formatIntCommas(detail.Totals.Messages), formatIntCommas(detail.Totals.Sessions), lastUsed(detail.LastUsed)),
		"",
		"By agent",
	}
	for _, agent := range detail.Agents {
		lines = append(lines, fmt.Sprintf("  %-14s %10s  %10s", displayAgentName(agent.Agent), formatCost(agent.Cost), formatCompact(agent.Tokens)))
	}
	if m.report.Range.From != m.report.Range.To && len(detail.PerDay) > 1 {
		values := make([]int64, 0, len(detail.PerDay))
		for _, day := range detail.PerDay {
			values = append(values, day.Tokens)
		}
		lines = append(lines, "", "Daily tokens "+sparklineText(values))
	}
	lines = append(lines, "", "Esc closes")
	for index, line := range lines {
		if index >= inner.h {
			break
		}
		c.set(inner.x, inner.y+index, truncate(line, inner.w), fgStyle(theme.fg))
	}
}

func lastUsed(value string) string {
	if value == "" {
		return "n/a"
	}
	return value
}

func (m *model) drawDayDetail(c *canvas, area rect) {
	theme := m.theme()
	detail := m.dayDetail
	box := popup(area, min(100, area.w-4), min(26, area.h-2))
	c.fill(box.x, box.y, box.w, box.h, textStyle{})
	c.box(box.x, box.y, box.w, box.h, " Day "+detail.Date+" ", boldStyle(theme.companyColor("other")), fgStyle(theme.border))
	inner := box.inner()
	var lines []string
	lines = append(lines,
		fmt.Sprintf("Cost %s   Tokens %s   Messages %s   Sessions %s",
			formatCost(detail.Totals.Cost), formatCompact(detail.Totals.Tokens),
			formatIntCommas(detail.Totals.Messages), formatIntCommas(detail.Totals.Sessions)),
		fmt.Sprintf("Input %s   Output %s   Cache read %s   Cache write %s",
			formatCompact(detail.Totals.Input), formatCompact(detail.Totals.Output),
			formatCompact(detail.Totals.CacheRead), formatCompact(detail.Totals.CacheWrite)),
		"",
	)
	for _, agent := range detail.Agents {
		lines = append(lines, fmt.Sprintf("%s  %s", displayAgentName(agent.Agent), formatCost(agent.Totals.Cost)))
		for _, model := range agent.Models {
			lines = append(lines, fmt.Sprintf("  %-34s %10s  %9s  CR %9s  hit %s",
				truncate(model.Model, 34), formatCost(model.Cost), formatCompact(model.Tokens),
				formatCompact(model.CacheRead), cacheHitPercent(model.CacheRead, model.Tokens)))
		}
	}
	lines = append(lines, "", "Esc closes")
	start := clampInt(m.detailScroll, 0, max(len(lines)-inner.h, 0))
	for index := 0; index < inner.h && start+index < len(lines); index++ {
		c.set(inner.x, inner.y+index, truncate(lines[start+index], inner.w), fgStyle(theme.fg))
	}
}

func (m *model) handleDailyKey(key string) (tea.Model, tea.Cmd) {
	switch key {
	case "c", "t", "d":
		field := sortCost
		switch key {
		case "t":
			field = sortTokens
		case "d":
			field = sortDate
		}
		if m.sortField == field {
			m.sortAsc = !m.sortAsc
		} else {
			m.sortField = field
			m.sortAsc = false
		}
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
	return func() tea.Msg {
		value, err := client.UsageDay(date, agents)
		if err != nil {
			return toastMsg{"Day: " + err.Error()}
		}
		return dayDetailMsg{key: key, date: date, value: value}
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

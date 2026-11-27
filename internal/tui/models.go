package tui

import (
	"fmt"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

type rangeHitbox struct {
	index int
	x0    int
	x1    int
}

func rangePresetHitboxes(area rect) []rangeHitbox {
	boxes := make([]rangeHitbox, 0, len(presets))
	x := area.x
	for index, preset := range presets {
		label := fmt.Sprintf(" %d %s ", index+1, preset)
		w := len([]rune(label))
		boxes = append(boxes, rangeHitbox{index: index, x0: x, x1: x + w})
		x += w + 1
	}
	return boxes
}

func (m *model) drawRangeBar(c *canvas, area rect) int {
	if area.h <= 0 || area.w <= 0 {
		return 0
	}
	theme := m.theme()
	y := area.y
	if m.currentRangeTab() {
		x := area.x
		for index, preset := range presets {
			key := fmt.Sprintf("%d", index+1)
			label := fmt.Sprintf(" %s %s ", key, preset)
			w := len([]rune(label))
			if preset == m.preset {
				style := textStyle{fg: theme.onAccent, bg: theme.accent, bold: true}
				c.fill(x, y, w, 1, style)
				c.set(x, y, label, style)
			} else {
				pillStyle := textStyle{fg: theme.fg, bg: theme.pillBg}
				keyStyle := textStyle{fg: theme.accentSoft, bg: theme.pillBg, bold: true}
				c.fill(x, y, w, 1, pillStyle)
				c.set(x+1, y, key, keyStyle)
				c.set(x+1+len(key), y, " "+preset, pillStyle)
			}
			x += w + 1
		}
		from, to := m.report.Range.From, m.report.Range.To
		if !m.reportLoaded {
			if m.preset == "custom" {
				from, to = m.from, m.to
			} else {
				from, to = "…", "…"
			}
		}
		x++
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

func (m *model) drawModels(c *canvas, area rect) {
	theme := m.theme()
	barHeight := m.drawRangeBar(c, area)
	area = rect{x: area.x, y: area.y + barHeight, w: area.w, h: area.h - barHeight}
	if area.h < 2 {
		return
	}
	c.box(area.x, area.y, area.w, area.h, " Models ", boldStyle(theme.accent), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 || inner.w <= 0 {
		return
	}
	if m.reportErr != "" {
		c.set(inner.x, inner.y, truncate(m.reportErr, inner.w), fgStyle(theme.gaugeHigh))
		return
	}
	models := m.sortedModels()
	if len(models) == 0 {
		if m.reportLoading {
			c.set(inner.x, inner.y, "Loading…", fgStyle(theme.dim))
		} else {
			c.set(inner.x, inner.y, "No model data", fgStyle(theme.dim))
		}
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

	headerStyle := func(active bool) textStyle {
		if active {
			return boldStyle(theme.accent)
		}
		return boldStyle(theme.dim)
	}
	x := inner.x
	c.setClip(x, inner.y, inner.right(), padRight("#", rankWidth), boldStyle(theme.dim))
	x += rankWidth + 1
	c.setClip(x, inner.y, inner.right(), padRight(truncate("Model", modelWidth), modelWidth), boldStyle(theme.dim))
	x += modelWidth + 1
	c.setClip(x, inner.y, inner.right(), padRight(truncate("Agent", agentWidth), agentWidth), boldStyle(theme.dim))
	x += agentWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft("Tokens"+sortIndicator(m.sortField == sortTokens, m.sortAsc), tokensWidth), headerStyle(m.sortField == sortTokens))
	x += tokensWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft("Cost"+sortIndicator(m.sortField == sortCost, m.sortAsc), costWidth), headerStyle(m.sortField == sortCost))
	x += costWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft("Input", inputWidth), boldStyle(theme.dim))
	x += inputWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft("Output", outputWidth), boldStyle(theme.dim))
	x += outputWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft(cacheRHeader, cacheReadWidth), boldStyle(theme.dim))
	x += cacheReadWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft(cacheWHeader, cacheWriteWidth), boldStyle(theme.dim))
	x += cacheWriteWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft(hitHeader, hitWidth), boldStyle(theme.dim))
	x += hitWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft("%", pctWidth), boldStyle(theme.dim))
	x += pctWidth + 1
	c.setClip(x, inner.y, inner.right(), padLeft("Msgs", msgWidth), boldStyle(theme.dim))
	x += msgWidth + 1
	if showLast {
		c.setClip(x, inner.y, inner.right(), padLeft("Last"+sortIndicator(m.sortField == sortDate, m.sortAsc), lastWidth), headerStyle(m.sortField == sortDate))
	}

	visible, showHint := tableRows(len(models), max(inner.h-1, 0))
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
			c.fill(inner.x, y, inner.w, 1, textStyle{bg: rowBg})
		}
		col := func(color lipgloss.Color, bold bool) textStyle {
			return textStyle{fg: color, bg: rowBg, bold: bold}
		}
		x = inner.x
		c.setClip(x, y, inner.right(), padRight(fmt.Sprintf("%d", rowIndex+1), rankWidth), col(theme.dim, false))
		x += rankWidth + 1
		c.setClip(x, y, inner.right(), padRight(truncate(entry.Model, modelWidth), modelWidth), col(theme.companyColor(entry.Company), selectedRow))
		x += modelWidth + 1
		c.setClip(x, y, inner.right(), padRight(truncate(displaySourceList(entry.Agents), agentWidth), agentWidth), col(theme.dim, selectedRow))
		x += agentWidth + 1
		c.setClip(x, y, inner.right(), padLeft(formatCompact(entry.Tokens), tokensWidth), col(theme.fg, selectedRow || m.sortField == sortTokens))
		x += tokensWidth + 1
		c.setClip(x, y, inner.right(), padLeft(formatCost(entry.Cost), costWidth), col(theme.costColor, selectedRow || m.sortField == sortCost))
		x += costWidth + 1
		c.setClip(x, y, inner.right(), padLeft(formatCompact(entry.Input), inputWidth), col(theme.fg, selectedRow))
		x += inputWidth + 1
		c.setClip(x, y, inner.right(), padLeft(formatCompact(entry.Output), outputWidth), col(theme.fg, selectedRow))
		x += outputWidth + 1
		c.setClip(x, y, inner.right(), padLeft(formatCompact(entry.CacheRead), cacheReadWidth), col(theme.fg, selectedRow))
		x += cacheReadWidth + 1
		c.setClip(x, y, inner.right(), padLeft(formatCompact(entry.CacheWrite), cacheWriteWidth), col(theme.fg, selectedRow))
		x += cacheWriteWidth + 1
		c.setClip(x, y, inner.right(), padLeft(cacheHitPercent(entry.CacheRead, entry.Tokens), hitWidth), col(theme.dim, selectedRow))
		x += hitWidth + 1
		c.setClip(x, y, inner.right(), padLeft(fmt.Sprintf("%.1f%%", modelShare(entry, shareTotal, m.sortField)), pctWidth), col(theme.dim, selectedRow))
		x += pctWidth + 1
		c.setClip(x, y, inner.right(), padLeft(formatCompact(entry.Messages), msgWidth), col(theme.dim, selectedRow))
		x += msgWidth + 1
		if showLast {
			last := entry.LastUsed
			if last == "" {
				last = "n/a"
			}
			c.setClip(x, y, inner.right(), padLeft(last, lastWidth), col(theme.dim, selectedRow))
		}
	}
	if showHint {
		drawn := min(visible, max(len(models)-offset, 0))
		hintY := inner.y + 1 + drawn
		if hintY < inner.bottom() {
			c.set(inner.x, hintY, truncate(scrollHint(offset, visible, len(models)), inner.w), fgStyle(theme.dim))
		}
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

func toggleSort(key string, field *int, asc *bool) {
	next := sortCost
	switch key {
	case "t":
		next = sortTokens
	case "d":
		next = sortDate
	}
	if *field == next {
		*asc = !*asc
	} else {
		*field = next
		*asc = false
	}
}

func (m *model) handleModelsKey(key string) (tea.Model, tea.Cmd) {
	switch key {
	case "c", "t", "d":
		toggleSort(key, &m.sortField, &m.sortAsc)
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
	m.detailSeq++
	seq := m.detailSeq
	from, to := m.report.Range.From, m.report.Range.To
	agents := m.selectedAgents()
	return func() tea.Msg {
		value, err := client.UsageModel(name, from, to, agents)
		return modelDetailMsg{key: key, purpose: "detail", seq: seq, value: value, err: err}
	}
}

func (m *model) drawModelDetail(c *canvas, area rect) {
	theme := m.theme()
	detail := m.modelDetail
	box := popup(area, min(90, area.w-4), min(24, area.h-2))
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
	start := clampInt(m.detailScroll, 0, max(len(lines)-inner.h, 0))
	m.detailScroll = start
	for index := 0; index < inner.h && start+index < len(lines); index++ {
		c.set(inner.x, inner.y+index, truncate(lines[start+index], inner.w), fgStyle(theme.fg))
	}
}

func lastUsed(value string) string {
	if value == "" {
		return "n/a"
	}
	return value
}

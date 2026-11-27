package tui

import (
	"github.com/fangwangme/tokenpulse/internal/report"
)

type keyHelp struct {
	key  string
	desc string
}

func (m *model) helpBindings() []keyHelp {
	bindings := []keyHelp{
		{"←→ / Tab", "switch tab"},
		{"↑↓ / j k", "navigate lists"},
		{"s", "toggle agent filter"},
		{"b", "toggle light / dark theme"},
		{"r", "refresh usage and quota"},
		{"Esc", "close detail or overlay"},
		{"?", "toggle this help overlay"},
		{"q / Ctrl+C", "quit dashboard"},
	}
	switch m.tab {
	case tabOverview:
		bindings = append(bindings,
			keyHelp{"t", "chart and top models by tokens"},
			keyHelp{"c", "chart and top models by cost"},
			keyHelp{"Enter", "open the selected model detail"},
			keyHelp{"1–6", "range preset today/7d/30d/year/all/custom"},
			keyHelp{"[ / ]", "shift the range earlier / later"},
			keyHelp{"n", "back to the preset range ending today"},
		)
	case tabModels:
		bindings = append(bindings,
			keyHelp{"c / t / d", "sort by cost / tokens / last used"},
			keyHelp{"Enter", "open the model detail"},
			keyHelp{"1–6", "range preset today/7d/30d/year/all/custom"},
			keyHelp{"[ / ]", "shift the range earlier / later"},
			keyHelp{"n", "back to the preset range ending today"},
		)
	case tabDaily:
		bindings = append(bindings,
			keyHelp{"n", "select today and the preset range ending today"},
			keyHelp{"c / t / d", "sort by cost / tokens / date"},
			keyHelp{"v", "switch the vs-7-days metric"},
			keyHelp{"Enter", "open the day detail"},
			keyHelp{"1–6", "range preset today/7d/30d/year/all/custom"},
			keyHelp{"[ / ]", "shift the range earlier / later"},
		)
	case tabActivity:
		bindings = append(bindings,
			keyHelp{"↑↓ / j k", "move the selected day"},
			keyHelp{"n", "select today"},
			keyHelp{"t / c", "heatmap metric tokens / cost"},
		)
	case tabKeeper:
		bindings = append(bindings,
			keyHelp{"1 / d", "toggle the 5h daily keeper"},
			keyHelp{"2 / w", "toggle the weekly keeper"},
			keyHelp{"p", "trigger an immediate ping"},
			keyHelp{"↑↓ / Tab", "switch the selected agent card"},
			keyHelp{"e", "toggle the Keeper engine"},
		)
	case tabSettings:
		bindings = append(bindings,
			keyHelp{"space / enter", "cycle or toggle the selected setting"},
			keyHelp{"J / K", "move a quota provider down / up"},
		)
	}
	return bindings
}

func (m *model) drawHelp(c *canvas, area rect) {
	theme := m.theme()
	bindings := m.helpBindings()
	keyWidth, descWidth := 10, 20
	for _, binding := range bindings {
		if len([]rune(binding.key)) > keyWidth {
			keyWidth = len([]rune(binding.key))
		}
		if len([]rune(binding.desc)) > descWidth {
			descWidth = len([]rune(binding.desc))
		}
	}
	width := min(keyWidth+descWidth+4, area.w-4)
	height := min(len(bindings)+4, area.h-2)
	box := popup(area, width, height)
	c.box(box.x, box.y, box.w, box.h, " Dashboard Keybindings ", boldStyle(theme.accent), fgStyle(theme.accent))
	inner := box.inner()
	y := inner.y
	for _, binding := range bindings {
		if y >= inner.bottom() {
			break
		}
		c.set(inner.x, y, padRight(" "+binding.key, keyWidth+2), fgStyle(theme.accent))
		c.set(inner.x+keyWidth+2, y, truncate(binding.desc, max(inner.w-keyWidth-2, 0)), fgStyle(theme.fg))
		y++
	}
	if y < inner.bottom() {
		y++
	}
	if y < inner.bottom() {
		c.set(inner.x, y, " press any key to close", fgStyle(theme.dim))
	}
}

func (m *model) drawFilter(c *canvas, area rect) {
	theme := m.theme()
	width := min(46, area.w-4)
	height := min(len(report.AllAgents)+6, area.h-2)
	box := popup(area, width, height)
	c.box(box.x, box.y, box.w, box.h, " Filter Sources ", boldStyle(theme.accent), fgStyle(theme.accent))
	inner := box.inner()
	if inner.h <= 0 {
		return
	}
	y := inner.y
	c.set(inner.x, y, truncate("Select data sources to display in dashboard:", inner.w), fgStyle(theme.fg))
	y += 2
	for index, agent := range report.AllAgents {
		if y >= inner.bottom() {
			break
		}
		marker := "  "
		if index == m.filterCursor {
			marker = "> "
		}
		checkbox := "[ ]"
		if m.agents[agent] {
			checkbox = "[x]"
		}
		c.set(inner.x, y, marker, fgStyle(theme.accent))
		color := theme.dim
		if m.agents[agent] {
			color = theme.gaugeLow
		}
		c.set(inner.x+2, y, checkbox, fgStyle(color))
		c.set(inner.x+6, y, displayAgentName(agent), fgStyle(theme.fg))
		y++
	}
	if y < inner.bottom() {
		y++
	}
	if y < inner.bottom() {
		c.set(inner.x, y, "space toggle  a all  s/Esc close", fgStyle(theme.dim))
	}
}

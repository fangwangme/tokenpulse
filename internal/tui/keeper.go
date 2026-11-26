package tui

import (
	"fmt"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/provider"
)

func (m *model) drawKeeper(c *canvas, area rect) {
	if area.h <= 0 || area.w <= 0 {
		return
	}
	headerHeight := min(3, area.h)
	cardsHeight := min(keeperCardsHeight(area.w), max(area.h-headerHeight, 0))
	logHeight := max(area.h-headerHeight-cardsHeight, 0)
	m.drawKeeperHeader(c, rect{x: area.x, y: area.y, w: area.w, h: headerHeight})
	m.drawKeeperCards(c, rect{x: area.x, y: area.y + headerHeight, w: area.w, h: cardsHeight})
	if logHeight > 0 {
		m.drawKeeperLog(c, rect{x: area.x, y: area.y + headerHeight + cardsHeight, w: area.w, h: logHeight})
	}
}

func keeperCardsHeight(width int) int {
	rows, _ := keeperGrid(width)
	if rows > 1 {
		return min(rows*10, 22)
	}
	return 11
}

func keeperGrid(width int) (int, int) {
	total := len(keeper.Agents)
	columns := max(min(width/36, total), 1)
	rows := (total + columns - 1) / columns
	return max(rows, 1), columns
}

func keeperAgentName(agent string) string {
	switch agent {
	case "claude":
		return "Claude Code"
	case "codex":
		return "OpenAI Codex"
	case "antigravity":
		return "Google Antigravity"
	default:
		return agent
	}
}

func (m *model) drawKeeperHeader(c *canvas, area rect) {
	theme := m.theme()
	if area.h < 1 {
		return
	}
	c.box(area.x, area.y, area.w, area.h, " Keeper Scheduled Activation Engine ", boldStyle(theme.accent), fgStyle(theme.border))
	inner := area.inner()
	if inner.h < 1 {
		return
	}
	y := inner.y
	x := inner.x
	c.set(x, y, "Master Switch: ", boldStyle(theme.fg))
	x += len("Master Switch: ")
	enabled := m.cfg.Keeper.Enabled
	status := " [DISABLED] "
	style := textStyle{fg: theme.fg, bg: theme.dim, bold: true}
	if enabled {
		status = " [ENABLED] "
		style = textStyle{fg: theme.onAccent, bg: theme.gaugeLow, bold: true}
	}
	c.set(x, y, status, style)
	x += len(status) + 3
	c.set(x, y, "Hint: ", boldStyle(theme.accentSoft))
	x += len("Hint: ")
	path := ""
	if m.opts.ConfigManager != nil {
		path = m.opts.ConfigManager.Path
	}
	c.set(x, y, truncate("To customize wakeup time/models/commands, edit "+path, max(inner.right()-x, 0)), fgStyle(theme.dim))
}

func (m *model) drawKeeperCards(c *canvas, area rect) {
	theme := m.theme()
	if area.h <= 0 || area.w <= 0 {
		return
	}
	rows, columns := keeperGrid(area.w)
	rowHeight := max(area.h/max(rows, 1), 1)
	defaults := config.DefaultKeeperAgents()
	now := time.Now()
	for row := 0; row < rows; row++ {
		y := area.y + row*rowHeight
		if y >= area.bottom() {
			break
		}
		height := min(rowHeight, area.bottom()-y)
		cells := splitH(rect{x: area.x, y: y, w: area.w, h: height}, columns)
		for column := 0; column < columns; column++ {
			index := row*columns + column
			if index >= len(keeper.Agents) {
				break
			}
			agent := keeper.Agents[index]
			agentCfg, ok := m.cfg.Keeper.Agents[agent]
			if !ok {
				agentCfg = defaults[agent]
			}
			selected := index == m.keeperCursor
			borderColor := theme.border
			titleStyle := boldStyle(theme.providerColor(agent))
			if selected {
				borderColor = theme.accent
				titleStyle = boldStyle(theme.accent)
			}
			card := cells[column]
			c.box(card.x, card.y, card.w, card.h, " "+keeperAgentName(agent)+" ", titleStyle, fgStyle(borderColor))
			inner := card.inner()
			if inner.h <= 0 {
				continue
			}
			lines := m.keeperCardLines(agent, agentCfg, selected, now)
			for lineIndex, line := range lines {
				if lineIndex >= inner.h {
					break
				}
				c.set(inner.x, inner.y+lineIndex, truncate(line, inner.w), fgStyle(theme.fg))
			}
			// Re-color the fixed prefixes so each row keeps the 0.6.16 palette.
			if inner.h >= 1 {
				c.set(inner.x, inner.y, keeperCheck(agentCfg.SessionKeeperEnabled), fgStyle(checkColor(theme, agentCfg.SessionKeeperEnabled)))
			}
			if inner.h >= 2 {
				c.set(inner.x, inner.y+1, keeperCheck(agentCfg.WeeklyKeeperEnabled), fgStyle(checkColor(theme, agentCfg.WeeklyKeeperEnabled)))
			}
		}
	}
}

func keeperCheck(enabled bool) string {
	if enabled {
		return "[✓]"
	}
	return "[ ]"
}

func checkColor(theme tuiTheme, enabled bool) lipgloss.Color {
	if enabled {
		return theme.gaugeLow
	}
	return theme.dim
}

func (m *model) keeperCardLines(agent string, agentCfg config.AgentKeeperConfig, selected bool, now time.Time) []string {
	nextDaily := "--:--"
	if trigger := keeper.NextDailyTrigger(agentCfg.DailyWakeupTime, nil, now); trigger != nil {
		nextDaily = trigger.Format("15:04")
	}
	nextWeekly := "Waiting Quota"
	if snapshot := m.keeperSnapshot(agent); snapshot != nil {
		for _, window := range snapshot.Windows {
			if window.PeriodMs != nil && *window.PeriodMs == 7*24*60*60*1000 && window.ResetsAt != nil {
				nextWeekly = window.ResetsAt.Local().Format("01-02 15:04")
				break
			}
		}
	}
	modelName := "—"
	for index := 0; index < len(m.keeperLog); index++ {
		if m.keeperLog[index].Agent == agent {
			if m.keeperLog[index].Model != "" {
				modelName = m.keeperLog[index].Model
			}
			break
		}
	}
	last := "Never triggered"
	for index := 0; index < len(m.keeperLog); index++ {
		record := m.keeperLog[index]
		if record.Agent != agent {
			continue
		}
		if record.Success {
			last = "✓ Success"
			if record.DurationMs > 0 {
				last += fmt.Sprintf(" (%dms)", record.DurationMs)
			}
		} else {
			last = "✗ Failed (" + truncate(record.OutputSnippet, 28) + ")"
		}
		break
	}
	lines := []string{
		keeperCheck(agentCfg.SessionKeeperEnabled) + " 5h Wakeup: " + agentCfg.DailyWakeupTime + " (next " + nextDaily + ")",
		keeperCheck(agentCfg.WeeklyKeeperEnabled) + " Weekly Sync: Auto Reset+1m",
		"   Next: " + nextWeekly,
		"Model: " + modelName,
		"Last: " + last,
		"",
	}
	if selected {
		lines = append(lines, "[1/d] 5h [2/w] Wk [p] Ping")
	} else {
		lines = append(lines, "Tab / ↑↓ to select")
	}
	return lines
}

func (m *model) keeperSnapshot(agent string) *provider.QuotaSnapshot {
	for index := range m.snapshots {
		if m.snapshots[index].Provider == agent {
			return &m.snapshots[index]
		}
	}
	return nil
}

func (m *model) drawKeeperLog(c *canvas, area rect) {
	theme := m.theme()
	c.box(area.x, area.y, area.w, area.h, " Live Execution & Heartbeat Stream (Newest First) ", boldStyle(theme.accent), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 {
		return
	}
	if len(m.keeperLog) == 0 {
		message := "  No keeper activations recorded yet. Press [p] on any agent to trigger an immediate test ping."
		if m.keeperLoading {
			message = "Loading keeper history…"
		}
		c.set(inner.x, inner.y, truncate(message, inner.w), fgStyle(theme.dim))
		return
	}
	y := inner.y
	for index := 0; index < len(m.keeperLog) && y < inner.bottom(); index++ {
		record := m.keeperLog[index]
		status := "✓ Success"
		if record.Success {
			if record.DurationMs > 0 || record.ExitCode != nil {
				status += fmt.Sprintf(" (%dms, exit %d)", record.DurationMs, exitCode(record))
			}
		} else {
			status = fmt.Sprintf("✗ Failed (%dms, exit %s)", record.DurationMs, exitCodeText(record))
		}
		header := fmt.Sprintf("[%s] [%s] ", record.Timestamp.Local().Format("2006-01-02 15:04:05"), keeperAgentName(record.Agent))
		if record.Trigger != "" {
			header += "(" + record.Trigger.Label() + ") "
		}
		c.set(inner.x, y, truncate(header+status, inner.w), fgStyle(theme.fg))
		y++
		if y >= inner.bottom() {
			break
		}
		if record.Model != "" {
			c.set(inner.x, y, truncate("  ├─ Model:  "+record.Model, inner.w), fgStyle(theme.dim))
			y++
			if y >= inner.bottom() {
				break
			}
		}
		if record.Prompt != "" {
			c.set(inner.x, y, truncate("  ├─ Prompt: \""+record.Prompt+"\"", inner.w), fgStyle(theme.dim))
			y++
			if y >= inner.bottom() {
				break
			}
		}
		if record.CommandExecuted != "" {
			c.set(inner.x, y, truncate("  ├─ Cmd:    "+record.CommandExecuted, inner.w), fgStyle(theme.dim))
			y++
			if y >= inner.bottom() {
				break
			}
		}
		prefix := "  └─ Reply:  "
		if !record.Success {
			prefix = "  └─ Error:  "
		}
		c.set(inner.x, y, truncate(prefix+record.OutputSnippet, inner.w), fgStyle(theme.fg))
		y += 2
	}
}

func exitCode(record keeper.ExecutionRecord) int {
	if record.ExitCode == nil {
		return 0
	}
	return *record.ExitCode
}

func exitCodeText(record keeper.ExecutionRecord) string {
	if record.ExitCode == nil {
		return "ERR"
	}
	return fmt.Sprintf("%d", *record.ExitCode)
}

func (m *model) moveKeeperCursor(direction int) {
	if len(keeper.Agents) == 0 {
		return
	}
	m.keeperCursor = (m.keeperCursor + direction + len(keeper.Agents)) % len(keeper.Agents)
}

func (m *model) handleKeeperKey(key string) (tea.Model, tea.Cmd) {
	agent := keeper.Agents[clampInt(m.keeperCursor, 0, len(keeper.Agents)-1)]
	agentCfg, ok := m.cfg.Keeper.Agents[agent]
	if !ok {
		agentCfg = config.DefaultKeeperAgents()[agent]
	}
	switch key {
	case "up", "k":
		m.moveKeeperCursor(-1)
	case "down", "j":
		m.moveKeeperCursor(1)
	case "1", "d":
		return m, m.patchSettings(map[string]any{
			"keeper.agents." + agent + ".session_keeper_enabled": !agentCfg.SessionKeeperEnabled,
		}, keeperAgentName(agent)+" 5h keeper")
	case "2", "w":
		return m, m.patchSettings(map[string]any{
			"keeper.agents." + agent + ".weekly_keeper_enabled": !agentCfg.WeeklyKeeperEnabled,
		}, keeperAgentName(agent)+" weekly keeper")
	case "e":
		return m, m.patchSettings(map[string]any{"keeper.enabled": !m.cfg.Keeper.Enabled}, "Keeper engine")
	case "p":
		client := m.client
		if client == nil {
			return m, nil
		}
		return m, func() tea.Msg {
			if err := client.KeeperPing(agent, ""); err != nil {
				return toastMsg{"Ping failed: " + err.Error()}
			}
			return toastMsg{"Ping sent to " + keeperAgentName(agent)}
		}
	}
	return m, nil
}

package tui

import (
	"fmt"
	"time"

	tea "github.com/charmbracelet/bubbletea"

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

func keeperAgentAt(area rect, col, row int) (int, bool) {
	if area.h <= 0 || area.w <= 0 {
		return 0, false
	}
	headerHeight := min(3, area.h)
	cardsHeight := min(keeperCardsHeight(area.w), max(area.h-headerHeight, 0))
	cards := rect{x: area.x, y: area.y + headerHeight, w: area.w, h: cardsHeight}
	if row < cards.y || row >= cards.bottom() || col < cards.x || col >= cards.right() {
		return 0, false
	}
	rows, columns := keeperGrid(cards.w)
	rowHeight := max(cards.h/max(rows, 1), 1)
	for r := 0; r < rows; r++ {
		y := cards.y + r*rowHeight
		height := min(rowHeight, cards.bottom()-y)
		if row < y || row >= y+height {
			continue
		}
		cells := splitH(rect{x: cards.x, y: y, w: cards.w, h: height}, columns)
		for cIdx, cell := range cells {
			if col >= cell.x && col < cell.right() {
				index := r*columns + cIdx
				if index < len(keeper.Agents) {
					return index, true
				}
			}
		}
	}
	return 0, false
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
	enabled := m.configValue().Keeper.Enabled
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
			agentCfg, ok := m.configValue().Keeper.Agents[agent]
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
			lines := m.keeperCardSpans(theme, agent, agentCfg, selected, now)
			for lineIndex, spans := range lines {
				if lineIndex >= inner.h {
					break
				}
				drawSpans(c, inner.x, inner.y+lineIndex, inner.right(), spans)
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

func checkStyle(theme tuiTheme, enabled bool) textStyle {
	if enabled {
		return boldStyle(theme.gaugeLow)
	}
	return fgStyle(theme.dim)
}

func (m *model) keeperCardSpans(theme tuiTheme, agent string, agentCfg config.AgentKeeperConfig, selected bool, now time.Time) [][]styledSpan {
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
	last := "Never triggered"
	lastColor := theme.dim
	for _, record := range m.keeperLog {
		if record.Agent != agent {
			continue
		}
		if record.Model != "" {
			modelName = record.Model
		}
		if record.Success {
			last = "✓ Success"
			if record.DurationMs > 0 {
				last += fmt.Sprintf(" (%dms)", record.DurationMs)
			}
			lastColor = theme.gaugeLow
		} else {
			last = "✗ Failed (" + truncate(record.OutputSnippet, 28) + ")"
			lastColor = theme.gaugeHigh
		}
		break
	}
	lines := [][]styledSpan{
		{
			{keeperCheck(agentCfg.SessionKeeperEnabled), checkStyle(theme, agentCfg.SessionKeeperEnabled)},
			{" 5h Wakeup: ", boldStyle(theme.fg)},
			{agentCfg.DailyWakeupTime, fgStyle(theme.accentSoft)},
			{" (next " + nextDaily + ")", fgStyle(theme.dim)},
		},
		{
			{keeperCheck(agentCfg.WeeklyKeeperEnabled), checkStyle(theme, agentCfg.WeeklyKeeperEnabled)},
			{" Weekly Sync: ", boldStyle(theme.fg)},
			{"Auto Reset+1m", fgStyle(theme.accentSoft)},
		},
		{
			{"   Next: " + nextWeekly, fgStyle(theme.dim)},
		},
		{
			{"Model: ", fgStyle(theme.dim)},
			{modelName, boldStyle(theme.fg)},
		},
		{
			{"Last: ", fgStyle(theme.dim)},
			{last, fgStyle(lastColor)},
		},
		nil,
	}
	if selected {
		lines = append(lines, []styledSpan{
			{"[1/d] ", boldStyle(theme.accentSoft)},
			{"5h ", fgStyle(theme.fg)},
			{"[2/w] ", boldStyle(theme.accentSoft)},
			{"Wk ", fgStyle(theme.fg)},
			{"[p] ", boldStyle(theme.accentSoft)},
			{"Ping", fgStyle(theme.fg)},
		})
	} else {
		lines = append(lines, []styledSpan{
			{"Tab / ↑↓ to select", fgStyle(theme.dim)},
		})
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
	title := " Live Execution & Heartbeat Stream (Newest First) "
	if m.keeperLogScroll > 0 {
		title = fmt.Sprintf(" Live Execution & Heartbeat Stream (Scroll: line %d) ", m.keeperLogScroll)
	}
	c.box(area.x, area.y, area.w, area.h, title, boldStyle(theme.accent), fgStyle(theme.border))
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

	var lines [][]styledSpan
	for _, record := range m.keeperLog {
		status := "✓ Success"
		statusColor := theme.gaugeLow
		if record.Success {
			if record.DurationMs > 0 || record.ExitCode != nil {
				status += fmt.Sprintf(" (%dms, exit %d)", record.DurationMs, exitCode(record))
			}
		} else {
			status = "✗ Failed"
			statusColor = theme.gaugeHigh
			if record.DurationMs > 0 || record.ExitCode != nil {
				status += fmt.Sprintf(" (%dms, exit %s)", record.DurationMs, exitCodeText(record))
			}
		}
		header := []styledSpan{
			{fmt.Sprintf("[%s] ", record.Timestamp.Local().Format("2006-01-02 15:04:05")), fgStyle(theme.dim)},
			{fmt.Sprintf("[%s] ", keeperAgentName(record.Agent)), boldStyle(theme.providerColor(record.Agent))},
		}
		if record.Trigger != "" {
			header = append(header, styledSpan{"(" + record.Trigger.Label() + ") ", fgStyle(theme.accentSoft)})
		}
		header = append(header, styledSpan{status, boldStyle(statusColor)})
		lines = append(lines, header)

		if record.Model != "" {
			lines = append(lines, []styledSpan{
				{"  ├─ Model:  ", fgStyle(theme.dim)},
				{record.Model, boldStyle(theme.fg)},
			})
		}
		if record.Prompt != "" {
			lines = append(lines, []styledSpan{
				{"  ├─ Prompt: ", fgStyle(theme.dim)},
				{"\"" + record.Prompt + "\"", fgStyle(theme.accentSoft)},
			})
		}
		if record.CommandExecuted != "" {
			lines = append(lines, []styledSpan{
				{"  ├─ Cmd:    ", fgStyle(theme.dim)},
				{record.CommandExecuted, fgStyle(theme.dim)},
			})
		}
		prefix := "  └─ Reply:  "
		replyColor := theme.fg
		if !record.Success {
			prefix = "  └─ Error:  "
			replyColor = theme.gaugeHigh
		}
		lines = append(lines, []styledSpan{
			{prefix, fgStyle(theme.dim)},
			{record.OutputSnippet, fgStyle(replyColor)},
		})
		lines = append(lines, nil)
	}

	start := clampInt(m.keeperLogScroll, 0, max(len(lines)-inner.h, 0))
	m.keeperLogScroll = start
	for i := 0; i < inner.h && start+i < len(lines); i++ {
		if len(lines[start+i]) == 0 {
			continue
		}
		drawSpans(c, inner.x, inner.y+i, inner.right(), lines[start+i])
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
	cfg := m.configValue()
	agent := keeper.Agents[clampInt(m.keeperCursor, 0, len(keeper.Agents)-1)]
	agentCfg, ok := cfg.Keeper.Agents[agent]
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
		return m, m.patchSettings(map[string]any{"keeper.enabled": !cfg.Keeper.Enabled}, "Keeper engine")
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

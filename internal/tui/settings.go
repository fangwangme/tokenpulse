package tui

import (
	"fmt"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/notification"
	"github.com/fangwangme/tokenpulse/internal/quota"
	"github.com/fangwangme/tokenpulse/internal/version"
)

type settingRow struct {
	key   string
	label string
	value string
	color lipgloss.Color
}

func (m *model) settingsRows() []settingRow {
	theme := m.theme()
	cfg := m.configValue()
	onOff := func(value bool) string {
		if value {
			return "true"
		}
		return "false"
	}
	auto := refreshLabel(cfg.Display.AutoRefreshSecs)
	if cfg.Display.AutoRefreshSecs > 0 && !m.refreshedAt.IsZero() {
		remaining := int(cfg.Display.AutoRefreshSecs) - int(time.Since(m.refreshedAt).Seconds())
		if remaining < 0 {
			remaining = 0
		}
		auto = fmt.Sprintf("%s (next %dm %ds)", auto, remaining/60, remaining%60)
	}
	themeLabel := string(cfg.Display.Theme)
	if cfg.Display.Theme == config.ThemeAuto {
		resolved := "dark"
		if m.theme().mode == 1 {
			resolved = "light"
		}
		themeLabel = "auto -> " + resolved
	}
	rows := []settingRow{
		{key: "quota_display_mode", value: string(cfg.Display.QuotaDisplayMode), color: theme.codex},
		{key: "show_empty_providers", value: onOff(cfg.Display.ShowEmptyProviders), color: theme.gemini},
		{key: "account_display", value: string(cfg.Display.AccountDisplay), color: theme.claude},
		{key: "auto_refresh_interval", value: auto, color: theme.accentSoft},
		{key: "theme", value: themeLabel, color: theme.antigravity},
		{key: "scan_antigravity", value: onOff(cfg.Display.ScanAntigravity), color: theme.antigravity},
		{key: "refresh_quota", value: onOff(cfg.Display.RefreshQuota), color: theme.accentSoft},
		{key: "notification_level", value: cfg.Display.NotificationLevel.DisplayLabel(), color: theme.gaugeLow},
		{key: "notification_sound", value: cfg.Display.NotificationSound, color: theme.gaugeLow},
		{key: "keeper_engine", value: enabledLabel(cfg.Keeper.Enabled), color: boolColor(theme, cfg.Keeper.Enabled)},
		{key: "keeper_antigravity_groups", value: cfg.Keeper.AntigravityGroupsLabel(), color: boolColor(theme, cfg.Keeper.AntigravityAllGroups())},
		{key: "web_remote", value: m.webRemoteLabel(), color: theme.accentSoft},
	}
	for _, id := range cfg.QuotaProviderOrder(quota.ProviderIDs) {
		rows = append(rows, settingRow{
			key:   "provider:" + id,
			label: id,
			value: onOff(cfg.Providers[id].Enabled),
			color: theme.providerColor(id),
		})
	}
	return rows
}

func enabledLabel(enabled bool) string {
	if enabled {
		return "enabled"
	}
	return "disabled"
}

func boolColor(theme tuiTheme, value bool) lipgloss.Color {
	if value {
		return theme.gaugeLow
	}
	return theme.dim
}

func (m *model) webRemoteLabel() string {
	switch {
	case m.webBusy:
		return "switching…"
	case m.rtStatus.ManagedBy == "systemd" || m.rtStatus.ManagedBy == "launchd":
		return fmt.Sprintf("managed by %s (%s)", m.rtStatus.ManagedBy, m.rtStatus.WebURL)
	case m.client != nil && m.client.IsRemote() && m.rtStatus.ManagedBy == "standalone":
		return "attached to local TUI"
	case m.client != nil && m.client.IsRemote():
		return fmt.Sprintf("ON (%s)  (enter stops)", m.rtStatus.WebURL)
	default:
		return "OFF (enter starts background serve)"
	}
}

func refreshLabel(seconds uint32) string {
	switch seconds {
	case 0:
		return "off"
	case 60:
		return "1m"
	case 120:
		return "2m"
	case 300:
		return "5m"
	case 600:
		return "10m"
	case 900:
		return "15m"
	default:
		return "custom"
	}
}

func (m *model) drawSettings(c *canvas, area rect) {
	theme := m.theme()
	c.box(area.x, area.y, area.w, area.h, " Settings ", boldStyle(theme.accent), fgStyle(theme.border))
	inner := area.inner()
	if inner.h <= 0 || inner.w <= 0 {
		return
	}
	configPath := ""
	if m.opts.ConfigManager != nil {
		configPath = m.opts.ConfigManager.Path
	}
	lines := []string{
		"Config file " + configPath,
		"Version " + version.String(),
		"",
	}
	rows := m.settingsRows()
	selectedLine := 0
	providerStart := -1
	for index, row := range rows {
		if strings.HasPrefix(row.key, "provider:") && providerStart < 0 {
			providerStart = index
			lines = append(lines, "Quota providers  (usage sources are filtered separately)")
		}
		if index == m.settingsCursor {
			selectedLine = len(lines)
		}
		marker := " "
		if index == m.settingsCursor {
			marker = ">"
		}
		if strings.HasPrefix(row.key, "provider:") {
			checkbox := "[ ]"
			if row.value == "true" {
				checkbox = "[x]"
			}
			lines = append(lines, fmt.Sprintf("%s %s %s", marker, checkbox, row.label))
		} else {
			lines = append(lines, fmt.Sprintf("%s %s = %s", marker, settingName(row.key), row.value))
		}
	}
	lines = append(lines, "", "space / enter cycle / toggle selected setting", "J / K move a provider down / up")
	offset := clampInt(selectedLine-inner.h+1, 0, max(len(lines)-inner.h, 0))
	for index := 0; index < inner.h && offset+index < len(lines); index++ {
		c.set(inner.x, inner.y+index, truncate(lines[offset+index], inner.w), fgStyle(theme.fg))
	}
	// Re-color values: walk the visible rows and replace the value span.
	for index := range rows {
		lineIndex := index
		if providerStart >= 0 && index >= providerStart {
			lineIndex = index + 1
		}
		lineIndex += 3
		if lineIndex < offset || lineIndex >= offset+inner.h {
			continue
		}
		row := rows[index]
		y := inner.y + lineIndex - offset
		marker := " "
		if index == m.settingsCursor {
			marker = ">"
		}
		if strings.HasPrefix(row.key, "provider:") {
			checkbox := "[ ]"
			if row.value == "true" {
				checkbox = "[x]"
			}
			c.setClip(inner.x, y, inner.right(), fmt.Sprintf("%s %s ", marker, checkbox), fgStyle(boolColor(theme, row.value == "true")))
			c.setClip(inner.x+6, y, inner.right(), row.label, boldStyle(row.color))
		} else {
			name := settingName(row.key)
			c.setClip(inner.x, y, inner.right(), marker, fgStyle(theme.accent))
			c.setClip(inner.x+2, y, inner.right(), name, boldStyle(theme.fg))
			c.setClip(inner.x+2+len([]rune(name))+3, y, inner.right(), row.value, fgStyle(row.color))
		}
	}
}

func settingName(key string) string {
	if key == "web_remote" {
		return "Web remote"
	}
	return key
}

func (m *model) handleSettingsKey(key string) (tea.Model, tea.Cmd) {
	rows := m.settingsRows()
	switch key {
	case "up", "k":
		if len(rows) > 0 {
			m.settingsCursor = (m.settingsCursor - 1 + len(rows)) % len(rows)
		}
	case "down", "j":
		if len(rows) > 0 {
			m.settingsCursor = (m.settingsCursor + 1) % len(rows)
		}
	case "enter", " ":
		return m, m.changeSetting()
	case "J", "K":
		return m, m.moveProvider(key == "J")
	}
	return m, nil
}

func (m *model) changeSetting() tea.Cmd {
	rows := m.settingsRows()
	if len(rows) == 0 {
		return nil
	}
	cfg := m.configValue()
	row := rows[clampInt(m.settingsCursor, 0, len(rows)-1)]
	switch row.key {
	case "quota_display_mode":
		next := config.QuotaUsed
		if cfg.Display.QuotaDisplayMode == config.QuotaUsed {
			next = config.QuotaRemaining
		}
		return m.patchSettings(map[string]any{"display.quota_display_mode": string(next)}, "Quota display")
	case "show_empty_providers":
		return m.patchSettings(map[string]any{"display.show_empty_providers": !cfg.Display.ShowEmptyProviders}, "Show empty providers")
	case "account_display":
		return m.patchSettings(map[string]any{"display.account_display": string(cfg.Display.AccountDisplay.Next())}, "Account display")
	case "auto_refresh_interval":
		return m.patchSettings(map[string]any{"display.auto_refresh_secs": int(m.nextRefreshInterval())}, "Auto refresh")
	case "theme":
		m.themeOverride = -1
		return m.patchSettings(map[string]any{"display.theme": string(cfg.Display.Theme.Next())}, "Theme")
	case "scan_antigravity":
		return m.patchSettings(map[string]any{"display.scan_antigravity": !cfg.Display.ScanAntigravity}, "Scan Antigravity")
	case "refresh_quota":
		return m.patchSettings(map[string]any{"display.refresh_quota": !cfg.Display.RefreshQuota}, "Refresh quota")
	case "notification_level":
		return m.patchSettings(map[string]any{"display.notification_level": string(cfg.Display.NotificationLevel.Next())}, "Notification level")
	case "notification_sound":
		next := nextNotificationSound(cfg.Display.NotificationSound)
		notification.PlayAlertSound(next)
		return m.patchSettings(map[string]any{"display.notification_sound": next}, "Notification sound")
	case "keeper_engine":
		return m.patchSettings(map[string]any{"keeper.enabled": !cfg.Keeper.Enabled}, "Keeper engine")
	case "keeper_antigravity_groups":
		return m.patchSettings(map[string]any{"keeper.agents.agy.weekly_all_groups": !cfg.Keeper.AntigravityAllGroups()}, "Antigravity groups")
	case "web_remote":
		return m.toggleWebRemote()
	}
	if strings.HasPrefix(row.key, "provider:") {
		id := strings.TrimPrefix(row.key, "provider:")
		return m.patchSettings(map[string]any{"providers." + id + ".enabled": !cfg.Providers[id].Enabled}, "Provider "+id)
	}
	return nil
}

func (m *model) nextRefreshInterval() uint32 {
	options := []uint32{0, 60, 120, 300, 600, 900}
	if m.client != nil && m.client.IsRemote() {
		options = []uint32{60, 120, 300, 600, 900}
	}
	current := m.configValue().Display.AutoRefreshSecs
	for index, value := range options {
		if value == current {
			return options[(index+1)%len(options)]
		}
	}
	return options[0]
}

func nextNotificationSound(current string) string {
	sounds := []string{"chime", "Hero", "Glass", "Submarine", "none"}
	position := 0
	for index, sound := range sounds {
		if strings.EqualFold(sound, current) {
			position = (index + 1) % len(sounds)
			break
		}
	}
	return sounds[position]
}

func (m *model) moveProvider(down bool) tea.Cmd {
	rows := m.settingsRows()
	if len(rows) == 0 {
		return nil
	}
	row := rows[clampInt(m.settingsCursor, 0, len(rows)-1)]
	if !strings.HasPrefix(row.key, "provider:") {
		return nil
	}
	id := strings.TrimPrefix(row.key, "provider:")
	order := m.configValue().QuotaProviderOrder(quota.ProviderIDs)
	index := -1
	for i, candidate := range order {
		if candidate == id {
			index = i
			break
		}
	}
	if index < 0 {
		return nil
	}
	target := index - 1
	if down {
		target = index + 1
	}
	if target < 0 || target >= len(order) {
		return nil
	}
	order[index], order[target] = order[target], order[index]
	m.settingsCursor = len(rows) - len(order) + target
	return m.patchSettings(map[string]any{"display.quota_provider_order": order}, "Provider order")
}

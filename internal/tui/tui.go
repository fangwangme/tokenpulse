package tui

import (
	"fmt"
	"sort"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/notification"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/quota"
	"github.com/fangwangme/tokenpulse/internal/remote"
	"github.com/fangwangme/tokenpulse/internal/report"
	"github.com/fangwangme/tokenpulse/internal/scan"
	"github.com/fangwangme/tokenpulse/internal/version"
)

type Options struct {
	DataDir        string
	Scanner        *scan.Scanner
	Config         config.Config
	ConfigManager  *config.Manager
	Since          *time.Time
	RefreshPricing bool
}

const (
	tabUsage = iota
	tabQuota
	tabKeeper
	tabSettings
	tabCount
)

var tabNames = []string{"Usage", "Quota", "Keeper", "Settings"}

type detailKind int

const (
	detailNone detailKind = iota
	detailDay
	detailModel
)

type model struct {
	opts    Options
	width   int
	height  int
	tab     int
	theme   int
	toast   string
	toastAt time.Time
	status  string

	preset    string
	from      string
	to        string
	agents    map[string]bool
	metric    int
	listMode  int
	focus     int
	report    report.Report
	reportErr string

	selectedRow int
	selectedDay string

	detail       any
	detailKind   detailKind
	showFilter   bool
	filterCursor int

	snapshots   []provider.QuotaSnapshot
	quotaErrors map[string]string
	quotaCursor int
	exhausted   map[string]bool

	keeperCursor int
	keeperLog    []keeper.ExecutionRecord
	engine       *keeper.Engine

	settingsCursor int

	serveMode   bool
	client      *remote.LocalClient
	state       *remote.StateInfo
	events      chan string
	lastScanned int64
	lastFetched int64
	loading     bool
	autoTick    int
}

type stateMsg struct {
	state *remote.StateInfo
	err   error
}
type scanMsg struct {
	err error
}
type quotaMsg struct {
	results []fetchResult
}
type fetchResult struct {
	Provider string
	Snapshot *provider.QuotaSnapshot
	Err      string
}
type toastMsg struct{ text string }
type wsMsg struct{ payload string }
type tickMsg struct{}
type keeperMsg struct{ records []keeper.ExecutionRecord }

func Run(opts Options) error {
	m := newModel(opts)
	program := tea.NewProgram(m, tea.WithAltScreen(), tea.WithMouseCellMotion())
	_, err := program.Run()
	return err
}

func newModel(opts Options) *model {
	cfg := opts.Config
	agents := map[string]bool{}
	for _, agent := range report.AllAgents {
		agents[agent] = true
	}
	if !cfg.Display.ScanAntigravity {
		agents["antigravity"] = false
	}
	m := &model{
		opts: opts, agents: agents, theme: 1, preset: "today", listMode: 0, focus: 1,
		exhausted: map[string]bool{}, quotaErrors: map[string]string{},
		events: make(chan string, 8),
	}
	if opts.Since != nil {
		m.preset = "custom"
		m.from = opts.Since.Format("2006-01-02")
		m.to = time.Now().Format("2006-01-02")
	}
	m.engine = keeper.NewEngine(cfg.Keeper, opts.ConfigManager.KeeperStatePath())
	m.keeperLog = m.engine.History.Recent(20)
	m.loadReport()
	m.loadCachedQuota()
	return m
}

type clientQuota struct {
	Providers []struct {
		Provider string                  `json:"provider"`
		Enabled  bool                    `json:"enabled"`
		Snapshot *provider.QuotaSnapshot `json:"snapshot"`
		Error    *struct {
			Message string    `json:"message"`
			At      time.Time `json:"at"`
		} `json:"error"`
	} `json:"providers"`
}

func (m *model) loadCachedQuota() {
	for _, snapshot := range cachedQuotas(m.opts.DataDir, m.opts.Config) {
		m.upsertSnapshot(snapshot)
	}
}

func (m *model) loadReport() {
	m.reportErr = ""
	from, to, err := m.resolveRange()
	if err != nil {
		m.reportErr = err.Error()
		return
	}
	value, err := m.opts.Scanner.Report.Report(report.Params{
		Preset: m.preset, From: from, To: to, Agents: m.selectedAgents(),
	})
	if err != nil {
		m.reportErr = err.Error()
		return
	}
	m.report = value
	m.selectedRow = 0
}

func (m *model) resolveRange() (string, string, error) {
	switch m.preset {
	case "custom":
		from, to := m.from, m.to
		if from == "" {
			from = time.Now().Format("2006-01-02")
		}
		if to == "" {
			to = time.Now().Format("2006-01-02")
		}
		return from, to, nil
	default:
		return "", "", nil
	}
}

func (m *model) selectedAgents() []string {
	var agents []string
	for _, agent := range report.AllAgents {
		if m.agents[agent] {
			agents = append(agents, agent)
		}
	}
	return agents
}

func (m *model) Init() tea.Cmd {
	if m.detectServe() {
		return tea.Batch(m.pollState(), m.pollQuota(), m.readWS())
	}
	return tea.Batch(m.scanCmd(), m.quotaCmd(), m.tickCmd(), m.keeperTickCmd())
}

func (m *model) detectServe() bool {
	client := remote.NewLocalClient(m.opts.DataDir)
	state, err := client.State()
	if err != nil {
		m.serveMode = false
		m.client = nil
		return false
	}
	m.serveMode = true
	m.client = client
	m.state = state
	m.lastScanned = state.ScannedAt
	m.lastFetched = state.FetchedAt
	m.startEventReader()
	return true
}

func (m *model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		return m, nil
	case tea.KeyMsg:
		return m.handleKey(msg)
	case stateMsg:
		if msg.err != nil {
			if m.serveMode {
				m.serveMode = false
				m.client = nil
				return m, tea.Batch(m.scanCmd(), m.quotaCmd(), m.tickCmd(), m.keeperTickCmd())
			}
			return m, nil
		}
		if !m.serveMode {
			m.detectServe()
		}
		if msg.state != nil {
			m.state = msg.state
			if msg.state.ScannedAt != m.lastScanned {
				m.lastScanned = msg.state.ScannedAt
				m.loadReport()
			}
			if msg.state.FetchedAt != m.lastFetched {
				m.lastFetched = msg.state.FetchedAt
				return m, m.pollQuota()
			}
		}
		return m, m.pollState()
	case scanMsg:
		m.loading = false
		if msg.err != nil {
			m.status = "Refresh failed: " + msg.err.Error()
		} else {
			m.loadReport()
			m.status = "Refresh complete"
		}
		return m, nil
	case quotaMsg:
		for _, result := range msg.results {
			if result.Err != "" {
				m.quotaErrors[result.Provider] = result.Err
			} else {
				delete(m.quotaErrors, result.Provider)
			}
			if result.Snapshot != nil {
				m.upsertSnapshot(*result.Snapshot)
			}
		}
		return m, nil
	case tickMsg:
		if m.serveMode {
			return m, tea.Tick(2*time.Second, func(time.Time) tea.Msg { return tickMsg{} })
		}
		cfg := m.opts.Config
		m.detectServe()
		if m.serveMode {
			return m, m.pollState()
		}
		interval := time.Duration(cfg.Display.AutoRefreshSecs) * time.Second
		if interval <= 0 {
			interval = 5 * time.Second
		}
		var cmds []tea.Cmd
		if cfg.Display.AutoRefreshSecs > 0 {
			m.loading = true
			cmds = append(cmds, m.scanCmd(), m.quotaCmd())
		}
		cmds = append(cmds, tea.Tick(interval, func(time.Time) tea.Msg { return tickMsg{} }))
		return m, tea.Batch(cmds...)
	case keeperMsg:
		return m, tea.Batch(m.tickCmd(), m.keeperTickCmd())
	case toastMsg:
		m.toast = msg.text
		m.toastAt = time.Now()
		return m, nil
	case wsMsg:
		cmd := m.handleEvent(msg.payload)
		return m, tea.Batch(cmd, m.readWS())
	}
	return m, nil
}

func (m *model) handleEvent(payload string) tea.Cmd {
	switch {
	case strings.Contains(payload, "usage_updated"):
		m.loadReport()
		return nil
	case strings.Contains(payload, "quota_updated"):
		return m.pollQuota()
	case strings.Contains(payload, "quota_recovered"):
		m.toast = "A quota window was restored"
		m.toastAt = time.Now()
		m.loadReport()
	case strings.Contains(payload, "settings_updated"):
		if cfg, err := m.opts.ConfigManager.Load(); err == nil {
			m.opts.Config = cfg
		}
	}
	return nil
}

func (m *model) handleKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	if m.showFilter {
		return m.handleFilterKey(msg)
	}
	key := msg.String()
	switch key {
	case "ctrl+c", "q":
		return m, tea.Quit
	case "tab":
		m.tab = (m.tab + 1) % tabCount
		return m, nil
	case "shift+tab":
		m.tab = (m.tab + tabCount - 1) % tabCount
		return m, nil
	case "b":
		m.theme = (m.theme + 1) % 3
		return m, nil
	case "r":
		return m, m.refresh()
	case "esc":
		if m.detailKind != detailNone {
			m.detailKind = detailNone
			m.detail = nil
		}
		return m, nil
	}
	switch m.tab {
	case tabUsage:
		return m.handleUsageKey(key)
	case tabQuota:
		return m.handleQuotaKey(key)
	case tabKeeper:
		return m.handleKeeperKey(key)
	case tabSettings:
		return m.handleSettingsKey(key)
	}
	return m, nil
}

func (m *model) handleUsageKey(key string) (tea.Model, tea.Cmd) {
	presets := []string{"today", "7d", "30d", "year", "all", "custom"}
	if len(key) == 1 && key[0] >= '1' && key[0] <= '6' {
		m.preset = presets[int(key[0]-'1')]
		if m.preset == "custom" {
			m.showFilter = true
			m.filterCursor = 0
		}
		m.loadReport()
		return m, nil
	}
	switch key {
	case "m":
		m.metric = 1 - m.metric
	case "v":
		if m.report.Range.From != m.report.Range.To {
			m.listMode = 1 - m.listMode
			m.selectedRow = 0
		}
	case "n":
		m.preset = "today"
		m.loadReport()
	case "[":
		m.shiftRange(-1)
	case "]":
		m.shiftRange(1)
	case "f":
		if m.report.Range.From != m.report.Range.To {
			m.focus = 1 - m.focus
		}
	case "down", "j":
		m.moveSelection(1)
	case "up", "k":
		m.moveSelection(-1)
	case "left", "h":
		return m, m.moveDay(-1)
	case "right", "l":
		return m, m.moveDay(1)
	case "enter":
		m.openDetail()
	case "s":
		m.showFilter = true
		m.filterCursor = 0
	}
	return m, nil
}

func (m *model) moveSelection(delta int) {
	rows := len(m.report.Days)
	if len(m.report.Days) == 0 || (m.report.Range.From == m.report.Range.To) {
		rows = len(m.report.Models)
	} else if m.listMode == 1 {
		rows = len(m.report.Models)
	}
	m.selectedRow += delta
	if m.selectedRow < 0 {
		m.selectedRow = 0
	}
	if rows > 0 && m.selectedRow >= rows {
		m.selectedRow = rows - 1
	}
}

func (m *model) moveDay(delta int) tea.Cmd {
	if m.report.Range.From == m.report.Range.To || m.report.Layout != "week-columns" && m.report.Layout != "week-rows" {
		return nil
	}
	current := m.report.Range.To
	if m.selectedDay != "" {
		current = m.selectedDay
	}
	date, err := time.ParseInLocation("2006-01-02", current, time.Local)
	if err != nil {
		return nil
	}
	date = date.AddDate(0, 0, delta)
	if date.Format("2006-01-02") < m.report.Range.From {
		date, _ = time.ParseInLocation("2006-01-02", m.report.Range.From, time.Local)
	}
	if date.Format("2006-01-02") > m.report.Range.To {
		date, _ = time.ParseInLocation("2006-01-02", m.report.Range.To, time.Local)
	}
	m.selectedDay = date.Format("2006-01-02")
	return nil
}

func (m *model) shiftRange(direction int) {
	from, to, err := m.resolveRange()
	if err != nil || m.report.Range.From == "" {
		return
	}
	fromDate, err1 := time.ParseInLocation("2006-01-02", m.report.Range.From, time.Local)
	toDate, err2 := time.ParseInLocation("2006-01-02", m.report.Range.To, time.Local)
	if err1 != nil || err2 != nil {
		return
	}
	if from == "" && to == "" {
		fromDate, _ = time.ParseInLocation("2006-01-02", m.report.Range.From, time.Local)
		toDate, _ = time.ParseInLocation("2006-01-02", m.report.Range.To, time.Local)
	}
	length := int(toDate.Sub(fromDate).Hours()/24) + 1
	shift := direction * length
	today := time.Now().Format("2006-01-02")
	newFrom := fromDate.AddDate(0, 0, shift)
	newTo := toDate.AddDate(0, 0, shift)
	if newTo.Format("2006-01-02") > today {
		offset := int(toDate.AddDate(0, 0, shift).Sub(mustDate(today)).Hours() / 24)
		newTo = mustDate(today)
		newFrom = newFrom.AddDate(0, 0, -offset)
	}
	first := today
	if m.report.FirstDay != nil {
		first = *m.report.FirstDay
	}
	if newFrom.Format("2006-01-02") < first {
		newFrom = mustDate(first)
		newTo = newFrom.AddDate(0, 0, length-1)
	}
	m.preset = "custom"
	m.from = newFrom.Format("2006-01-02")
	m.to = newTo.Format("2006-01-02")
	m.loadReport()
}

func mustDate(value string) time.Time {
	date, _ := time.ParseInLocation("2006-01-02", value, time.Local)
	return date
}

func (m *model) openDetail() {
	if len(m.report.Days) > 0 && m.report.Range.From != m.report.Range.To && m.listMode == 0 {
		day := m.report.Days[m.selectedRow]
		detail, err := m.opts.Scanner.Report.Day(day.Date, m.selectedAgents())
		if err == nil {
			m.detail, m.detailKind = detail, detailDay
		}
		return
	}
	if len(m.report.Models) > 0 {
		index := m.selectedRow
		if index >= len(m.report.Models) {
			index = len(m.report.Models) - 1
		}
		model := m.report.Models[index]
		from, to := m.report.Range.From, m.report.Range.To
		detail, err := m.opts.Scanner.Report.Model(model.Model, from, to, m.selectedAgents())
		if err == nil {
			m.detail, m.detailKind = detail, detailModel
		}
	}
}

func (m *model) handleFilterKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	key := msg.String()
	switch key {
	case "esc", "s":
		m.showFilter = false
		if m.preset == "custom" && m.from == "" {
			m.preset = "today"
		}
	case "a":
		for _, agent := range report.AllAgents {
			m.agents[agent] = true
		}
		m.loadReport()
		m.showFilter = false
	case "up", "k":
		if m.filterCursor > 0 {
			m.filterCursor--
		}
	case "down", "j":
		if m.filterCursor < len(report.AllAgents)-1 {
			m.filterCursor++
		}
	case "enter", " ":
		agent := report.AllAgents[m.filterCursor]
		if m.agents[agent] {
			count := 0
			for _, selected := range m.agents {
				if selected {
					count++
				}
			}
			if count <= 1 {
				m.status = "At least one agent stays selected"
				return m, nil
			}
			m.agents[agent] = false
		} else {
			m.agents[agent] = true
		}
		m.loadReport()
	}
	return m, nil
}

func (m *model) handleQuotaKey(key string) (tea.Model, tea.Cmd) {
	switch key {
	case "up", "k":
		if m.quotaCursor > 0 {
			m.quotaCursor--
		}
	case "down", "j":
		if m.quotaCursor < len(m.snapshots)-1 {
			m.quotaCursor++
		}
	}
	return m, nil
}

func (m *model) handleKeeperKey(key string) (tea.Model, tea.Cmd) {
	cfg := m.opts.Config
	rows := len(keeper.Agents)
	switch key {
	case "up", "k":
		if m.keeperCursor > 0 {
			m.keeperCursor--
		}
	case "down", "j":
		if m.keeperCursor < rows-1 {
			m.keeperCursor++
		}
	case "e":
		cfg.Keeper.Enabled = !cfg.Keeper.Enabled
		m.saveConfig(cfg)
		if m.serveMode {
			return m, m.patchSettings(map[string]any{"keeper.enabled": cfg.Keeper.Enabled})
		}
	case "enter", " ":
		agent := keeper.Agents[m.keeperCursor]
		if m.serveMode {
			var entry config.AgentKeeperConfig
			key := "keeper.agents." + agent + ".session_keeper_enabled"
			if cfg.Keeper.Agents[agent].SessionKeeperEnabled {
				entry = cfg.Keeper.Agents[agent]
				entry.SessionKeeperEnabled = false
				cfg.Keeper.Agents[agent] = entry
				m.saveConfig(cfg)
				return m, m.patchSettings(map[string]any{key: false})
			}
			entry = cfg.Keeper.Agents[agent]
			entry.SessionKeeperEnabled = true
			cfg.Keeper.Agents[agent] = entry
			m.saveConfig(cfg)
			return m, m.patchSettings(map[string]any{key: true})
		}
		state, err := m.opts.ConfigManager.ToggleAgentSessionKeeper(agent)
		if err != nil {
			m.status = err.Error()
			return m, nil
		}
		entry := cfg.Keeper.Agents[agent]
		entry.SessionKeeperEnabled = state
		cfg.Keeper.Agents[agent] = entry
		m.opts.Config = cfg
	case "w":
		agent := keeper.Agents[m.keeperCursor]
		state, err := m.opts.ConfigManager.ToggleAgentWeeklyKeeper(agent)
		if err != nil {
			m.status = err.Error()
			return m, nil
		}
		entry := cfg.Keeper.Agents[agent]
		entry.WeeklyKeeperEnabled = state
		cfg.Keeper.Agents[agent] = entry
		m.opts.Config = cfg
		if m.serveMode {
			return m, m.patchSettings(map[string]any{"keeper.agents." + agent + ".weekly_keeper_enabled": state})
		}
	case "p":
		agent := keeper.Agents[m.keeperCursor]
		if m.serveMode {
			if err := m.client.Post("/api/keeper/ping", map[string]any{"agent": agent}, nil); err != nil {
				m.status = err.Error()
			}
			return m, nil
		}
		request, err := m.engine.Manual(agent, "")
		if err != nil {
			m.status = err.Error()
			return m, nil
		}
		go func() {
			record := m.engine.Execute(request)
			m.engine.Finish(request, record)
		}()
	}
	return m, nil
}

func (m *model) handleSettingsKey(key string) (tea.Model, tea.Cmd) {
	const rows = 12
	switch key {
	case "up", "k":
		if m.settingsCursor > 0 {
			m.settingsCursor--
		}
	case "down", "j":
		if m.settingsCursor < rows-1 {
			m.settingsCursor++
		}
	case "enter", " ":
		return m, m.changeSetting()
	}
	return m, nil
}

func (m *model) changeSetting() tea.Cmd {
	cfg := m.opts.Config
	var update map[string]any
	switch m.settingsCursor {
	case 0:
		cfg.Display.Theme = cfg.Display.Theme.Next()
	case 1:
		if cfg.Display.QuotaDisplayMode == config.QuotaUsed {
			cfg.Display.QuotaDisplayMode = config.QuotaRemaining
		} else {
			cfg.Display.QuotaDisplayMode = config.QuotaUsed
		}
		update = map[string]any{"display.quota_display_mode": string(cfg.Display.QuotaDisplayMode)}
	case 2:
		options := []uint32{0, 60, 120, 300, 600, 900}
		next := uint32(300)
		for i, value := range options {
			if value == cfg.Display.AutoRefreshSecs {
				next = options[(i+1)%len(options)]
				break
			}
		}
		cfg.Display.AutoRefreshSecs = next
		update = map[string]any{"display.auto_refresh_secs": int(next)}
	case 3:
		cfg.Display.AccountDisplay = cfg.Display.AccountDisplay.Next()
		update = map[string]any{"display.account_display": string(cfg.Display.AccountDisplay)}
	case 4:
		cfg.Display.ShowEmptyProviders = !cfg.Display.ShowEmptyProviders
		update = map[string]any{"display.show_empty_providers": cfg.Display.ShowEmptyProviders}
	case 5:
		cfg.Display.RefreshQuota = !cfg.Display.RefreshQuota
		update = map[string]any{"display.refresh_quota": cfg.Display.RefreshQuota}
	case 6:
		cfg.Display.ScanAntigravity = !cfg.Display.ScanAntigravity
		update = map[string]any{"display.scan_antigravity": cfg.Display.ScanAntigravity}
	case 7:
		cfg.Display.NotificationLevel = cfg.Display.NotificationLevel.Next()
		update = map[string]any{"display.notification_level": string(cfg.Display.NotificationLevel)}
	case 8:
		sounds := []string{"chime", "Hero", "Glass", "Submarine", "none"}
		next := "chime"
		for i, sound := range sounds {
			if strings.EqualFold(sound, cfg.Display.NotificationSound) {
				next = sounds[(i+1)%len(sounds)]
				break
			}
		}
		cfg.Display.NotificationSound = next
		update = map[string]any{"display.notification_sound": next}
		notification.PlayAlertSound(next)
	case 9:
		cfg.Keeper.Enabled = !cfg.Keeper.Enabled
		update = map[string]any{"keeper.enabled": cfg.Keeper.Enabled}
	case 10:
		cfg.Keeper.SetAntigravityAllGroups(!cfg.Keeper.AntigravityAllGroups())
		update = map[string]any{"keeper.agents.antigravity.weekly_all_groups": cfg.Keeper.AntigravityAllGroups()}
	case 11:
		for _, id := range []string{"claude", "codex", "antigravity"} {
			provider := cfg.Providers[id]
			provider.Enabled = !provider.Enabled
			if provider.Enabled {
				update = map[string]any{"providers." + id + ".enabled": true}
				cfg.Providers[id] = provider
				break
			}
			cfg.Providers[id] = provider
			update = map[string]any{"providers." + id + ".enabled": false}
			break
		}
	}
	m.saveConfig(cfg)
	if m.serveMode && update != nil {
		return m.patchSettings(update)
	}
	return nil
}

func (m *model) saveConfig(cfg config.Config) {
	if err := m.opts.ConfigManager.Save(&cfg); err != nil {
		m.status = err.Error()
		return
	}
	m.opts.Config = cfg
}

func (m *model) patchSettings(update map[string]any) tea.Cmd {
	if m.client == nil {
		return nil
	}
	normalized := make(map[string]any, len(update))
	for key, value := range update {
		switch {
		case strings.HasPrefix(key, "providers.antigravity."):
			normalized[strings.Replace(key, "providers.antigravity.", "providers.agy.", 1)] = value
		case strings.HasPrefix(key, "keeper.agents.antigravity."):
			normalized[strings.Replace(key, "keeper.agents.antigravity.", "keeper.agents.agy.", 1)] = value
		default:
			normalized[key] = value
		}
	}
	update = normalized
	client := m.client
	return func() tea.Msg {
		if err := client.Patch("/api/settings", update, nil); err != nil {
			return toastMsg{"Settings failed: " + err.Error()}
		}
		return toastMsg{"Settings applied"}
	}
}

func (m *model) refresh() tea.Cmd {
	if m.serveMode && m.client != nil {
		client := m.client
		return func() tea.Msg {
			if err := client.Post("/api/refresh", map[string]any{"usage": true, "quota": true}, nil); err != nil {
				return toastMsg{err.Error()}
			}
			return toastMsg{"Refresh requested"}
		}
	}
	m.loading = true
	return tea.Batch(m.scanCmd(), m.quotaCmd())
}

// ── commands ──

func (m *model) scanCmd() tea.Cmd {
	scanner := m.opts.Scanner
	cfg := m.opts.Config
	return func() tea.Msg {
		err := scanner.Run(scan.Options{
			Providers: scan.ProvidersFromConfig(cfg), ScanAntigravity: cfg.Display.ScanAntigravity,
			RefreshPricing: m.opts.RefreshPricing,
		})
		return scanMsg{err}
	}
}

func (m *model) quotaCmd() tea.Cmd {
	dataDir := m.opts.DataDir
	cfg := m.opts.Config
	return func() tea.Msg {
		if !cfg.Display.RefreshQuota {
			return quotaMsg{}
		}
		results := fetchQuotas(dataDir, cfg)
		return quotaMsg{results}
	}
}

func (m *model) tickCmd() tea.Cmd {
	interval := time.Duration(m.opts.Config.Display.AutoRefreshSecs) * time.Second
	if interval <= 0 {
		interval = 5 * time.Second
	}
	return tea.Tick(interval, func(time.Time) tea.Msg { return tickMsg{} })
}

func (m *model) keeperTickCmd() tea.Cmd {
	interval := time.Duration(m.opts.Config.Keeper.CheckIntervalSecs) * time.Second
	if interval < 10*time.Second {
		interval = 10 * time.Second
	}
	return tea.Tick(interval, func(time.Time) tea.Msg { return keeperMsg{} })
}

func (m *model) pollState() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	return tea.Tick(2*time.Second, func(time.Time) tea.Msg {
		state, err := client.State()
		return stateMsg{state, err}
	})
}

func (m *model) pollQuota() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	return func() tea.Msg {
		var payload clientQuota
		if err := client.Get("/api/quota", &payload); err != nil {
			return quotaMsg{}
		}
		var results []fetchResult
		for _, entry := range payload.Providers {
			name := entry.Provider
			if name == "agy" {
				name = "antigravity"
			}
			if entry.Snapshot != nil {
				snapshot := *entry.Snapshot
				if snapshot.Provider == "agy" {
					snapshot.Provider = "antigravity"
				}
				entry.Snapshot = &snapshot
			}
			result := fetchResult{Provider: name, Snapshot: entry.Snapshot}
			if entry.Error != nil {
				result.Err = entry.Error.Message
			}
			results = append(results, result)
		}
		return quotaMsg{results}
	}
}

func (m *model) readWS() tea.Cmd {
	if !m.serveMode || m.client == nil {
		return nil
	}
	events := m.events
	if events == nil {
		return nil
	}
	return func() tea.Msg {
		payload, ok := <-events
		if !ok {
			return nil
		}
		return wsMsg{payload}
	}
}

func (m *model) startEventReader() {
	client := m.client
	if client == nil {
		return
	}
	go func() {
		for {
			conn, err := client.WebSocket()
			if err != nil {
				time.Sleep(2 * time.Second)
				continue
			}
			for {
				_, data, err := conn.ReadMessage()
				if err != nil {
					conn.Close()
					time.Sleep(2 * time.Second)
					break
				}
				select {
				case m.events <- string(data):
				default:
				}
			}
		}
	}()
}

func (m *model) upsertSnapshot(snapshot provider.QuotaSnapshot) {
	key := snapshot.Provider + ":" + windowKey(snapshot.Windows)
	_ = key
	for i := range m.snapshots {
		if m.snapshots[i].Provider == snapshot.Provider {
			m.snapshots[i] = snapshot
			return
		}
	}
	m.snapshots = append(m.snapshots, snapshot)
	sort.SliceStable(m.snapshots, func(i, j int) bool {
		return providerOrder(m.snapshots[i].Provider) < providerOrder(m.snapshots[j].Provider)
	})
}

func windowKey(windows []provider.RateWindow) string {
	var parts []string
	for _, window := range windows {
		parts = append(parts, window.Label)
	}
	return strings.Join(parts, "|")
}

func providerOrder(provider string) int {
	switch provider {
	case "claude":
		return 0
	case "codex":
		return 1
	case "antigravity":
		return 2
	default:
		return 3
	}
}

func cachedQuotas(dataDir string, cfg config.Config) []provider.QuotaSnapshot {
	store := quota.NewCache(dataDir)
	var out []provider.QuotaSnapshot
	for _, id := range cfg.EnabledQuotaProviders([]string{"claude", "codex", "antigravity"}) {
		if cached, err := store.LoadValid(id, time.Now()); err == nil && cached != nil {
			out = append(out, cached.Snapshot)
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return providerOrder(out[i].Provider) < providerOrder(out[j].Provider) })
	return out
}

func fetchQuotas(dataDir string, cfg config.Config) []fetchResult {
	var fetchers []quota.Fetcher
	for _, id := range cfg.EnabledQuotaProviders([]string{"claude", "codex", "antigravity"}) {
		switch id {
		case "claude":
			fetchers = append(fetchers, quota.NewClaudeFetcher(cfg.Display.AccountDisplay.ShowsAccount()))
		case "codex":
			fetchers = append(fetchers, quota.NewCodexFetcher())
		case "antigravity":
			fetchers = append(fetchers, quota.NewAntigravityFetcher())
		}
	}
	var out []fetchResult
	for _, result := range quota.FetchAll(dataDir, fetchers) {
		entry := fetchResult{Provider: result.Provider, Snapshot: result.Snapshot}
		if result.Err != nil {
			entry.Err = result.Err.Error()
		}
		out = append(out, entry)
	}
	return out
}

// ── view ──

func (m *model) View() string {
	width := m.width
	if width == 0 {
		width = 100
	}
	var b strings.Builder
	b.WriteString(m.renderHeader(width))
	b.WriteString("\n")
	switch m.tab {
	case tabUsage:
		b.WriteString(m.renderUsage(width))
	case tabQuota:
		b.WriteString(m.renderQuota(width))
	case tabKeeper:
		b.WriteString(m.renderKeeper(width))
	case tabSettings:
		b.WriteString(m.renderSettings(width))
	}
	if m.detailKind != detailNone {
		b.WriteString("\n")
		b.WriteString(m.renderDetail(width))
	}
	if m.showFilter {
		b.WriteString("\n")
		b.WriteString(m.renderFilter(width))
	}
	if m.toast != "" && time.Since(m.toastAt) < 4*time.Second {
		b.WriteString("\n")
		b.WriteString(m.styles().toast.Render(m.toast))
	}
	b.WriteString("\n")
	b.WriteString(m.renderFooter(width))
	return b.String()
}

func (m *model) styles() styles {
	return stylesFor(m.theme)
}

func (m *model) renderHeader(width int) string {
	s := m.styles()
	var tabs []string
	for i, name := range tabNames {
		style := s.dim
		if i == m.tab {
			style = s.active
		}
		tabs = append(tabs, style.Render(" "+name+" "))
	}
	title := s.title.Render("TokenPulse " + version.String())
	tabBar := strings.Join(tabs, " ")
	gap := width - lipgloss.Width(title) - lipgloss.Width(tabBar) - 2
	if gap < 1 {
		gap = 1
	}
	return title + strings.Repeat(" ", gap) + tabBar
}

func (m *model) renderUsage(width int) string {
	s := m.styles()
	var b strings.Builder
	presets := []string{"today", "7d", "30d", "year", "all", "custom"}
	var parts []string
	for i, preset := range presets {
		label := fmt.Sprintf("%d %s", i+1, preset)
		if preset == m.preset {
			parts = append(parts, s.active.Render(label))
		} else {
			parts = append(parts, s.dim.Render(label))
		}
	}
	b.WriteString(strings.Join(parts, "  "))
	fmt.Fprintf(&b, "   %s..%s  scanned %s\n", m.report.Range.From, m.report.Range.To, formatScanned(m.report.ScannedAt))
	if m.reportErr != "" {
		b.WriteString(s.error.Render(m.reportErr) + "\n")
		return b.String()
	}
	b.WriteString("\n")
	totals := m.report.Totals
	metricCost := formatCost(totals.Cost)
	metricTokens := formatInt(totals.Tokens)
	comparison := ""
	if m.report.Comparison != nil && m.report.Range.From != m.report.Range.To {
		change := percentChange(totals.Cost, m.report.Comparison.Totals.Cost)
		if m.metric == 1 {
			change = percentChange(float64(totals.Tokens), float64(m.report.Comparison.Totals.Tokens))
		}
		comparison = fmt.Sprintf("  %s vs %s..%s", change, m.report.Comparison.From, m.report.Comparison.To)
	}
	fmt.Fprintf(&b, "Cost %s   Tokens %s   Sessions %s%s\n", metricCost, metricTokens, formatInt(totals.Sessions), comparison)
	unpriced := false
	for _, model := range m.report.Models {
		if model.Unpriced {
			unpriced = true
		}
	}
	if unpriced {
		b.WriteString(s.warn.Render("Some models are unpriced; cost is an estimate at public API prices.") + "\n")
	}
	b.WriteString("\n")
	b.WriteString(m.renderHeatmap(width))
	b.WriteString("\n")
	b.WriteString(m.renderList(width))
	return b.String()
}

func (m *model) renderHeatmap(width int) string {
	if m.report.Range.From == m.report.Range.To {
		return ""
	}
	s := m.styles()
	metric := func(day report.DaySummary) float64 {
		if m.metric == 1 {
			return day.Cost
		}
		return float64(day.Tokens)
	}
	var max float64
	for _, day := range m.report.Days {
		if value := metric(day); value > max {
			max = value
		}
	}
	byDate := map[string]report.DaySummary{}
	for _, day := range m.report.Days {
		byDate[day.Date] = day
	}
	cell := func(date time.Time, selected bool) string {
		day, ok := byDate[date.Format("2006-01-02")]
		level := 0
		if ok && max > 0 {
			level = int(metric(day)/max*4) + 1
			if level > 5 {
				level = 5
			}
		}
		block := "·"
		if level > 0 {
			block = []string{"", "░", "▒", "▓", "█", "█"}[level]
		}
		style := s.heat[level]
		if selected {
			style = s.selected
		}
		return style.Render(block)
	}
	var b strings.Builder
	from := mustDate(m.report.Range.From)
	to := mustDate(m.report.Range.To)
	switch m.report.Layout {
	case "strip":
		for date := from; !date.After(to); date = date.AddDate(0, 0, 1) {
			b.WriteString(cell(date, m.selectedDay == date.Format("2006-01-02")))
		}
		b.WriteString("  ")
		for date := from; !date.After(to); date = date.AddDate(0, 0, 1) {
			b.WriteString(date.Format("Mon")[:2] + " ")
		}
	case "week-rows":
		start := from.AddDate(0, 0, -((int(from.Weekday()) + 6) % 7))
		end := to.AddDate(0, 0, 6-((int(to.Weekday())+6)%7))
		for date := start; !date.After(end); date = date.AddDate(0, 0, 1) {
			if date.Weekday() == time.Monday && !date.Equal(start) {
				b.WriteString("\n")
			}
			if date.Before(from) || date.After(to) {
				b.WriteString(" ")
			} else {
				b.WriteString(cell(date, m.selectedDay == date.Format("2006-01-02")))
			}
		}
	case "week-columns":
		start := from.AddDate(0, 0, -((int(from.Weekday()) + 6) % 7))
		end := to.AddDate(0, 0, 6-((int(to.Weekday())+6)%7))
		columns := int(end.Sub(start).Hours()/24/7) + 1
		maxColumns := (width - 4) / 2
		if maxColumns < 1 {
			maxColumns = 1
		}
		first := 0
		if columns > maxColumns {
			first = columns - maxColumns
		}
		for row := 0; row < 7; row++ {
			for week := first; week < columns; week++ {
				date := start.AddDate(0, 0, week*7+row)
				if date.Before(from) || date.After(to) {
					b.WriteString("  ")
				} else {
					b.WriteString(cell(date, m.selectedDay == date.Format("2006-01-02")) + " ")
				}
			}
			if row < 6 {
				b.WriteString("\n")
			}
		}
	}
	b.WriteString("\n")
	b.WriteString(s.dim.Render("less ░ ▒ ▓ █ more   active " + formatInt(int64(activeDays(m.report.Days))) + " days"))
	return b.String()
}

func activeDays(days []report.DaySummary) int { return len(days) }

func (m *model) renderList(width int) string {
	s := m.styles()
	var b strings.Builder
	period := m.report.Range.From != m.report.Range.To
	if !period || m.listMode == 0 {
		if !period {
			b.WriteString(s.dim.Render("Models") + "\n")
		} else {
			b.WriteString(s.dim.Render("Dates (v: models)") + "\n")
		}
		rows := len(m.report.Models)
		if period {
			rows = len(m.report.Days)
		}
		if rows == 0 {
			b.WriteString(s.dim.Render("  no usage in this range") + "\n")
			return b.String()
		}
		max := m.maxRowMetric()
		for i, day := range m.report.Days {
			if !period {
				break
			}
			if i >= 20 {
				break
			}
			b.WriteString(m.renderRow(i, day.Date, fmt.Sprintf("%d sessions", day.Sessions), day.Cost, day.Tokens, max, s.text, false))
		}
		if !period {
			for i, model := range m.report.Models {
				if i >= 20 {
					break
				}
				agents := strings.Join(model.Agents, ",")
				b.WriteString(m.renderRow(i, model.Model, fmt.Sprintf("%d sessions · %s", model.Sessions, agents),
					model.Cost, model.Tokens, max, companyColor(model.Company, s), true))
			}
		}
	} else {
		b.WriteString(s.dim.Render("Models (v: dates)") + "\n")
		max := m.maxRowMetric()
		for i, model := range m.report.Models {
			if i >= 20 {
				break
			}
			agents := strings.Join(model.Agents, ",")
			b.WriteString(m.renderRow(i, model.Model, fmt.Sprintf("%d sessions · %s", model.Sessions, agents),
				model.Cost, model.Tokens, max, companyColor(model.Company, s), true))
		}
	}
	return b.String()
}

func companyColor(company string, s styles) lipgloss.Style {
	switch company {
	case "anthropic":
		return s.companyAnthropic
	case "openai":
		return s.companyOpenAI
	case "google":
		return s.companyGoogle
	default:
		return s.dim
	}
}

func (m *model) maxRowMetric() float64 {
	var max float64
	for _, day := range m.report.Days {
		if m.metric == 1 && day.Cost > max {
			max = day.Cost
		} else if m.metric == 0 && float64(day.Tokens) > max {
			max = float64(day.Tokens)
		}
	}
	for _, model := range m.report.Models {
		if m.metric == 1 && model.Cost > max {
			max = model.Cost
		} else if m.metric == 0 && float64(model.Tokens) > max {
			max = float64(model.Tokens)
		}
	}
	return max
}

func (m *model) renderRow(index int, label, detail string, cost float64, tokens int64, max float64, labelStyle lipgloss.Style, styled bool) string {
	s := m.styles()
	marker := " "
	if index == m.selectedRow {
		marker = ">"
	}
	value := float64(tokens)
	if m.metric == 1 {
		value = cost
	}
	bars := 0
	if max > 0 {
		bars = int(value / max * 12)
	}
	bar := strings.Repeat("▄", bars)
	if !styled {
		labelStyle = s.text
	}
	line := fmt.Sprintf("%s %-34s %-28s %8s %10s  %s", marker,
		truncate(label, 34), truncate(detail, 28), formatCost(cost), formatInt(tokens), bar)
	return labelStyle.Render(line) + "\n"
}

func (m *model) renderQuota(width int) string {
	if len(m.snapshots) == 0 {
		return m.styles().dim.Render("No quota snapshots. Press r to refresh.")
	}
	s := m.styles()
	var b strings.Builder
	for i, snapshot := range m.snapshots {
		style := s.text
		if i == m.quotaCursor {
			style = s.selected
		}
		plan := ""
		if m.opts.Config.Display.AccountDisplay.ShowsPlan() && snapshot.Plan != "" {
			plan = "  " + displayPlan(snapshot.Plan)
		}
		account := ""
		if m.opts.Config.Display.AccountDisplay.ShowsAccount() && snapshot.Account != "" {
			account = "  " + snapshot.Account
		}
		b.WriteString(style.Render(strings.ToUpper(snapshot.Provider)+plan+account) + "\n")
		if err := m.quotaErrors[snapshot.Provider]; err != "" {
			b.WriteString(s.error.Render("  "+err) + "\n")
		}
		for _, window := range snapshot.Windows {
			level := levelStyle(window.UsedPercent, s)
			percent := window.UsedPercent
			if m.opts.Config.Display.QuotaDisplayMode == config.QuotaRemaining {
				percent = 100 - percent
			}
			bars := int(percent / 100 * 20)
			bar := strings.Repeat("█", bars) + strings.Repeat("░", 20-bars)
			pace := ""
			if pace := computePace(window); pace != "" {
				pace = "  " + pace
			}
			fmt.Fprintf(&b, "  %-20s %s %5.1f%%%s\n", truncate(window.Label, 20), level.Render(bar), percent, pace)
		}
		if snapshot.ExtraUsage != nil {
			limit := ""
			if snapshot.ExtraUsage.Limit != nil {
				limit = fmt.Sprintf(" / $%.2f", *snapshot.ExtraUsage.Limit)
			}
			fmt.Fprintf(&b, "  Extra usage: $%.2f%s %s\n", snapshot.ExtraUsage.Used, limit, snapshot.ExtraUsage.Currency)
		}
		if len(snapshot.RateLimitResetCredits) > 0 {
			fmt.Fprintf(&b, "  Banked resets: %d available\n", len(snapshot.RateLimitResetCredits))
		}
		b.WriteString("\n")
	}
	return b.String()
}

func levelStyle(used float64, s styles) lipgloss.Style {
	switch {
	case used >= 100:
		return s.danger
	case used >= 80:
		return s.warn
	default:
		return s.good
	}
}

func computePace(window provider.RateWindow) string {
	pace := quota.ComputePace(window, time.Now())
	if pace == nil {
		return ""
	}
	switch pace.Kind {
	case "ahead":
		return "ahead"
	case "behind":
		return "behind"
	default:
		return "on track"
	}
}

func (m *model) renderKeeper(width int) string {
	s := m.styles()
	cfg := m.opts.Config
	var b strings.Builder
	engine := "off"
	if cfg.Keeper.Enabled {
		engine = "on"
	}
	b.WriteString(fmt.Sprintf("Keeper engine: %s  (e toggles)\n\n", engine))
	for i, agent := range keeper.Agents {
		entry := cfg.Keeper.Agents[agent]
		marker := " "
		if i == m.keeperCursor {
			marker = ">"
		}
		fmt.Fprintf(&b, "%s %-12s 5h=%v weekly=%v at %s\n", marker, agent,
			entry.SessionKeeperEnabled, entry.WeeklyKeeperEnabled, entry.DailyWakeupTime)
	}
	b.WriteString("\n")
	b.WriteString(s.dim.Render("Recent executions") + "\n")
	if len(m.keeperLog) == 0 {
		b.WriteString(s.dim.Render("  none") + "\n")
	}
	for _, record := range m.keeperLog {
		status := "ok"
		style := s.good
		if !record.Success {
			status = "failed"
			style = s.error
		}
		fmt.Fprintf(&b, "  %s %-10s %-8s %-8s %s %s\n", record.Timestamp.Format("01-02 15:04"),
			record.Agent, record.Trigger.Label(), status, record.Model,
			style.Render(truncate(record.OutputSnippet, 40)))
	}
	return b.String()
}

func (m *model) renderSettings(width int) string {
	s := m.styles()
	cfg := m.opts.Config
	rows := []string{
		fmt.Sprintf("Theme: %s", cfg.Display.Theme),
		fmt.Sprintf("Quota display: %s", cfg.Display.QuotaDisplayMode),
		fmt.Sprintf("Auto refresh: %ds", cfg.Display.AutoRefreshSecs),
		fmt.Sprintf("Account display: %s", cfg.Display.AccountDisplay),
		fmt.Sprintf("Show empty providers: %v", cfg.Display.ShowEmptyProviders),
		fmt.Sprintf("Refresh quota: %v", cfg.Display.RefreshQuota),
		fmt.Sprintf("Scan Antigravity: %v", cfg.Display.ScanAntigravity),
		fmt.Sprintf("Notification level: %s", cfg.Display.NotificationLevel),
		fmt.Sprintf("Notification sound: %s", cfg.Display.NotificationSound),
		fmt.Sprintf("Keeper engine: %v", cfg.Keeper.Enabled),
		fmt.Sprintf("Antigravity groups: %s", cfg.Keeper.AntigravityGroupsLabel()),
		"Quota providers: " + enabledProviderLabel(cfg),
	}
	var b strings.Builder
	for i, row := range rows {
		marker := " "
		style := s.text
		if i == m.settingsCursor {
			marker = ">"
			style = s.selected
		}
		b.WriteString(style.Render(marker+" "+row) + "\n")
	}
	return b.String()
}

func enabledProviderLabel(cfg config.Config) string {
	var enabled []string
	for _, id := range []string{"claude", "codex", "antigravity"} {
		if cfg.Providers[id].Enabled {
			enabled = append(enabled, id)
		}
	}
	return strings.Join(enabled, ",")
}

func (m *model) renderDetail(width int) string {
	s := m.styles()
	var b strings.Builder
	switch detail := m.detail.(type) {
	case report.DayDetail:
		b.WriteString(s.title.Render("Day " + detail.Date))
		b.WriteString("\n")
		b.WriteString(fmt.Sprintf("cost %s  tokens %s  input %s  output %s  cache read %s  cache write %s  hit %.2f%%  messages %d  sessions %d\n",
			formatCost(detail.Totals.Cost), formatInt(detail.Totals.Tokens), formatInt(detail.Totals.Input),
			formatInt(detail.Totals.Output), formatInt(detail.Totals.CacheRead), formatCacheWrite(detail.Totals.CacheWrite),
			cacheHit(detail.Totals), detail.Totals.Messages, detail.Totals.Sessions))
		for _, agent := range detail.Agents {
			b.WriteString(fmt.Sprintf("  %-12s %s  %s\n", agent.Agent, formatCost(agent.Totals.Cost), formatInt(agent.Totals.Tokens)))
			for _, model := range agent.Models {
				b.WriteString(fmt.Sprintf("    %-34s %s  %s\n", truncate(model.Model, 34), formatCost(model.Cost), formatInt(model.Tokens)))
			}
		}
	case report.ModelDetail:
		b.WriteString(s.title.Render(detail.Model + "  " + detail.Company + " · " + m.report.Range.From + ".." + m.report.Range.To))
		b.WriteString("\n")
		b.WriteString(fmt.Sprintf("cost %s  tokens %s  input %s  output %s  cache read %s  messages %d  last used %s\n",
			formatCost(detail.Totals.Cost), formatInt(detail.Totals.Tokens), formatInt(detail.Totals.Input),
			formatInt(detail.Totals.Output), formatInt(detail.Totals.CacheRead), detail.Totals.Messages, detail.LastUsed))
		for _, agent := range detail.Agents {
			b.WriteString(fmt.Sprintf("  %-12s %s  %s\n", agent.Agent, formatCost(agent.Cost), formatInt(agent.Tokens)))
		}
		if len(detail.PerDay) > 1 {
			b.WriteString("  ")
			var max int64
			for _, day := range detail.PerDay {
				if day.Tokens > max {
					max = day.Tokens
				}
			}
			for _, day := range detail.PerDay {
				height := 0
				if max > 0 {
					height = int(day.Tokens * 6 / max)
				}
				if height == 0 {
					b.WriteString(" ")
				} else {
					b.WriteString(strings.Repeat("▁▂▃▄▅▆", 1)[:1])
				}
			}
			b.WriteString("\n")
		}
	}
	b.WriteString(s.dim.Render("esc closes"))
	return b.String()
}

func (m *model) renderFilter(width int) string {
	s := m.styles()
	var b strings.Builder
	b.WriteString(s.title.Render("Agents") + "  " + s.dim.Render("space toggles · a all · esc closes") + "\n")
	for i, agent := range report.AllAgents {
		mark := "[ ]"
		if m.agents[agent] {
			mark = "[x]"
		}
		style := s.text
		if i == m.filterCursor {
			style = s.selected
		}
		b.WriteString(style.Render("  "+mark+" "+agent) + "\n")
	}
	return b.String()
}

func (m *model) renderFooter(width int) string {
	s := m.styles()
	help := "tab switch · r refresh · b theme · q quit"
	if m.tab == tabUsage {
		help = "1-6 range · m metric · v list · enter detail · f heatmap · [ ] shift · " + help
	}
	if m.status != "" {
		return s.dim.Render(truncate(m.status, width-2)) + "\n" + s.dim.Render(help)
	}
	return s.dim.Render(help)
}

// ── formatting ──

func formatCost(value float64) string { return fmt.Sprintf("$%.2f", value) }

func formatInt(value int64) string {
	raw := fmt.Sprintf("%d", value)
	negative := strings.HasPrefix(raw, "-")
	if negative {
		raw = raw[1:]
	}
	var parts []string
	for len(raw) > 3 {
		parts = append([]string{raw[len(raw)-3:]}, parts...)
		raw = raw[:len(raw)-3]
	}
	parts = append([]string{raw}, parts...)
	out := strings.Join(parts, ",")
	if negative {
		return "-" + out
	}
	return out
}

func formatScanned(ms int64) string {
	if ms == 0 {
		return "never"
	}
	return time.UnixMilli(ms).Format("15:04")
}

func formatCacheWrite(value int64) string {
	if value == 0 {
		return "—"
	}
	return formatInt(value)
}

func cacheHit(totals report.Totals) float64 {
	if totals.Tokens == 0 {
		return 0
	}
	return float64(totals.CacheRead) / float64(totals.Tokens) * 100
}

func percentChange(current, previous float64) string {
	if previous == 0 {
		if current == 0 {
			return "0.0%"
		}
		return "+100.0%"
	}
	change := (current - previous) / previous * 100
	return fmt.Sprintf("%+.1f%%", change)
}

func displayPlan(plan string) string {
	runes := []rune(strings.TrimSpace(plan))
	if len(runes) == 0 {
		return ""
	}
	return strings.ToUpper(string(runes[0])) + string(runes[1:])
}

func truncate(value string, width int) string {
	runes := []rune(value)
	if len(runes) <= width {
		return value
	}
	if width <= 1 {
		return string(runes[:width])
	}
	return string(runes[:width-1]) + "…"
}

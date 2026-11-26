package tui

import (
	"fmt"
	"sort"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/fangwangme/tokenpulse/internal/config"
	money "github.com/fangwangme/tokenpulse/internal/format"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/notification"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/quota"
	"github.com/fangwangme/tokenpulse/internal/remote"
	"github.com/fangwangme/tokenpulse/internal/report"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
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
	NoScan         bool
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

	keeperCursor int
	keeperLog    []keeper.ExecutionRecord

	settingsCursor int
	providerCursor int

	client      rt.Client
	rtStatus    rt.StatusSnapshot
	eventCh     <-chan rt.Event
	eventCancel func()
	webBusy     bool
}

type toastMsg struct{ text string }
type eventMsg struct{ event rt.Event }
type quotaViewMsg struct{ view rt.QuotaView }
type keeperLogMsg struct{ records []keeper.ExecutionRecord }
type configMsg struct{ cfg config.Config }
type statusMsg struct {
	status rt.StatusSnapshot
	err    error
}
type webToggleMsg struct {
	client      rt.Client
	status      rt.StatusSnapshot
	toast       string
	err         error
	clearClient bool
}

func Run(opts Options) error {
	client, err := attachClient(opts)
	if err != nil {
		return err
	}
	m := newModel(opts, client)
	program := tea.NewProgram(m, tea.WithAltScreen(), tea.WithMouseCellMotion())
	_, err = program.Run()
	m.closeClient()
	return err
}

func newModel(opts Options, client rt.Client) *model {
	cfg := opts.Config
	if client != nil {
		if loaded, err := client.Config(); err == nil {
			cfg = loaded
		}
	}
	opts.Config = cfg
	agents := map[string]bool{}
	for _, agent := range report.AllAgents {
		agents[agent] = true
	}
	if !cfg.Display.ScanAntigravity {
		agents["antigravity"] = false
	}
	m := &model{
		opts: opts, agents: agents, theme: 1, preset: "today", listMode: 0, focus: 1,
		quotaErrors: map[string]string{}, client: client,
	}
	if opts.Since != nil {
		m.preset = "custom"
		m.from = opts.Since.Format("2006-01-02")
		m.to = time.Now().Format("2006-01-02")
	}
	if client != nil {
		if status, err := client.Status(); err == nil {
			m.rtStatus = status
		}
		m.loadReport()
		m.loadQuota()
		m.loadKeeperLog()
	}
	return m
}

func (m *model) closeClient() {
	if m.eventCancel != nil {
		m.eventCancel()
		m.eventCancel = nil
		m.eventCh = nil
	}
	if m.client != nil {
		_ = m.client.Close()
		m.client = nil
	}
}

func (m *model) loadQuota() {
	if m.client == nil {
		return
	}
	view, err := m.client.Quota()
	if err != nil {
		return
	}
	m.applyQuotaView(view)
}

func (m *model) applyQuotaView(view rt.QuotaView) {
	m.snapshots = nil
	m.quotaErrors = map[string]string{}
	for _, entry := range view.Providers {
		if entry.Error != nil && entry.Error.Message != "" {
			m.quotaErrors[entry.Provider] = entry.Error.Message
		}
		if entry.Snapshot != nil {
			m.upsertSnapshot(*entry.Snapshot)
		}
	}
}

func (m *model) loadKeeperLog() {
	if m.client == nil {
		return
	}
	if records, err := m.client.KeeperRecords(20); err == nil {
		m.keeperLog = records
	}
}

func (m *model) loadReport() {
	m.reportErr = ""
	if m.client == nil {
		return
	}
	from, to, err := m.resolveRange()
	if err != nil {
		m.reportErr = err.Error()
		return
	}
	value, err := m.client.Usage(report.Params{
		Preset: m.preset, From: from, To: to, Agents: m.selectedAgents(),
	})
	if err != nil {
		m.reportErr = err.Error()
		return
	}
	m.report = value
	if m.selectedRow < 0 {
		m.selectedRow = 0
	}
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

func (m *model) subscribeEvents() tea.Cmd {
	if m.eventCancel != nil {
		m.eventCancel()
		m.eventCancel = nil
		m.eventCh = nil
	}
	if m.client == nil {
		return nil
	}
	ch, cancel, err := m.client.Subscribe()
	if err != nil {
		return nil
	}
	m.eventCh = ch
	m.eventCancel = cancel
	return m.nextEventCmd()
}

func (m *model) nextEventCmd() tea.Cmd {
	ch := m.eventCh
	if ch == nil {
		return nil
	}
	return func() tea.Msg {
		ev, ok := <-ch
		if !ok {
			return nil
		}
		return eventMsg{event: ev}
	}
}

func (m *model) statusPollCmd() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	return tea.Tick(3*time.Second, func(time.Time) tea.Msg {
		status, err := client.Status()
		return statusMsg{status: status, err: err}
	})
}

func (m *model) Init() tea.Cmd {
	return tea.Batch(m.subscribeEvents(), m.statusPollCmd())
}

func (m *model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		return m, nil
	case tea.KeyMsg:
		return m.handleKey(msg)
	case statusMsg:
		if msg.err != nil {
			if m.client != nil && m.client.IsRemote() && !m.webBusy {
				// If the background serve stopped or crashed, fall back to an
				// in-process runtime once runtime.lock becomes available.
				if lock, err := rt.AcquireLock(m.opts.DataDir); err == nil {
					if localClient, err := newInProcess(m.opts, lock, false, !m.opts.NoScan); err == nil {
						m.client = localClient
						if st, err := localClient.Status(); err == nil {
							m.rtStatus = st
						}
						m.status = ""
						m.toast = "Background serve stopped; switched to local mode"
						m.toastAt = time.Now()
						return m, tea.Batch(m.subscribeEvents(), m.statusPollCmd(), m.fetchConfigCmd())
					}
				}
				m.status = "Background serve unreachable; retrying…"
			}
			return m, m.statusPollCmd()
		}
		if m.status == "Background serve unreachable; retrying…" {
			m.status = ""
		}
		prevScanned := m.rtStatus.ScannedAt
		prevFetched := m.rtStatus.FetchedAt
		m.rtStatus = msg.status
		if msg.status.ScannedAt != prevScanned {
			m.loadReport()
		}
		var cmds []tea.Cmd
		cmds = append(cmds, m.statusPollCmd())
		if msg.status.FetchedAt != prevFetched {
			cmds = append(cmds, m.fetchQuotaCmd())
		}
		return m, tea.Batch(cmds...)
	case eventMsg:
		cmd := m.handleEvent(msg.event)
		return m, tea.Batch(cmd, m.nextEventCmd())
	case quotaViewMsg:
		m.applyQuotaView(msg.view)
		return m, nil
	case keeperLogMsg:
		m.keeperLog = msg.records
		return m, nil
	case configMsg:
		m.opts.Config = msg.cfg
		return m, nil
	case webToggleMsg:
		m.webBusy = false
		if msg.clearClient {
			m.client = nil
		}
		if msg.client != nil {
			m.client = msg.client
			m.rtStatus = msg.status
		}
		if msg.err != nil {
			m.toast = "Web toggle failed: " + msg.err.Error()
			m.toastAt = time.Now()
			return m, tea.Batch(m.subscribeEvents(), m.statusPollCmd())
		}
		m.toast = msg.toast
		m.toastAt = time.Now()
		m.loadReport()
		m.loadQuota()
		m.loadKeeperLog()
		return m, tea.Batch(m.subscribeEvents(), m.statusPollCmd(), m.fetchConfigCmd())
	case toastMsg:
		m.toast = msg.text
		m.toastAt = time.Now()
		return m, nil
	}
	return m, nil
}

func (m *model) fetchQuotaCmd() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	return func() tea.Msg {
		view, err := client.Quota()
		if err != nil {
			return nil
		}
		return quotaViewMsg{view: view}
	}
}

func (m *model) fetchKeeperLogCmd() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	return func() tea.Msg {
		records, err := client.KeeperRecords(20)
		if err != nil {
			return nil
		}
		return keeperLogMsg{records: records}
	}
}

func (m *model) fetchConfigCmd() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	return func() tea.Msg {
		cfg, err := client.Config()
		if err != nil {
			return nil
		}
		return configMsg{cfg: cfg}
	}
}

func (m *model) handleEvent(ev rt.Event) tea.Cmd {
	switch ev.Type {
	case "usage_updated":
		m.loadReport()
		return nil
	case "quota_updated":
		return m.fetchQuotaCmd()
	case "quota_recovered":
		m.toast = "A quota window was restored"
		m.toastAt = time.Now()
		return m.fetchQuotaCmd()
	case "keeper_updated":
		return m.fetchKeeperLogCmd()
	case "settings_updated":
		return m.fetchConfigCmd()
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
	if m.client == nil {
		return
	}
	if len(m.report.Days) > 0 && m.report.Range.From != m.report.Range.To && m.listMode == 0 {
		day := m.report.Days[m.selectedRow]
		detail, err := m.client.UsageDay(day.Date, m.selectedAgents())
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
		detail, err := m.client.UsageModel(model.Model, from, to, m.selectedAgents())
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
		return m, m.patchSettings(map[string]any{"keeper.enabled": !cfg.Keeper.Enabled})
	case "enter", " ":
		agent := keeper.Agents[m.keeperCursor]
		next := !cfg.Keeper.Agents[agent].SessionKeeperEnabled
		return m, m.patchSettings(map[string]any{"keeper.agents." + agent + ".session_keeper_enabled": next})
	case "w":
		agent := keeper.Agents[m.keeperCursor]
		next := !cfg.Keeper.Agents[agent].WeeklyKeeperEnabled
		return m, m.patchSettings(map[string]any{"keeper.agents." + agent + ".weekly_keeper_enabled": next})
	case "p":
		if m.client == nil {
			return m, nil
		}
		agent := keeper.Agents[m.keeperCursor]
		client := m.client
		return m, func() tea.Msg {
			if err := client.KeeperPing(agent, ""); err != nil {
				return toastMsg{"Ping failed: " + err.Error()}
			}
			return toastMsg{"Ping sent to " + agent}
		}
	}
	return m, nil
}

func (m *model) handleSettingsKey(key string) (tea.Model, tea.Cmd) {
	const rows = 13
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
		update = map[string]any{"display.theme": string(cfg.Display.Theme.Next())}
	case 1:
		next := config.QuotaUsed
		if cfg.Display.QuotaDisplayMode == config.QuotaUsed {
			next = config.QuotaRemaining
		}
		update = map[string]any{"display.quota_display_mode": string(next)}
	case 2:
		options := []uint32{0, 60, 120, 300, 600, 900}
		if m.client != nil && m.client.IsRemote() {
			options = []uint32{60, 120, 300, 600, 900}
		}
		next := uint32(900)
		for i, value := range options {
			if value == cfg.Display.AutoRefreshSecs {
				next = options[(i+1)%len(options)]
				break
			}
		}
		update = map[string]any{"display.auto_refresh_secs": int(next)}
	case 3:
		update = map[string]any{"display.account_display": string(cfg.Display.AccountDisplay.Next())}
	case 4:
		update = map[string]any{"display.show_empty_providers": !cfg.Display.ShowEmptyProviders}
	case 5:
		update = map[string]any{"display.refresh_quota": !cfg.Display.RefreshQuota}
	case 6:
		update = map[string]any{"display.scan_antigravity": !cfg.Display.ScanAntigravity}
	case 7:
		update = map[string]any{"display.notification_level": string(cfg.Display.NotificationLevel.Next())}
	case 8:
		sounds := []string{"chime", "Hero", "Glass", "Submarine", "none"}
		next := "chime"
		for i, sound := range sounds {
			if strings.EqualFold(sound, cfg.Display.NotificationSound) {
				next = sounds[(i+1)%len(sounds)]
				break
			}
		}
		update = map[string]any{"display.notification_sound": next}
		notification.PlayAlertSound(next)
	case 9:
		update = map[string]any{"keeper.enabled": !cfg.Keeper.Enabled}
	case 10:
		update = map[string]any{"keeper.agents.agy.weekly_all_groups": !cfg.Keeper.AntigravityAllGroups()}
	case 11:
		ids := []string{"claude", "codex", "antigravity"}
		id := ids[m.providerCursor%len(ids)]
		m.providerCursor = (m.providerCursor + 1) % len(ids)
		update = map[string]any{"providers." + id + ".enabled": !cfg.Providers[id].Enabled}
	case 12:
		return m.toggleWebRemote()
	}
	if update != nil {
		return m.patchSettings(update)
	}
	return nil
}

func (m *model) toggleWebRemote() tea.Cmd {
	if m.webBusy || m.client == nil {
		return nil
	}
	if m.rtStatus.ManagedBy == "systemd" || m.rtStatus.ManagedBy == "launchd" {
		m.toast = "Web remote is managed by " + m.rtStatus.ManagedBy + "; use `tokenpulse service`"
		m.toastAt = time.Now()
		return nil
	}
	if m.client.IsRemote() && m.rtStatus.ManagedBy == "standalone" {
		m.toast = "Attached to local TUI; toggle Web remote from that session"
		m.toastAt = time.Now()
		return nil
	}
	m.webBusy = true
	opts := m.opts
	wasRemote := m.client.IsRemote()
	prevClient := m.client
	if m.eventCancel != nil {
		m.eventCancel()
		m.eventCancel = nil
		m.eventCh = nil
	}
	return func() tea.Msg {
		if wasRemote {
			lock, err := stopDaemonAndAcquire(opts.DataDir)
			if err != nil {
				return webToggleMsg{client: prevClient, err: err}
			}
			next, err := newInProcess(opts, lock, false, !opts.NoScan)
			if err != nil {
				return webToggleMsg{err: err}
			}
			status, _ := next.Status()
			return webToggleMsg{client: next, status: status, toast: "Web remote stopped"}
		}
		_ = prevClient.Close()
		local, err := startDetachedServe(opts.DataDir)
		if err != nil {
			if lock, lerr := rt.AcquireLock(opts.DataDir); lerr == nil {
				if fallback, ferr := newInProcess(opts, lock, false, !opts.NoScan); ferr == nil {
					st, _ := fallback.Status()
					return webToggleMsg{client: fallback, status: st, err: err}
				}
			}
			return webToggleMsg{clearClient: true, err: err}
		}
		next := remote.NewSocketClient(local)
		status, _ := next.Status()
		msg := "Web remote started"
		if status.WebURL != "" {
			msg = "Web remote: " + status.WebURL
		}
		return webToggleMsg{client: next, status: status, toast: msg}
	}
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
	client := m.client
	return func() tea.Msg {
		cfg, err := client.PatchSettings(normalized)
		if err != nil {
			return toastMsg{"Settings failed: " + err.Error()}
		}
		return configMsg{cfg: cfg}
	}
}

func (m *model) refresh() tea.Cmd {
	if m.client == nil {
		return nil
	}
	client := m.client
	return func() tea.Msg {
		result, err := client.Refresh(true, true)
		if err != nil {
			return toastMsg{"Refresh failed: " + err.Error()}
		}
		if result.Throttled && !result.Accepted {
			return toastMsg{fmt.Sprintf("Showing cache; retry in %ds", result.RetryAfter)}
		}
		return toastMsg{"Refresh complete"}
	}
}

func (m *model) upsertSnapshot(snapshot provider.QuotaSnapshot) {
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
	body := b.String()
	footer := m.renderFooter(width)
	if m.height > 0 {
		bodyLines := strings.Count(body, "\n") + 1
		footerLines := strings.Count(footer, "\n") + 1
		if pad := m.height - bodyLines - footerLines; pad > 0 {
			body += strings.Repeat("\n", pad)
		}
	}
	return body + "\n" + footer
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
	modeBadge := s.dim.Render(" ○ local")
	if m.client != nil && m.client.IsRemote() {
		if m.rtStatus.ManagedBy == "standalone" {
			modeBadge = s.dim.Render(" ● local (attached)")
		} else {
			label := " ● web"
			if m.rtStatus.ManagedBy == "systemd" || m.rtStatus.ManagedBy == "launchd" {
				label = " ● " + m.rtStatus.ManagedBy
			}
			modeBadge = s.badgeOn.Render(label)
		}
	}
	title := s.title.Render("TokenPulse "+version.String()) + modeBadge
	tabBar := strings.Join(tabs, " ")
	gap := width - lipgloss.Width(title) - lipgloss.Width(tabBar) - 2
	if gap < 1 {
		gap = 1
	}
	rule := s.rule.Render(strings.Repeat("─", max(width, 20)))
	return title + strings.Repeat(" ", gap) + tabBar + "\n" + rule
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
		change := percentChange(float64(totals.Tokens), float64(m.report.Comparison.Totals.Tokens))
		if m.metric == 1 {
			change = percentChange(totals.Cost, m.report.Comparison.Totals.Cost)
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
	if index == m.selectedRow {
		return s.selected.Render(line) + "\n"
	}
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
			bars := barCells(percent)
			bar := strings.Repeat("█", bars) + strings.Repeat("░", 20-bars)
			pace := ""
			if text := computePace(window); text != "" {
				pace = "  " + text
			}
			reset := ""
			if window.ResetsAt != nil {
				if rem := time.Until(*window.ResetsAt); rem > 0 {
					reset = "  " + s.dim.Render("resets in "+formatDurationShort(rem))
				}
			}
			fmt.Fprintf(&b, "  %-20s %s %5.1f%%%s%s\n", truncate(displayWindowLabel(window.Label), 20), level.Render(bar), percent, pace, reset)
		}
		if snapshot.ExtraUsage != nil {
			limit := ""
			if snapshot.ExtraUsage.Limit != nil {
				limit = " / " + money.Cost(*snapshot.ExtraUsage.Limit)
			}
			fmt.Fprintf(&b, "  Extra usage: %s%s %s\n", money.Cost(snapshot.ExtraUsage.Used), limit, snapshot.ExtraUsage.Currency)
		}
		if len(snapshot.RateLimitResetCredits) > 0 {
			fmt.Fprintf(&b, "  Banked resets: %d available\n", len(snapshot.RateLimitResetCredits))
		}
		b.WriteString("\n")
	}
	return b.String()
}

func displayWindowLabel(label string) string {
	switch strings.ToLower(strings.TrimSpace(label)) {
	case "session (5h)", "5h", "5 hours":
		return "5h"
	case "weekly (7d)", "weekly":
		return "Weekly"
	default:
		return label
	}
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
	badge := func(on bool) string {
		if on {
			return s.badgeOn.Render("[ON]")
		}
		return s.badgeOff.Render("[OFF]")
	}
	b.WriteString(fmt.Sprintf("Keeper engine: %s  %s\n\n", badge(cfg.Keeper.Enabled), s.dim.Render("(e toggles)")))
	for i, agent := range keeper.Agents {
		entry := cfg.Keeper.Agents[agent]
		marker := " "
		if i == m.keeperCursor {
			marker = ">"
		}
		row := fmt.Sprintf("%s %-12s  5h %s  weekly %s  at %s", marker, agent,
			badge(entry.SessionKeeperEnabled), badge(entry.WeeklyKeeperEnabled), entry.DailyWakeupTime)
		if i == m.keeperCursor {
			b.WriteString(s.selected.Render(row) + "\n")
		} else {
			b.WriteString(row + "\n")
		}
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
	onOff := func(v bool) string {
		if v {
			return "ON"
		}
		return "OFF"
	}
	webLabel := "OFF (enter starts background serve)"
	if m.webBusy {
		webLabel = "switching…"
	} else if m.rtStatus.ManagedBy == "systemd" || m.rtStatus.ManagedBy == "launchd" {
		webLabel = fmt.Sprintf("managed by %s (%s)", m.rtStatus.ManagedBy, m.rtStatus.WebURL)
	} else if m.client != nil && m.client.IsRemote() && m.rtStatus.ManagedBy == "standalone" {
		webLabel = "attached to local TUI"
	} else if m.client != nil && m.client.IsRemote() {
		webLabel = fmt.Sprintf("ON (%s)  (enter stops)", m.rtStatus.WebURL)
	}
	rows := []string{
		fmt.Sprintf("%-26s %s", "Theme", cfg.Display.Theme),
		fmt.Sprintf("%-26s %s", "Quota display", cfg.Display.QuotaDisplayMode),
		fmt.Sprintf("%-26s %ds", "Auto refresh", cfg.Display.AutoRefreshSecs),
		fmt.Sprintf("%-26s %s", "Account display", cfg.Display.AccountDisplay),
		fmt.Sprintf("%-26s %s", "Show empty providers", onOff(cfg.Display.ShowEmptyProviders)),
		fmt.Sprintf("%-26s %s", "Refresh quota", onOff(cfg.Display.RefreshQuota)),
		fmt.Sprintf("%-26s %s", "Scan Antigravity", onOff(cfg.Display.ScanAntigravity)),
		fmt.Sprintf("%-26s %s", "Notification level", cfg.Display.NotificationLevel),
		fmt.Sprintf("%-26s %s", "Notification sound", cfg.Display.NotificationSound),
		fmt.Sprintf("%-26s %s", "Keeper engine", onOff(cfg.Keeper.Enabled)),
		fmt.Sprintf("%-26s %s", "Antigravity groups", cfg.Keeper.AntigravityGroupsLabel()),
		fmt.Sprintf("%-26s %s", "Quota providers (cycles)", enabledProviderLabel(cfg)),
		fmt.Sprintf("%-26s %s", "Web remote", webLabel),
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
		b.WriteString(fmt.Sprintf("cost %s  tokens %s  input %s  output %s  cache read %s  cache write %s  messages %d  sessions %d\n",
			formatCost(detail.Totals.Cost), formatInt(detail.Totals.Tokens), formatInt(detail.Totals.Input),
			formatInt(detail.Totals.Output), formatInt(detail.Totals.CacheRead), formatCacheWrite(detail.Totals.CacheWrite),
			detail.Totals.Messages, detail.Totals.Sessions))
		for _, agent := range detail.Agents {
			b.WriteString(fmt.Sprintf("  %-12s %s  %s\n", agent.Agent, formatCost(agent.Totals.Cost), formatInt(agent.Totals.Tokens)))
			for _, model := range agent.Models {
				b.WriteString(fmt.Sprintf("    %-34s %s  %s  cache read %s  hit %.2f%%\n", truncate(model.Model, 34),
					formatCost(model.Cost), formatInt(model.Tokens), formatInt(model.CacheRead), cacheHit(model.Totals)))
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
				b.WriteString(sparkRune(day.Tokens, max))
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

// barCells clamps a quota percentage to the 20-cell bar, so out-of-range
// provider data cannot panic strings.Repeat.
func barCells(percent float64) int {
	if !(percent > 0) {
		return 0
	}
	if percent > 100 {
		percent = 100
	}
	return int(percent / 100 * 20)
}

// sparkRune picks one rune, never a byte, for a day's share of the range max.
func sparkRune(tokens, max int64) string {
	levels := []rune("▁▂▃▄▅▆▇")
	if tokens <= 0 || max <= 0 {
		return " "
	}
	level := int(float64(tokens) / float64(max) * float64(len(levels)-1))
	if level < 0 {
		level = 0
	}
	if level >= len(levels) {
		level = len(levels) - 1
	}
	return string(levels[level])
}

func (m *model) renderFooter(width int) string {
	s := m.styles()
	common := "tab switch · r refresh · b theme · q quit"
	var help string
	switch m.tab {
	case tabUsage:
		help = "1-6 range · m metric · v list · enter detail · f heatmap · [ ] shift · s agents · " + common
	case tabQuota:
		help = "↑/↓ provider · " + common
	case tabKeeper:
		help = "e engine · enter 5h · w weekly · p ping · " + common
	case tabSettings:
		help = "↑/↓ move · enter change · " + common
	default:
		help = common
	}
	if m.status != "" {
		return s.dim.Render(truncate(m.status, width-2)) + "\n" + s.dim.Render(truncate(help, width-2))
	}
	return s.dim.Render(truncate(help, width-2))
}

func formatDurationShort(d time.Duration) string {
	if d < time.Minute {
		return "<1m"
	}
	days := int(d.Hours()) / 24
	hours := int(d.Hours()) % 24
	mins := int(d.Minutes()) % 60
	if days > 0 {
		return fmt.Sprintf("%dd %dh", days, hours)
	}
	if hours > 0 {
		return fmt.Sprintf("%dh %dm", hours, mins)
	}
	return fmt.Sprintf("%dm", mins)
}

// ── formatting ──

func formatCost(value float64) string { return money.Cost(value) }

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
	if width <= 0 {
		return ""
	}
	runes := []rune(value)
	if len(runes) <= width {
		return value
	}
	if width == 1 {
		return string(runes[:1])
	}
	return string(runes[:width-1]) + "…"
}

package tui

import (
	"fmt"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/quota"
	"github.com/fangwangme/tokenpulse/internal/remote"
	"github.com/fangwangme/tokenpulse/internal/report"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
	"github.com/fangwangme/tokenpulse/internal/scan"
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
	tabOverview = iota
	tabModels
	tabDaily
	tabActivity
	tabQuota
	tabKeeper
	tabSettings
	tabCount
)

var tabNames = []string{"Overview", "Models", "Daily", "Activity", "Quota", "Keeper", "Settings"}

var presets = []string{"today", "7d", "30d", "year", "all", "custom"}

const (
	metricTokens = iota
	metricCost
)

const (
	sortDate = iota
	sortCost
	sortTokens
)

type recoveryState struct {
	provider  string
	window    string
	remaining float64
	started   time.Time
}

func (r *recoveryState) active() bool {
	return r != nil && time.Since(r.started) < 5*time.Second
}

func (r *recoveryState) pulse() bool {
	return r != nil && time.Since(r.started) < 2500*time.Millisecond
}

type model struct {
	opts   Options
	client rt.Client
	cfg    config.Config

	width, height int
	tab           int
	themeOverride int

	rtStatus    rt.StatusSnapshot
	eventCh     <-chan rt.Event
	eventCancel func()
	webBusy     bool

	status      string
	statusLevel int
	statusAt    time.Time
	toast       string
	toastAt     time.Time
	refreshedAt time.Time
	recovery    *recoveryState

	preset     string
	basePreset string
	from, to   string

	agents       map[string]bool
	filterOpen   bool
	filterCursor int
	showHelp     bool

	customOpen   bool
	customField  int
	customFrom   string
	customTo     string
	customErr    string
	dailyToToday bool

	report        report.Report
	reportErr     string
	reportKey     string
	reportLoading bool
	reportLoaded  bool

	company        companyData
	companyKey     string
	companyLoading bool
	companyPending int
	epoch          int

	prior        report.Report
	priorKey     string
	priorLoading bool
	priorLoaded  bool

	overviewMetric int
	dailyMetric    int
	activityMetric int
	sortField      int
	sortAsc        bool

	overviewSel, overviewOff int
	modelsSel, modelsOff     int
	dailySel, dailyOff       int

	modelDetail  *report.ModelDetail
	dayDetail    *report.DayDetail
	detailScroll int

	activity        report.Report
	activityErr     string
	activityKey     string
	activityLoading bool
	activityLoaded  bool
	activitySel     string

	activityDetail      *report.DayDetail
	activityDetailDate  string
	activityDetailError string
	activityDetailLoad  bool
	activityDetailOff   int

	snapshots    []provider.QuotaSnapshot
	quotaErrors  map[string]string
	quotaLoaded  bool
	quotaLoading bool

	keeperLog     []keeper.ExecutionRecord
	keeperLoading bool
	keeperLoaded  bool
	keeperCursor  int

	settingsCursor int
}

type companyData struct {
	tokens map[string]map[string]int64
	cost   map[string]map[string]float64
}

func (c *companyData) empty() bool { return len(c.tokens) == 0 && len(c.cost) == 0 }

func (c *companyData) add(detail report.ModelDetail) {
	company := detail.Company
	if company != "anthropic" && company != "openai" && company != "google" {
		return
	}
	if c.tokens == nil {
		c.tokens = map[string]map[string]int64{}
		c.cost = map[string]map[string]float64{}
	}
	if c.tokens[company] == nil {
		c.tokens[company] = map[string]int64{}
		c.cost[company] = map[string]float64{}
	}
	for _, day := range detail.PerDay {
		c.tokens[company][day.Date] += day.Tokens
		c.cost[company][day.Date] += day.Cost
	}
}

type eventMsg struct{ event rt.Event }
type quotaViewMsg struct {
	view rt.QuotaView
	err  error
}
type keeperLogMsg struct {
	records []keeper.ExecutionRecord
	err     error
}
type configMsg struct {
	cfg     config.Config
	err     error
	message string
}
type statusMsg struct {
	status rt.StatusSnapshot
	err    error
}
type toastMsg struct{ text string }
type recoveryTickMsg struct{}
type refreshMsg struct {
	result rt.RefreshResult
	err    error
}
type webToggleMsg struct {
	client      rt.Client
	status      rt.StatusSnapshot
	toast       string
	err         error
	clearClient bool
}

type reportMsg struct {
	key   string
	value report.Report
	err   error
}
type priorMsg struct {
	key   string
	value report.Report
	err   error
}
type activityMsg struct {
	key   string
	value report.Report
	err   error
}
type modelDetailMsg struct {
	key     string
	model   string
	purpose string
	value   report.ModelDetail
	err     error
}
type dayDetailMsg struct {
	key   string
	date  string
	value report.DayDetail
	err   error
}
type activityDayMsg struct {
	epoch int
	date  string
	value report.DayDetail
	err   error
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
	agents := map[string]bool{}
	for _, agent := range report.AllAgents {
		agents[agent] = true
	}
	m := &model{
		opts:           opts,
		client:         client,
		cfg:            cfg,
		themeOverride:  -1,
		agents:         agents,
		preset:         "30d",
		basePreset:     "30d",
		sortField:      sortTokens,
		overviewMetric: metricTokens,
		dailyMetric:    metricTokens,
		activityMetric: metricTokens,
		quotaErrors:    map[string]string{},
	}
	if opts.Since != nil {
		m.preset = "custom"
		m.basePreset = "custom"
		m.from = opts.Since.Format("2006-01-02")
		m.to = dateString(startOfToday())
	}
	if client != nil {
		if status, err := client.Status(); err == nil {
			m.rtStatus = status
		}
	}
	return m
}

// configValue keeps the model usable when a test constructs it directly with
// only Options.Config set.
func (m *model) configValue() config.Config {
	if m.cfg.Version == 0 && m.opts.Config.Version != 0 {
		return m.opts.Config
	}
	return m.cfg
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

func (m *model) theme() tuiTheme { return resolveTheme(m.cfg, m.themeOverride) }

func (m *model) setStatus(text string, level int) {
	m.status = text
	m.statusLevel = level
	m.statusAt = time.Now()
}

func (m *model) setToast(text string) {
	m.toast = text
	m.toastAt = time.Now()
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

func (m *model) agentCount() int {
	count := 0
	for _, selected := range m.agents {
		if selected {
			count++
		}
	}
	return count
}

func (m *model) rangeKey() string {
	parts := m.selectedAgents()
	return fmt.Sprintf("%s|%s|%s|%s|%d", m.preset, m.from, m.to, strings.Join(parts, ","), m.epoch)
}

func (m *model) rangeParams() report.Params {
	params := report.Params{Preset: m.preset, Agents: m.selectedAgents()}
	if m.preset == "custom" {
		params.From = m.from
		params.To = m.to
	}
	return params
}

func (m *model) loadReportCmd() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	key := m.rangeKey()
	params := m.rangeParams()
	m.reportKey = key
	m.reportLoading = true
	return func() tea.Msg {
		value, err := client.Usage(params)
		return reportMsg{key: key, value: value, err: err}
	}
}

func (m *model) ensureReport() tea.Cmd {
	if m.client == nil {
		return nil
	}
	key := m.rangeKey()
	if m.reportKey == key && (m.reportLoaded || m.reportLoading) {
		return nil
	}
	return m.loadReportCmd()
}

func (m *model) ensurePrior() tea.Cmd {
	if m.client == nil || !m.reportLoaded || m.report.FirstDay == nil {
		return nil
	}
	key := m.rangeKey()
	if m.priorKey == key && (m.priorLoaded || m.priorLoading) {
		return nil
	}
	from, err := parseDate(m.report.Range.From)
	if err != nil {
		return nil
	}
	start := from.AddDate(0, 0, -7)
	first, err := parseDate(*m.report.FirstDay)
	if err != nil {
		return nil
	}
	if start.Before(first) {
		start = first
	}
	end := from.AddDate(0, 0, -1)
	if start.After(end) {
		m.priorKey = key
		m.priorLoaded = true
		m.prior = report.Report{}
		return nil
	}
	params := report.Params{
		Preset: "custom",
		From:   dateString(start),
		To:     dateString(end),
		Agents: m.selectedAgents(),
	}
	m.priorKey = key
	m.priorLoading = true
	client := m.client
	return func() tea.Msg {
		value, err := client.Usage(params)
		return priorMsg{key: key, value: value, err: err}
	}
}

func (m *model) activityWindow() (string, string, bool) {
	today := startOfToday()
	start := today.AddDate(0, 0, -364)
	if jan1 := time.Date(today.Year(), 1, 1, 0, 0, 0, 0, time.Local); jan1.Before(start) {
		start = jan1
	}
	if m.reportLoaded && m.report.FirstDay != nil {
		first, err := parseDate(*m.report.FirstDay)
		if err == nil && first.After(start) {
			start = first
		}
	} else if !m.reportLoaded {
		return "", "", false
	} else {
		// No data at all: the service rejects a start before today.
		start = today
	}
	return dateString(start), dateString(today), true
}

func (m *model) loadActivityCmd() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	from, to, ok := m.activityWindow()
	if !ok {
		return nil
	}
	key := fmt.Sprintf("activity|%s|%s|%s", from, to, strings.Join(m.selectedAgents(), ","))
	params := report.Params{Preset: "custom", From: from, To: to, Agents: m.selectedAgents()}
	m.activityKey = key
	m.activityLoading = true
	return func() tea.Msg {
		value, err := client.Usage(params)
		return activityMsg{key: key, value: value, err: err}
	}
}

func (m *model) ensureActivity() tea.Cmd {
	if m.client == nil {
		return nil
	}
	if !m.reportLoaded {
		return nil
	}
	from, to, ok := m.activityWindow()
	if !ok {
		return nil
	}
	key := fmt.Sprintf("activity|%s|%s|%s", from, to, strings.Join(m.selectedAgents(), ","))
	if m.activityKey == key && (m.activityLoaded || m.activityLoading) {
		return nil
	}
	return m.loadActivityCmd()
}

func (m *model) ensureCompany() tea.Cmd {
	if m.client == nil || !m.reportLoaded {
		return nil
	}
	key := m.rangeKey()
	if m.companyKey == key && (m.companyLoading || !m.company.empty()) {
		return nil
	}
	var cmds []tea.Cmd
	client := m.client
	from, to := m.report.Range.From, m.report.Range.To
	agents := m.selectedAgents()
	m.companyKey = key
	m.companyLoading = true
	m.companyPending = 0
	for _, entry := range m.report.Models {
		if entry.Company == "anthropic" || entry.Company == "openai" || entry.Company == "google" {
			m.companyPending++
		}
	}
	for _, entry := range m.report.Models {
		if entry.Company != "anthropic" && entry.Company != "openai" && entry.Company != "google" {
			continue
		}
		name := entry.Model
		cmds = append(cmds, func() tea.Msg {
			value, err := client.UsageModel(name, from, to, agents)
			return modelDetailMsg{key: key, model: name, purpose: "company", value: value, err: err}
		})
	}
	if len(cmds) == 0 || m.companyPending == 0 {
		m.companyLoading = false
	}
	return tea.Batch(cmds...)
}

func (m *model) ensureActivityDetail() tea.Cmd {
	if m.client == nil || !m.activityLoaded || m.activitySel == "" {
		return nil
	}
	if m.activityDetailDate == m.activitySel && (m.activityDetailLoad || m.activityDetail != nil) {
		return nil
	}
	client := m.client
	date := m.activitySel
	m.activityDetailDate = date
	m.activityDetailLoad = true
	m.activityDetail = nil
	m.activityDetailError = ""
	agents := m.selectedAgents()
	epoch := m.epoch
	return func() tea.Msg {
		value, err := client.UsageDay(date, agents)
		return activityDayMsg{epoch: epoch, date: date, value: value, err: err}
	}
}

func (m *model) fetchQuotaCmd() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	m.quotaLoading = true
	return func() tea.Msg {
		view, err := client.Quota()
		return quotaViewMsg{view: view, err: err}
	}
}

func (m *model) applyQuotaView(view rt.QuotaView) {
	m.snapshots = nil
	m.quotaErrors = map[string]string{}
	cfg := m.configValue()
	showEmpty := cfg.Display.ShowEmptyProviders
	byID := map[string]rt.QuotaProviderView{}
	for _, entry := range view.Providers {
		byID[entry.Provider] = entry
	}
	for _, id := range cfg.QuotaProviderOrder(quota.ProviderIDs) {
		entry, ok := byID[id]
		if !ok || !entry.Enabled {
			continue
		}
		if entry.Error != nil && entry.Error.Message != "" {
			m.quotaErrors[entry.Provider] = entry.Error.Message
		}
		if entry.Snapshot != nil {
			m.snapshots = append(m.snapshots, *entry.Snapshot)
		} else if showEmpty {
			m.snapshots = append(m.snapshots, provider.QuotaSnapshot{Provider: entry.Provider})
		}
	}
	m.quotaLoaded = true
	m.quotaLoading = false
}

func (m *model) fetchKeeperLogCmd() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	m.keeperLoading = true
	return func() tea.Msg {
		records, err := client.KeeperRecords(50)
		return keeperLogMsg{records: records, err: err}
	}
}

func (m *model) fetchConfigCmd() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	return func() tea.Msg {
		cfg, err := client.Config()
		return configMsg{cfg: cfg, err: err}
	}
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
	m.reportLoading = false
	return tea.Batch(
		m.subscribeEvents(),
		m.statusPollCmd(),
		m.fetchConfigCmd(),
		m.fetchQuotaCmd(),
		m.fetchKeeperLogCmd(),
		m.loadReportCmd(),
	)
}

func (m *model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		return m, nil
	case tea.KeyMsg:
		return m.handleKey(msg)
	case tea.MouseMsg:
		return m.handleMouse(msg)
	case statusMsg:
		return m.handleStatus(msg)
	case eventMsg:
		cmd := m.handleEvent(msg.event)
		return m, tea.Batch(cmd, m.nextEventCmd())
	case reportMsg:
		return m.handleReport(msg)
	case priorMsg:
		if msg.key == m.rangeKey() {
			m.priorLoading = false
			if msg.err != nil {
				m.priorLoaded = false
				m.prior = report.Report{}
				m.setStatus(msg.err.Error(), 2)
			} else {
				m.priorLoaded = true
				m.prior = msg.value
			}
		}
		return m, nil
	case activityMsg:
		return m.handleActivity(msg)
	case modelDetailMsg:
		return m.handleModelDetail(msg)
	case dayDetailMsg:
		if msg.key == m.dayDetailKey(msg.date) {
			m.dayDetail = &msg.value
		}
		return m, nil
	case activityDayMsg:
		if msg.epoch == m.epoch && msg.date == m.activitySel {
			m.activityDetailLoad = false
			if msg.err != nil {
				m.activityDetail = nil
				m.activityDetailError = msg.err.Error()
			} else {
				m.activityDetail = &msg.value
				m.activityDetailError = ""
			}
		}
		return m, nil
	case quotaViewMsg:
		m.quotaLoading = false
		if msg.err != nil {
			m.setStatus("Quota: "+msg.err.Error(), 2)
		} else {
			m.applyQuotaView(msg.view)
		}
		return m, nil
	case keeperLogMsg:
		m.keeperLoading = false
		if msg.err != nil {
			m.setStatus("Keeper: "+msg.err.Error(), 2)
		} else {
			m.keeperLog = msg.records
			m.keeperLoaded = true
		}
		return m, nil
	case configMsg:
		if msg.err != nil {
			m.setStatus("Settings: "+msg.err.Error(), 2)
			return m, nil
		}
		m.cfg = msg.cfg
		m.opts.Config = msg.cfg
		if msg.message != "" {
			m.setToast(msg.message + " updated")
		}
		return m, m.fetchQuotaCmd()
	case refreshMsg:
		return m, m.handleRefresh(msg)
	case webToggleMsg:
		return m.handleWebToggle(msg)
	case toastMsg:
		m.setToast(msg.text)
		return m, nil
	case recoveryTickMsg:
		if m.recovery != nil && m.recovery.active() {
			return m, m.recoveryTickCmd()
		}
		return m, nil
	}
	return m, nil
}

func (m *model) handleStatus(msg statusMsg) (tea.Model, tea.Cmd) {
	if msg.err != nil {
		if m.client != nil && m.client.IsRemote() && !m.webBusy {
			if lock, err := rt.AcquireLock(m.opts.DataDir); err == nil {
				if localClient, err := newInProcess(m.opts, lock, false, !m.opts.NoScan); err == nil {
					m.client = localClient
					if st, err := localClient.Status(); err == nil {
						m.rtStatus = st
					}
					m.setStatus("", 0)
					m.setToast("Background serve stopped; switched to local mode")
					m.invalidateUsage()
					m.quotaLoaded = false
					m.keeperLoaded = false
					return m, tea.Batch(m.subscribeEvents(), m.statusPollCmd(), m.fetchConfigCmd(),
						m.fetchQuotaCmd(), m.fetchKeeperLogCmd(), m.currentTabCmd())
				}
			}
			m.setStatus("Background serve unreachable; retrying…", 1)
		}
		return m, m.statusPollCmd()
	}
	if strings.HasPrefix(m.status, "Background serve unreachable") {
		m.setStatus("", 0)
	}
	previous := m.rtStatus
	m.rtStatus = msg.status
	var cmds []tea.Cmd
	if msg.status.ScannedAt != previous.ScannedAt {
		m.refreshedAt = time.Now()
		m.invalidateUsage()
		cmds = append(cmds, m.currentTabCmd())
	}
	if msg.status.FetchedAt != previous.FetchedAt {
		cmds = append(cmds, m.fetchQuotaCmd())
	}
	cmds = append(cmds, m.statusPollCmd())
	return m, tea.Batch(cmds...)
}

func (m *model) handleEvent(ev rt.Event) tea.Cmd {
	switch ev.Type {
	case "usage_updated":
		m.invalidateUsage()
		return m.currentTabCmd()
	case "quota_updated":
		return m.fetchQuotaCmd()
	case "quota_recovered":
		state := &recoveryState{started: time.Now()}
		if data, ok := ev.Data.(map[string]any); ok {
			state.provider, _ = data["provider"].(string)
			state.window, _ = data["window"].(string)
			switch value := data["remaining"].(type) {
			case float64:
				state.remaining = value
			case int:
				state.remaining = float64(value)
			}
		}
		m.recovery = state
		m.setToast("")
		return tea.Batch(m.fetchQuotaCmd(), m.recoveryTickCmd())
	case "keeper_updated":
		return m.fetchKeeperLogCmd()
	case "settings_updated":
		return m.fetchConfigCmd()
	}
	return nil
}

func (m *model) handleReport(msg reportMsg) (tea.Model, tea.Cmd) {
	if msg.key != m.rangeKey() {
		return m, nil
	}
	m.reportLoading = false
	if msg.err != nil {
		m.reportLoaded = false
		m.reportErr = msg.err.Error()
		m.setStatus(msg.err.Error(), 2)
		return m, nil
	}
	m.report = msg.value
	m.reportErr = ""
	m.reportLoaded = true
	m.refreshedAt = time.Now()
	m.overviewSel = clampInt(m.overviewSel, 0, max(len(m.report.Models)-1, 0))
	m.modelsSel = clampInt(m.modelsSel, 0, max(len(m.report.Models)-1, 0))
	m.dailySel = clampInt(m.dailySel, 0, max(len(m.report.Days)-1, 0))
	var cmds []tea.Cmd
	if m.dailyToToday {
		m.dailyToToday = false
		for i, day := range m.report.Days {
			if day.Date == dateString(startOfToday()) {
				m.dailySel = i
				break
			}
		}
	}
	switch m.tab {
	case tabOverview:
		cmds = append(cmds, m.ensureCompany())
	case tabDaily:
		cmds = append(cmds, m.ensurePrior())
	case tabActivity:
		cmds = append(cmds, m.ensureActivity())
	}
	return m, tea.Batch(cmds...)
}

func (m *model) handleActivity(msg activityMsg) (tea.Model, tea.Cmd) {
	if msg.key != m.activityKey {
		return m, nil
	}
	m.activityLoading = false
	if msg.err != nil {
		m.activityLoaded = false
		m.activityErr = msg.err.Error()
		m.setStatus(msg.err.Error(), 2)
		return m, nil
	}
	m.activity = msg.value
	m.activityErr = ""
	m.activityLoaded = true
	today := dateString(startOfToday())
	hasToday := false
	for _, day := range m.activity.Days {
		if day.Date == today {
			hasToday = true
			break
		}
	}
	if m.activitySel == "" || !activityDayExists(m.activity.Days, m.activitySel) {
		if hasToday {
			m.activitySel = today
		} else if len(m.activity.Days) > 0 {
			m.activitySel = m.activity.Days[0].Date
		}
	}
	return m, m.ensureActivityDetail()
}

func activityDayExists(days []report.DaySummary, date string) bool {
	for _, day := range days {
		if day.Date == date {
			return true
		}
	}
	return false
}

func (m *model) handleModelDetail(msg modelDetailMsg) (tea.Model, tea.Cmd) {
	if msg.purpose == "company" {
		if msg.key != m.rangeKey() {
			return m, nil
		}
		if msg.err != nil {
			m.setStatus("Company: "+msg.err.Error(), 2)
		} else {
			m.company.add(msg.value)
		}
		if m.companyKey == msg.key {
			m.companyPending = max(m.companyPending-1, 0)
			if m.companyPending == 0 {
				m.companyLoading = false
			}
		}
		return m, nil
	}
	if msg.purpose == "detail" {
		if msg.key != m.rangeKey() {
			return m, nil
		}
		if msg.err != nil {
			m.setStatus("Model: "+msg.err.Error(), 2)
			return m, nil
		}
		m.modelDetail = &msg.value
		m.detailScroll = 0
	}
	return m, nil
}

func (m *model) handleRefresh(msg refreshMsg) tea.Cmd {
	if msg.err != nil {
		m.setStatus("Refresh: "+msg.err.Error(), 2)
		return nil
	}
	if msg.result.Throttled && !msg.result.Accepted {
		m.setStatus(fmt.Sprintf("Showing cache; retry in %ds", msg.result.RetryAfter), 1)
		return nil
	}
	m.setToast("Refresh complete")
	m.refreshedAt = time.Now()
	m.invalidateUsage()
	return tea.Batch(m.currentTabCmd(), m.fetchQuotaCmd())
}

func (m *model) handleWebToggle(msg webToggleMsg) (tea.Model, tea.Cmd) {
	m.webBusy = false
	if msg.clearClient {
		m.client = nil
	}
	if msg.client != nil {
		m.client = msg.client
		m.rtStatus = msg.status
	}
	if msg.err != nil {
		m.setToast("Web toggle failed: " + msg.err.Error())
		m.invalidateUsage()
		m.quotaLoaded = false
		m.keeperLoaded = false
		return m, tea.Batch(m.subscribeEvents(), m.statusPollCmd(),
			m.fetchQuotaCmd(), m.fetchKeeperLogCmd(), m.currentTabCmd())
	}
	m.setToast(msg.toast)
	m.invalidateUsage()
	m.quotaLoaded = false
	m.keeperLoaded = false
	return m, tea.Batch(
		m.subscribeEvents(), m.statusPollCmd(), m.fetchConfigCmd(),
		m.fetchQuotaCmd(), m.fetchKeeperLogCmd(), m.currentTabCmd(),
	)
}

func (m *model) invalidateUsage() {
	m.reportLoaded = false
	m.reportKey = ""
	m.company = companyData{}
	m.companyKey = ""
	m.companyLoading = false
	m.companyPending = 0
	m.epoch++
	m.prior = report.Report{}
	m.priorKey = ""
	m.priorLoaded = false
	m.priorLoading = false
	m.activity = report.Report{}
	m.activityKey = ""
	m.activityLoaded = false
	m.activityLoading = false
	m.activityDetail = nil
	m.activityDetailDate = ""
	m.activityDetailLoad = false
	m.modelDetail = nil
	m.dayDetail = nil
}

func (m *model) currentTabCmd() tea.Cmd {
	switch m.tab {
	case tabOverview:
		return tea.Batch(m.ensureReport(), m.ensureCompany())
	case tabModels:
		return m.ensureReport()
	case tabDaily:
		return tea.Batch(m.ensureReport(), m.ensurePrior())
	case tabActivity:
		return tea.Batch(m.ensureActivity(), m.ensureActivityDetail())
	case tabQuota:
		if !m.quotaLoaded {
			return m.fetchQuotaCmd()
		}
	case tabKeeper:
		if !m.keeperLoaded {
			return m.fetchKeeperLogCmd()
		}
	case tabSettings:
		return m.fetchConfigCmd()
	}
	return nil
}

func (m *model) switchTab(direction int) {
	m.tab = (m.tab + direction + tabCount) % tabCount
	m.overviewSel, m.overviewOff = 0, 0
	m.modelsSel, m.modelsOff = 0, 0
	m.dailySel, m.dailyOff = 0, 0
	m.detailScroll = 0
	m.activityDetailOff = 0
	m.modelDetail = nil
	m.dayDetail = nil
	m.showHelp = false
}

func (m *model) currentRangeTab() bool {
	return m.tab == tabOverview || m.tab == tabModels || m.tab == tabDaily
}

func (m *model) selectPreset(index int) tea.Cmd {
	preset := presets[index]
	if preset == "custom" {
		m.customOpen = true
		m.customField = 0
		m.customErr = ""
		from, to := m.report.Range.From, m.report.Range.To
		if !m.reportLoaded {
			now := startOfToday()
			from = dateString(now.AddDate(0, 0, -29))
			to = dateString(now)
		}
		m.customFrom = from
		m.customTo = to
		return nil
	}
	m.preset = preset
	m.basePreset = preset
	m.from, m.to = "", ""
	m.invalidateUsage()
	return m.ensureReport()
}

func (m *model) shiftRange(direction int) tea.Cmd {
	if !m.reportLoaded {
		return nil
	}
	from, err1 := parseDate(m.report.Range.From)
	to, err2 := parseDate(m.report.Range.To)
	if err1 != nil || err2 != nil {
		return nil
	}
	length := int(to.Sub(from).Hours()/24) + 1
	shift := direction * length
	today := startOfToday()
	newFrom := from.AddDate(0, 0, shift)
	newTo := to.AddDate(0, 0, shift)
	if newTo.After(today) {
		offset := int(newTo.Sub(today).Hours() / 24)
		newTo = today
		newFrom = newFrom.AddDate(0, 0, -offset)
	}
	if m.report.FirstDay != nil {
		if first, err := parseDate(*m.report.FirstDay); err == nil && newFrom.Before(first) {
			newFrom = first
			newTo = newFrom.AddDate(0, 0, length-1)
			if newTo.After(today) {
				newTo = today
			}
		}
	}
	m.preset = "custom"
	m.from = dateString(newFrom)
	m.to = dateString(newTo)
	m.invalidateUsage()
	return m.ensureReport()
}

func (m *model) todayRange() tea.Cmd {
	preset := m.basePreset
	if preset == "" || preset == "custom" {
		preset = "30d"
	}
	m.preset = preset
	m.from, m.to = "", ""
	m.invalidateUsage()
	if m.tab == tabDaily {
		m.dailyToToday = true
	}
	return m.ensureReport()
}

func (m *model) applyCustomRange() tea.Cmd {
	from, err := parseDate(m.customFrom)
	if err != nil {
		m.customErr = "Invalid start date"
		return nil
	}
	to, err := parseDate(m.customTo)
	if err != nil {
		m.customErr = "Invalid end date"
		return nil
	}
	if from.After(to) {
		from, to = to, from
	}
	today := startOfToday()
	if from.After(today) || to.After(today) {
		m.customErr = "Dates cannot be after today"
		return nil
	}
	if m.report.FirstDay != nil {
		if first, err := parseDate(*m.report.FirstDay); err == nil && from.Before(first) {
			m.customErr = "Start cannot be before " + *m.report.FirstDay
			return nil
		}
	}
	m.customOpen = false
	m.customErr = ""
	m.preset = "custom"
	m.from = dateString(from)
	m.to = dateString(to)
	m.invalidateUsage()
	return m.ensureReport()
}

func (m *model) handleKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	key := msg.String()
	if key == "ctrl+c" {
		return m, tea.Quit
	}
	if m.showHelp {
		m.showHelp = false
		return m, nil
	}
	if m.customOpen {
		return m.handleCustomKey(msg)
	}
	if m.filterOpen {
		return m.handleFilterKey(msg)
	}
	if m.modelDetail != nil || m.dayDetail != nil {
		switch key {
		case "q":
			return m, tea.Quit
		case "esc":
			m.modelDetail = nil
			m.dayDetail = nil
			m.detailScroll = 0
		case "j", "down":
			m.detailScroll++
		case "k", "up":
			if m.detailScroll > 0 {
				m.detailScroll--
			}
		}
		return m, nil
	}
	switch key {
	case "q":
		return m, tea.Quit
	case "?":
		m.showHelp = true
		return m, nil
	case "b":
		if m.themeOverride < 0 {
			m.themeOverride = 1 - m.theme().mode
		} else {
			m.themeOverride = 1 - m.themeOverride
		}
		return m, nil
	case "s":
		m.filterOpen = true
		m.filterCursor = 0
		return m, nil
	case "r":
		return m, m.refreshCmd()
	case "esc":
		return m, nil
	case "left", "h":
		m.switchTab(-1)
		return m, m.currentTabCmd()
	case "right", "l":
		m.switchTab(1)
		return m, m.currentTabCmd()
	case "tab", "shift+tab":
		direction := 1
		if key == "shift+tab" {
			direction = -1
		}
		if m.tab == tabKeeper {
			m.moveKeeperCursor(direction)
			return m, nil
		}
		m.switchTab(direction)
		return m, m.currentTabCmd()
	}
	if m.currentRangeTab() && len(key) == 1 && key[0] >= '1' && key[0] <= '6' {
		return m, m.selectPreset(int(key[0] - '1'))
	}
	if m.currentRangeTab() {
		switch key {
		case "[":
			return m, m.shiftRange(-1)
		case "]":
			return m, m.shiftRange(1)
		case "n":
			return m, m.todayRange()
		}
	}
	switch m.tab {
	case tabOverview:
		return m.handleOverviewKey(key)
	case tabModels:
		return m.handleModelsKey(key)
	case tabDaily:
		return m.handleDailyKey(key)
	case tabActivity:
		return m.handleActivityKey(key)
	case tabQuota:
		return m, nil
	case tabKeeper:
		return m.handleKeeperKey(key)
	case tabSettings:
		return m.handleSettingsKey(key)
	}
	return m, nil
}

func (m *model) handleCustomKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	key := msg.String()
	switch key {
	case "esc":
		m.customOpen = false
		m.customErr = ""
	case "tab", "shift+tab":
		m.customField = 1 - m.customField
	case "enter":
		return m, m.applyCustomRange()
	case "backspace":
		if m.customField == 0 && len(m.customFrom) > 0 {
			m.customFrom = m.customFrom[:len(m.customFrom)-1]
		} else if m.customField == 1 && len(m.customTo) > 0 {
			m.customTo = m.customTo[:len(m.customTo)-1]
		}
	default:
		if len(key) == 1 && (key[0] >= '0' && key[0] <= '9' || key[0] == '-') {
			if m.customField == 0 && len(m.customFrom) < 10 {
				m.customFrom += key
			} else if m.customField == 1 && len(m.customTo) < 10 {
				m.customTo += key
			}
		}
	}
	return m, nil
}

func (m *model) handleFilterKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	key := msg.String()
	switch key {
	case "esc", "s":
		m.filterOpen = false
	case "a":
		for _, agent := range report.AllAgents {
			m.agents[agent] = true
		}
		m.invalidateUsage()
		return m, m.currentTabCmd()
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
			if m.agentCount() <= 1 {
				m.setStatus("At least one agent stays selected", 1)
				return m, nil
			}
			m.agents[agent] = false
		} else {
			m.agents[agent] = true
		}
		m.invalidateUsage()
		return m, m.currentTabCmd()
	}
	return m, nil
}

func (m *model) handleMouse(msg tea.MouseMsg) (tea.Model, tea.Cmd) {
	if msg.Action != tea.MouseActionPress || msg.Button != tea.MouseButtonLeft {
		return m, nil
	}
	if m.showHelp || m.filterOpen || m.customOpen {
		return m, nil
	}
	area := rect{x: 0, y: 0, w: m.frameWidth(), h: m.frameHeight()}
	header := rect{x: 0, y: 0, w: area.w, h: 4}
	tabs := rect{x: 0, y: header.h, w: area.w, h: 3}
	if msg.Y < tabs.y || msg.Y >= tabs.y+tabs.h {
		return m, nil
	}
	inner := tabs.inner()
	if msg.X < inner.x || msg.X >= inner.right() {
		return m, nil
	}
	index := (msg.X - inner.x) * tabCount / max(inner.w, 1)
	if index < 0 || index >= tabCount {
		return m, nil
	}
	if index == m.tab {
		return m, nil
	}
	m.switchTab(index - m.tab)
	return m, m.currentTabCmd()
}

func (m *model) recoveryTickCmd() tea.Cmd {
	if m.recovery == nil {
		return nil
	}
	return tea.Tick(250*time.Millisecond, func(time.Time) tea.Msg { return recoveryTickMsg{} })
}

func (m *model) refreshCmd() tea.Cmd {
	client := m.client
	if client == nil {
		return nil
	}
	return func() tea.Msg {
		result, err := client.Refresh(true, true)
		return refreshMsg{result: result, err: err}
	}
}

func (m *model) patchSettings(update map[string]any, message string) tea.Cmd {
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
		return configMsg{cfg: cfg, message: message}
	}
}

func (m *model) dayDetailKey(date string) string {
	return fmt.Sprintf("%s|%d", date, m.epoch)
}

func (m *model) toggleWebRemote() tea.Cmd {
	if m.webBusy || m.client == nil {
		return nil
	}
	if m.rtStatus.ManagedBy == "systemd" || m.rtStatus.ManagedBy == "launchd" {
		m.setToast("Web remote is managed by " + m.rtStatus.ManagedBy + "; use `tokenpulse service`")
		return nil
	}
	if m.client.IsRemote() && m.rtStatus.ManagedBy == "standalone" {
		m.setToast("Attached to local TUI; toggle Web remote from that session")
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
		message := "Web remote started"
		if status.WebURL != "" {
			message = "Web remote: " + status.WebURL
		}
		return webToggleMsg{client: next, status: status, toast: message}
	}
}

func (m *model) frameWidth() int {
	if m.width > 0 {
		return m.width
	}
	return 100
}

func (m *model) frameHeight() int {
	if m.height > 0 {
		return m.height
	}
	return 40
}

// View draws the full dashboard frame.
func (m *model) View() string {
	width, height := m.frameWidth(), m.frameHeight()
	c := newCanvas(width, height)
	area := rect{x: 0, y: 0, w: width, h: height}
	m.renderDashboard(c, area)
	return c.render()
}

func (m *model) renderDashboard(c *canvas, area rect) {
	headerHeight := min(4, area.h)
	tabsHeight := min(3, max(area.h-headerHeight, 0))
	footerHeight := min(3, max(area.h-headerHeight-tabsHeight, 0))
	bodyHeight := max(area.h-headerHeight-tabsHeight-footerHeight, 0)
	header := rect{x: 0, y: 0, w: area.w, h: headerHeight}
	tabs := rect{x: 0, y: headerHeight, w: area.w, h: tabsHeight}
	body := rect{x: 0, y: headerHeight + tabsHeight, w: area.w, h: bodyHeight}
	footer := rect{x: 0, y: area.h - footerHeight, w: area.w, h: footerHeight}

	m.drawHeader(c, header)
	m.drawTabs(c, tabs)
	m.drawPage(c, body)
	m.drawFooter(c, footer)

	if m.modelDetail != nil {
		m.drawModelDetail(c, area)
	}
	if m.dayDetail != nil {
		m.drawDayDetail(c, area)
	}
	if m.recovery.active() {
		m.drawRecoveryToast(c, area)
		if m.recovery.pulse() {
			c.tintBorder(textStyle{bg: rgb(6, 78, 59)})
		}
	} else if m.toast != "" && time.Since(m.toastAt) < 4*time.Second {
		m.drawToast(c, area)
	}
	if m.showHelp {
		m.drawHelp(c, area)
	}
	if m.filterOpen {
		m.drawFilter(c, area)
	}
}

func (m *model) drawPage(c *canvas, area rect) {
	switch m.tab {
	case tabOverview:
		m.drawOverview(c, area)
	case tabModels:
		m.drawModels(c, area)
	case tabDaily:
		m.drawDaily(c, area)
	case tabActivity:
		m.drawActivity(c, area)
	case tabQuota:
		m.drawQuota(c, area)
	case tabKeeper:
		m.drawKeeper(c, area)
	case tabSettings:
		m.drawSettings(c, area)
	}
}

func (m *model) drawHeader(c *canvas, area rect) {
	if area.h < 2 || area.w < 4 {
		return
	}
	theme := m.theme()
	c.box(area.x, area.y, area.w, area.h, "", textStyle{}, fgStyle(theme.border))
	inner := area.inner()
	y := inner.y
	title := "TokenPulse"
	c.set(inner.x, y, title, boldStyle(theme.accent))
	c.set(inner.x+len([]rune(title)), y, " Usage Analytics", boldStyle(theme.fg))
	badge := ""
	switch {
	case m.client != nil && m.client.IsRemote() && m.rtStatus.ManagedBy == "standalone":
		badge = "● local (attached)"
	case m.client != nil && m.client.IsRemote():
		badge = "● web"
	case m.client != nil:
		badge = "● local"
	}
	if badge != "" {
		start := max(inner.right()-len([]rune(badge))-1, inner.x)
		c.set(start, y, badge, fgStyle(theme.dim))
	}
	if inner.h < 2 {
		return
	}
	y++
	rangeText := "—"
	if m.report.FirstDay != nil {
		rangeText = *m.report.FirstDay + " → " + dateString(startOfToday())
	} else if m.reportLoaded {
		rangeText = m.report.Range.From + " → " + m.report.Range.To
	}
	c.set(inner.x, y, "Total Cost ", fgStyle(theme.dim))
	x := inner.x + len("Total Cost ")
	cost := formatCost(m.report.Totals.Cost)
	c.set(x, y, cost, boldStyle(theme.accent))
	x += len([]rune(cost)) + 2
	c.set(x, y, "Total Tokens ", fgStyle(theme.dim))
	x += len("Total Tokens ")
	tokens := formatIntCommas(m.report.Totals.Tokens)
	c.set(x, y, tokens, boldStyle(theme.codex))
	x += len([]rune(tokens)) + 2
	c.set(x, y, "Data Range ", fgStyle(theme.dim))
	x += len("Data Range ")
	c.set(x, y, rangeText, fgStyle(theme.fg))
	x += len([]rune(rangeText)) + 2
	if m.agentCount() > 1 {
		c.set(x, y, "Agents ", fgStyle(theme.dim))
		x += len("Agents ")
		c.set(x, y, fmt.Sprintf("%d", m.agentCount()), boldStyle(theme.accentSoft))
	}
}

func (m *model) drawTabs(c *canvas, area rect) {
	if area.h < 2 || area.w < 4 {
		return
	}
	theme := m.theme()
	c.box(area.x, area.y, area.w, area.h, "", textStyle{}, fgStyle(theme.border))
	inner := area.inner()
	columns := splitH(inner, tabCount)
	for index, name := range tabNames {
		column := columns[index]
		style := fgStyle(theme.dim)
		if index == m.tab {
			style = textStyle{fg: theme.onAccent, bg: theme.accent, bold: true}
			c.fill(column.x, column.y, column.w, column.h, style)
		}
		label := truncate(name, max(column.w-1, 1))
		start := column.x + max((column.w-len([]rune(label)))/2, 0)
		c.set(start, column.y, label, style)
	}
}

func (m *model) drawFooter(c *canvas, area rect) {
	if area.h < 1 || area.w < 2 {
		return
	}
	theme := m.theme()
	for x := area.x; x < area.right(); x++ {
		c.set(x, area.y, "─", fgStyle(theme.border))
	}
	if area.h < 2 {
		return
	}
	help := m.helpLine()
	c.set(area.x+1, area.y+1, truncate(help, area.w-2), fgStyle(theme.dim))
	if area.h >= 3 {
		info := m.infoLine()
		available := area.w - 2
		status := ""
		statusColor := theme.accent
		if m.status != "" && time.Since(m.statusAt) < 10*time.Second {
			status = truncate(m.status, max(area.w/2, 10))
			if m.statusLevel == 1 {
				statusColor = theme.gaugeMid
			} else if m.statusLevel == 2 {
				statusColor = theme.gaugeHigh
			}
			available = max(available-len([]rune(status))-2, 1)
		}
		c.set(area.x+1, area.y+2, truncate(info, available), fgStyle(theme.dim))
		if status != "" {
			start := area.right() - 1 - len([]rune(status))
			if start > area.x+1+available {
				c.set(start, area.y+2, status, boldStyle(statusColor))
			}
		}
	}
}

func (m *model) helpLine() string {
	switch m.tab {
	case tabOverview:
		return fmt.Sprintf(" q quit  r refresh  ←→ tab  ↑↓ select  t/c metric (%s)  s filter  ? help", metricLabel(m.overviewMetric))
	case tabModels:
		return fmt.Sprintf(" q quit  r refresh  ←→ tab  ↑↓ select  c/t/d sort (%s %s)  s filter  ? help", sortLabel(m.sortField), sortDirection(m.sortAsc))
	case tabDaily:
		return fmt.Sprintf(" q quit  r refresh  ←→ tab  ↑↓ select  n today  c/t/d sort (%s %s)  v metric (%s)  s filter  ? help", sortLabel(m.sortField), sortDirection(m.sortAsc), metricLabel(m.dailyMetric))
	case tabActivity:
		return fmt.Sprintf(" q quit  r refresh  ←→ tab  n today  [/] detail  t/c metric (%s)  s filter  ? help", metricLabel(m.activityMetric))
	case tabQuota:
		return " q quit  r refresh  ←→ tab  ? help"
	case tabKeeper:
		return " q quit  ↑↓/Tab select  1/d toggle 5h  2/w toggle weekly  p ping now  e engine  ? help"
	case tabSettings:
		return " q quit  r refresh  ←→ tab  ↑↓ select  space/enter change  J/K reorder provider  ? help"
	}
	return " q quit"
}

func (m *model) infoLine() string {
	rangeText := "Data: —"
	if m.reportLoaded {
		rangeText = "Data: " + m.report.Range.From + " → " + m.report.Range.To
	}
	refresh := ""
	at := m.refreshedAt
	if at.IsZero() && m.report.ScannedAt != 0 {
		at = time.UnixMilli(m.report.ScannedAt)
	}
	if !at.IsZero() {
		refresh = "Refreshed: " + at.Format("15:04:05")
	}
	if seconds := m.cfg.Display.AutoRefreshSecs; seconds > 0 && !at.IsZero() {
		remaining := int(seconds) - int(time.Since(at).Seconds())
		if remaining < 0 {
			remaining = 0
		}
		refresh += fmt.Sprintf("  |  Auto-refresh in: %dm %ds", remaining/60, remaining%60)
	}
	if refresh == "" {
		return rangeText
	}
	return rangeText + "  |  " + refresh
}

func metricLabel(metric int) string {
	if metric == metricCost {
		return "cost"
	}
	return "tokens"
}

func sortLabel(field int) string {
	switch field {
	case sortCost:
		return "cost"
	case sortTokens:
		return "tokens"
	default:
		return "date"
	}
}

func sortDirection(ascending bool) string {
	if ascending {
		return "↑"
	}
	return "↓"
}

func (m *model) drawToast(c *canvas, area rect) {
	theme := m.theme()
	width, height := 46, 3
	box := rect{
		x: max(area.right()-width-2, area.x),
		y: max(area.bottom()-height-2, area.y),
		w: min(width, area.w),
		h: min(height, area.h),
	}
	if box.w < 10 || box.h < 3 {
		return
	}
	c.fill(box.x, box.y, box.w, box.h, textStyle{})
	c.box(box.x, box.y, box.w, box.h, "", textStyle{}, fgStyle(theme.accent))
	c.set(box.x+2, box.y+1, truncate(m.toast, box.w-4), fgStyle(theme.fg))
}

func (m *model) drawRecoveryToast(c *canvas, area rect) {
	width, height := 46, 5
	box := rect{
		x: max(area.right()-width-2, area.x),
		y: max(area.bottom()-height-2, area.y),
		w: min(width, area.w),
		h: min(height, area.h),
	}
	if box.w < 10 || box.h < 5 {
		return
	}
	accent := rgb(52, 211, 153)
	c.fill(box.x, box.y, box.w, box.h, textStyle{})
	c.box(box.x, box.y, box.w, box.h, " Quota Restored ", boldStyle(accent), fgStyle(accent))
	icon := []string{"●", "◉", "⦿", "✦"}[int(time.Since(m.recovery.started).Milliseconds()/250)%4]
	y := box.y + 1
	line := icon + " "
	if m.recovery.provider != "" {
		line += displayAgentName(m.recovery.provider)
	}
	if m.recovery.window != "" {
		line += " (" + m.recovery.window + ")"
	}
	c.set(box.x+2, y, truncate(line, box.w-4), boldStyle(accent))
	ratio := 1 - time.Since(m.recovery.started).Seconds()/5
	if ratio < 0 {
		ratio = 0
	}
	filled := int(ratio*16 + 0.5)
	bar := strings.Repeat("█", filled) + strings.Repeat("░", 16-filled)
	remaining := time.Until(m.recovery.started.Add(5 * time.Second)).Seconds()
	if remaining < 0 {
		remaining = 0
	}
	line2 := fmt.Sprintf("%.0f%% remaining  [%s] %.1fs", m.recovery.remaining, bar, remaining)
	c.set(box.x+2, y+1, truncate(line2, box.w-4), fgStyle(accent))
}

// renderHeader keeps the attach test's readable header contract.
func (m *model) renderHeader(width int) string {
	c := newCanvas(width, 4)
	m.drawHeader(c, rect{x: 0, y: 0, w: width, h: 4})
	return strings.Join(c.lines(), "\n")
}

func (m *model) renderSettings(width int) string {
	c := newCanvas(width, 30)
	m.drawSettings(c, rect{x: 0, y: 0, w: width, h: 30})
	return strings.Join(c.lines(), "\n")
}

func (m *model) renderQuota(width int) string {
	c := newCanvas(width, 40)
	m.drawQuota(c, rect{x: 0, y: 0, w: width, h: 40})
	return strings.Join(c.lines(), "\n")
}

func (m *model) renderKeeper(width int) string {
	c := newCanvas(width, 40)
	m.drawKeeper(c, rect{x: 0, y: 0, w: width, h: 40})
	return strings.Join(c.lines(), "\n")
}

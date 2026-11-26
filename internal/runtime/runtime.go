// Package runtime owns TokenPulse's single-writer refresh loop, quota cache,
// Keeper scheduler, and event bus shared by the TUI and remote server.
package runtime

import (
	"errors"
	"log"
	"os"
	"runtime/debug"
	"sync"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/notification"
	"github.com/fangwangme/tokenpulse/internal/pricing"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/quota"
	"github.com/fangwangme/tokenpulse/internal/report"
	"github.com/fangwangme/tokenpulse/internal/scan"
)

const ManualRefreshInterval = 60 * time.Second

type QuotaError struct {
	Message string    `json:"message"`
	At      time.Time `json:"at"`
}

type QuotaProviderView struct {
	Provider string                  `json:"provider"`
	Enabled  bool                    `json:"enabled"`
	Snapshot *provider.QuotaSnapshot `json:"snapshot"`
	Error    *QuotaError             `json:"error"`
}

type QuotaView struct {
	Providers []QuotaProviderView `json:"providers"`
	FetchedAt int64               `json:"fetchedAt"`
}

type RefreshResult struct {
	Accepted   bool  `json:"accepted"`
	Throttled  bool  `json:"throttled"`
	RetryAfter int   `json:"retryAfter"`
	ScannedAt  int64 `json:"scannedAt"`
	FetchedAt  int64 `json:"fetchedAt"`
}

type StatusSnapshot struct {
	Running   bool   `json:"running"`
	ScannedAt int64  `json:"scannedAt"`
	FetchedAt int64  `json:"fetchedAt"`
	ManagedBy string `json:"managedBy,omitempty"`
	WebURL    string `json:"webUrl,omitempty"`
	WebError  string `json:"webError,omitempty"`
}

type refreshFlags struct {
	usage bool
	quota bool
}

// Runtime is the single logical writer and scheduler for one data directory.
type Runtime struct {
	DataDir         string
	Config          *config.Manager
	Scanner         *scan.Scanner
	Report          *report.Service
	Pricing         *pricing.Service
	Cache           *quota.Cache
	Keeper          *keeper.Engine
	Events          *Bus
	ManagedBy       string
	Logger          *log.Logger
	OnQuotaRecovery func(title, body string)

	mu               sync.Mutex
	snapshots        map[string]provider.QuotaSnapshot
	quotaErrors      map[string]QuotaError
	exhausted        map[string]bool
	lastScannedAt    int64
	lastFetchedAt    time.Time
	lastUsageRefresh time.Time
	lastQuotaRefresh time.Time
	lastManualUsage  time.Time
	lastManualQuota  time.Time
	running          bool
	webURL           string
	webError         string

	firstScan  chan struct{}
	firstOnce  sync.Once
	stop       chan struct{}
	stopOnce   sync.Once
	stopMu     sync.Mutex
	stopping   bool
	startOnce  sync.Once
	wake       chan struct{}
	refreshMu  sync.Mutex
	settingsMu sync.Mutex
	bgWG       sync.WaitGroup
	execWG     sync.WaitGroup
}

// StartOptions controls background refresh scheduling when Start is called.
type StartOptions struct {
	InitialRefresh bool
	AutoRefresh    bool
}

// New constructs a Runtime wired to dataDir. Call WarmCache and Start to begin
// background refreshes.
func New(dataDir string, manager *config.Manager, scanner *scan.Scanner, managedBy string, logger *log.Logger) (*Runtime, error) {
	cfg, err := manager.Load()
	if err != nil {
		return nil, err
	}
	if managedBy == "" {
		managedBy = os.Getenv("TOKENPULSE_MANAGED_BY")
	}
	if managedBy == "" {
		managedBy = "standalone"
	}
	return &Runtime{
		DataDir:     dataDir,
		Config:      manager,
		Scanner:     scanner,
		Report:      scanner.Report,
		Pricing:     scanner.Pricing,
		Cache:       quota.NewCache(dataDir),
		Keeper:      keeper.NewEngine(cfg.Keeper, manager.KeeperStatePath()),
		Events:      NewBus(),
		ManagedBy:   managedBy,
		Logger:      logger,
		snapshots:   map[string]provider.QuotaSnapshot{},
		quotaErrors: map[string]QuotaError{},
		exhausted:   map[string]bool{},
		firstScan:   make(chan struct{}),
		stop:        make(chan struct{}),
		wake:        make(chan struct{}, 1),
	}, nil
}

// SetWebState records the active web pairing URL or listener error.
func (r *Runtime) SetWebState(url, webErr string) {
	r.mu.Lock()
	r.webURL = url
	r.webError = webErr
	r.mu.Unlock()
}

// MarkScanned records scan/fetch timestamps and unblocks WaitFirstScan.
func (r *Runtime) MarkScanned(scannedAt int64, fetchedAt time.Time) {
	r.mu.Lock()
	r.lastScannedAt = scannedAt
	r.lastFetchedAt = fetchedAt
	r.mu.Unlock()
	r.firstOnce.Do(func() { close(r.firstScan) })
}

// WarmCache loads persisted usage scan timestamps and quota snapshots so
// frontends can render immediately before the first background refresh finishes.
func (r *Runtime) WarmCache() {
	if scanned, err := r.Report.ScannedAtValue(); err == nil && scanned > 0 {
		r.mu.Lock()
		r.lastScannedAt = scanned
		r.mu.Unlock()
	}
	for _, id := range quota.ProviderIDs {
		cached, err := r.Cache.LoadAny(id)
		if err != nil || cached == nil {
			continue
		}
		snapshot := cached.Snapshot
		quota.Normalize(&snapshot)
		r.mu.Lock()
		r.snapshots[id] = snapshot
		if snapshot.FetchedAt.After(r.lastFetchedAt) {
			r.lastFetchedAt = snapshot.FetchedAt
		}
		r.mu.Unlock()
	}
}

// Start launches the background refresh and Keeper loops.
func (r *Runtime) Start(opts StartOptions) {
	r.startOnce.Do(func() {
		if !opts.InitialRefresh {
			r.firstOnce.Do(func() { close(r.firstScan) })
		}
		if opts.AutoRefresh || opts.InitialRefresh {
			r.bgWG.Add(1)
			go func() {
				defer r.bgWG.Done()
				r.refreshLoop(opts)
			}()
		}
		r.bgWG.Add(1)
		go func() {
			defer r.bgWG.Done()
			r.keeperLoop()
		}()
	})
}

func (r *Runtime) beginTask() bool {
	r.stopMu.Lock()
	defer r.stopMu.Unlock()
	if r.stopping {
		return false
	}
	r.execWG.Add(1)
	return true
}

// Stop stops accepting new refresh/keeper tasks, terminates background loops,
// and waits for every in-flight refresh and Keeper execution to finish before
// returning, so callers can safely release runtime.lock afterwards.
func (r *Runtime) Stop() {
	r.stopMu.Lock()
	r.stopping = true
	r.stopOnce.Do(func() {
		close(r.stop)
	})
	r.stopMu.Unlock()
	r.bgWG.Wait()
	r.execWG.Wait()
}

// WaitFirstScan blocks until either a persisted scan exists, the first refresh
// finishes, Stop is called, or timeout elapses.
func (r *Runtime) WaitFirstScan(timeout time.Duration) {
	r.mu.Lock()
	scanned := r.lastScannedAt
	r.mu.Unlock()
	if scanned > 0 {
		return
	}
	select {
	case <-r.firstScan:
	case <-r.stop:
	case <-time.After(timeout):
	}
}

func (r *Runtime) Status() StatusSnapshot {
	r.mu.Lock()
	defer r.mu.Unlock()
	return StatusSnapshot{
		Running:   r.running,
		ScannedAt: r.lastScannedAt,
		FetchedAt: fetchedAtMs(r.lastFetchedAt),
		ManagedBy: r.ManagedBy,
		WebURL:    r.webURL,
		WebError:  r.webError,
	}
}

func (r *Runtime) Usage(params report.Params) (report.Report, error) {
	value, err := r.Report.Report(params)
	if err != nil {
		return report.Report{}, err
	}
	value.PricesAvailable = r.Pricing.Available()
	return value, nil
}

func (r *Runtime) UsageDay(date string, agents []string) (report.DayDetail, error) {
	return r.Report.Day(date, agents)
}

func (r *Runtime) UsageModel(model, from, to string, agents []string) (report.ModelDetail, error) {
	return r.Report.Model(model, from, to, agents)
}

func (r *Runtime) Quota() QuotaView {
	cfg, _ := r.Config.Load()
	order := cfg.QuotaProviderOrder(quota.ProviderIDs)
	enabled := map[string]bool{}
	for _, id := range cfg.EnabledQuotaProviders(order) {
		enabled[id] = true
	}
	r.mu.Lock()
	snapshots := make(map[string]provider.QuotaSnapshot, len(r.snapshots))
	for id, snapshot := range r.snapshots {
		snapshots[id] = snapshot
	}
	failures := make(map[string]QuotaError, len(r.quotaErrors))
	for id, failure := range r.quotaErrors {
		failures[id] = failure
	}
	fetchedAt := r.lastFetchedAt
	r.mu.Unlock()

	providers := make([]QuotaProviderView, 0, len(order))
	for _, id := range order {
		view := QuotaProviderView{Provider: id, Enabled: enabled[id]}
		if snapshot, ok := snapshots[id]; ok {
			copy := snapshot
			quota.Normalize(&copy)
			view.Snapshot = &copy
		}
		if failure, ok := failures[id]; ok {
			copy := failure
			view.Error = &copy
		}
		providers = append(providers, view)
	}
	return QuotaView{Providers: providers, FetchedAt: fetchedAtMs(fetchedAt)}
}

func (r *Runtime) KeeperSnapshots() []provider.QuotaSnapshot {
	r.mu.Lock()
	defer r.mu.Unlock()
	snapshots := make([]provider.QuotaSnapshot, 0, len(r.snapshots))
	for _, snapshot := range r.snapshots {
		snapshots = append(snapshots, snapshot)
	}
	return snapshots
}

func (r *Runtime) KeeperRecords(limit int) []keeper.ExecutionRecord {
	if r.Keeper == nil || r.Keeper.History == nil {
		return nil
	}
	return r.Keeper.History.Recent(limit)
}

// RequestRefresh runs a synchronous refresh subject to the 60-second per-target
// manual cooldown.
func (r *Runtime) RequestRefresh(usage, quotaWanted bool) RefreshResult {
	if !r.beginTask() {
		st := r.Status()
		return RefreshResult{ScannedAt: st.ScannedAt, FetchedAt: st.FetchedAt}
	}
	defer r.execWG.Done()
	now := time.Now()
	r.mu.Lock()
	lastUsage := r.lastManualUsage
	if r.lastUsageRefresh.After(lastUsage) {
		lastUsage = r.lastUsageRefresh
	}
	lastQuota := r.lastManualQuota
	if r.lastQuotaRefresh.After(lastQuota) {
		lastQuota = r.lastQuotaRefresh
	}
	allowUsage, allowQuota, retryAfter := RefreshGate(now, lastUsage, lastQuota, usage, quotaWanted)
	if allowUsage {
		r.lastManualUsage = now
	}
	if allowQuota {
		r.lastManualQuota = now
	}
	scannedAt := r.lastScannedAt
	fetchedAt := r.lastFetchedAt
	r.mu.Unlock()

	accepted := allowUsage || allowQuota
	if accepted {
		r.refresh(refreshFlags{usage: allowUsage, quota: allowQuota}, now)
		done := time.Now()
		r.mu.Lock()
		if allowUsage {
			r.lastManualUsage = done
		}
		if allowQuota {
			r.lastManualQuota = done
		}
		scannedAt = r.lastScannedAt
		fetchedAt = r.lastFetchedAt
		r.mu.Unlock()
	}
	return RefreshResult{
		Accepted:   accepted,
		Throttled:  (usage && !allowUsage) || (quotaWanted && !allowQuota),
		RetryAfter: retryAfter,
		ScannedAt:  scannedAt,
		FetchedAt:  fetchedAtMs(fetchedAt),
	}
}

// RefreshGate allows one manual refresh per target per minute.
func RefreshGate(now, lastUsage, lastQuota time.Time, usage, quotaWanted bool) (bool, bool, int) {
	allowUsage := usage && (lastUsage.IsZero() || now.Sub(lastUsage) >= ManualRefreshInterval)
	allowQuota := quotaWanted && (lastQuota.IsZero() || now.Sub(lastQuota) >= ManualRefreshInterval)
	retry := 0
	if usage && !allowUsage {
		retry = max(retry, secondsUntil(lastUsage.Add(ManualRefreshInterval), now))
	}
	if quotaWanted && !allowQuota {
		retry = max(retry, secondsUntil(lastQuota.Add(ManualRefreshInterval), now))
	}
	return allowUsage, allowQuota, retry
}

func secondsUntil(deadline, now time.Time) int {
	seconds := int(deadline.Sub(now).Seconds())
	if float64(seconds) < deadline.Sub(now).Seconds() {
		seconds++
	}
	return max(seconds, 1)
}

func (r *Runtime) WakeRefreshLoop() {
	select {
	case r.wake <- struct{}{}:
	default:
	}
}

// UpdateSettings serializes Load -> mutate -> atomic Save -> notify under
// settingsMu so concurrent phone and TUI edits cannot clobber each other.
func (r *Runtime) UpdateSettings(mutate func(*config.Config) error, refreshIntervalChanged bool) (config.Config, error) {
	if !r.beginTask() {
		return config.Config{}, errors.New("runtime is stopping")
	}
	defer r.execWG.Done()
	r.settingsMu.Lock()
	defer r.settingsMu.Unlock()
	cfg, err := r.Config.Load()
	if err != nil {
		return config.Config{}, err
	}
	if err := mutate(&cfg); err != nil {
		return config.Config{}, err
	}
	if err := r.Config.Save(&cfg); err != nil {
		return config.Config{}, err
	}
	r.NotifySettingsUpdated(cfg, refreshIntervalChanged)
	return cfg, nil
}

func (r *Runtime) NotifySettingsUpdated(cfg config.Config, refreshIntervalChanged bool) {
	if r.Keeper != nil {
		r.Keeper.UpdateConfig(cfg.Keeper)
	}
	if refreshIntervalChanged {
		r.WakeRefreshLoop()
	}
	r.Events.Publish("settings_updated", map[string]any{})
}

func (r *Runtime) TriggerKeeperPing(agent, group string) (keeper.Request, error) {
	if !r.beginTask() {
		return keeper.Request{}, errors.New("runtime is stopping")
	}
	request, err := r.Keeper.Manual(agent, group)
	if err != nil {
		r.execWG.Done()
		return keeper.Request{}, err
	}
	go func() {
		defer r.execWG.Done()
		record := r.Keeper.Execute(request)
		r.Keeper.Finish(request, record)
		r.Events.Publish("keeper_updated", map[string]any{"agent": record.Agent})
	}()
	return request, nil
}

func (r *Runtime) Reprice(apply, yes bool, expect string) (*pricing.RepriceReport, error) {
	if apply && !yes {
		return nil, errors.New("--apply requires --yes")
	}
	if !r.beginTask() {
		return nil, errors.New("runtime is stopping")
	}
	defer r.execWG.Done()
	r.refreshMu.Lock()
	defer r.refreshMu.Unlock()
	if err := r.Pricing.Load(false); err != nil {
		return nil, err
	}
	rep, err := r.Pricing.Reprice(apply, expect)
	if err != nil {
		return nil, err
	}
	r.Report.Clear()
	if scanned, err := r.Report.ScannedAtValue(); err == nil {
		r.mu.Lock()
		r.lastScannedAt = scanned
		r.mu.Unlock()
		r.Events.Publish("usage_updated", map[string]any{"scannedAt": scanned})
	}
	return &rep, nil
}

func (r *Runtime) refreshLoop(opts StartOptions) {
	if opts.InitialRefresh {
		select {
		case <-r.stop:
			return
		default:
		}
		r.refresh(refreshFlags{usage: true, quota: true}, time.Now())
	}
	if !opts.AutoRefresh {
		return
	}
	firstDelay := r.refreshInterval()
	if firstDelay <= 0 {
		firstDelay = 24 * time.Hour
	}
	timer := time.NewTimer(firstDelay)
	defer timer.Stop()
	for {
		select {
		case <-r.stop:
			return
		case <-r.wake:
			next := r.refreshInterval()
			if next <= 0 {
				next = 24 * time.Hour
			}
			resetTimer(timer, next)
		case <-timer.C:
			if interval := r.refreshInterval(); interval > 0 {
				r.refresh(refreshFlags{usage: true, quota: true}, time.Now())
			}
			next := r.refreshInterval()
			if next <= 0 {
				next = 24 * time.Hour
			}
			resetTimer(timer, next)
		}
	}
}

func resetTimer(timer *time.Timer, d time.Duration) {
	if !timer.Stop() {
		select {
		case <-timer.C:
		default:
		}
	}
	timer.Reset(d)
}

func (r *Runtime) refreshInterval() time.Duration {
	cfg, err := r.Config.Load()
	if err == nil {
		if cfg.Display.AutoRefreshSecs == 0 && r.ManagedBy == "standalone" {
			return 0
		}
		if cfg.Display.AutoRefreshSecs > 0 {
			return time.Duration(cfg.Display.AutoRefreshSecs) * time.Second
		}
	}
	return 900 * time.Second
}

func (r *Runtime) keeperLoop() {
	for {
		interval := 60 * time.Second
		if cfg, err := r.Config.Load(); err == nil && cfg.Keeper.CheckIntervalSecs > 0 {
			interval = time.Duration(cfg.Keeper.CheckIntervalSecs) * time.Second
		}
		if interval < 10*time.Second {
			interval = 10 * time.Second
		}
		timer := time.NewTimer(interval)
		select {
		case <-r.stop:
			timer.Stop()
			return
		case <-timer.C:
			r.runKeeperChecks()
		}
	}
}

func (r *Runtime) runKeeperChecks() {
	cfg, err := r.Config.Load()
	if err != nil || !cfg.Keeper.Enabled {
		return
	}
	r.Keeper.UpdateConfig(cfg.Keeper)
	snapshots := r.KeeperSnapshots()
	now := time.Now()
	for _, request := range r.Keeper.CheckScheduled(snapshots, now) {
		r.startKeeper(request)
	}
	for _, request := range r.Keeper.CheckQuota(snapshots, now) {
		r.startKeeper(request)
	}
}

func (r *Runtime) refresh(flags refreshFlags, requested time.Time) {
	r.refreshMu.Lock()
	defer r.refreshMu.Unlock()
	r.mu.Lock()
	if flags.usage && (r.lastUsageRefresh.After(requested) || time.Since(r.lastUsageRefresh) < ManualRefreshInterval) {
		flags.usage = false
	}
	if flags.quota && (r.lastQuotaRefresh.After(requested) || time.Since(r.lastQuotaRefresh) < ManualRefreshInterval) {
		flags.quota = false
	}
	r.mu.Unlock()
	if flags.usage || flags.quota {
		r.doRefresh(flags)
	}
}

func (r *Runtime) doRefresh(flags refreshFlags) {
	r.mu.Lock()
	r.running = true
	r.mu.Unlock()
	defer func() {
		r.mu.Lock()
		r.running = false
		r.mu.Unlock()
	}()
	if flags.usage {
		defer r.firstOnce.Do(func() { close(r.firstScan) })
	}

	cfg, err := r.Config.Load()
	if err != nil {
		cfg = config.Default()
	}
	r.Keeper.UpdateConfig(cfg.Keeper)
	if r.Logger != nil {
		r.Logger.Printf("refresh start usage=%v quota=%v", flags.usage, flags.quota)
	}
	var wg sync.WaitGroup
	if flags.usage {
		wg.Add(1)
		go func() {
			defer wg.Done()
			defer func() {
				r.mu.Lock()
				r.lastUsageRefresh = time.Now()
				r.mu.Unlock()
			}()
			if err := r.Scanner.Run(scan.Options{
				Providers: scan.ProvidersFromConfig(cfg), ScanAntigravity: cfg.Display.ScanAntigravity,
			}); err != nil {
				if r.Logger != nil {
					r.Logger.Printf("usage refresh failed: %v", err)
				}
				return
			}
			if scanned, err := r.Report.ScannedAtValue(); err == nil {
				r.mu.Lock()
				r.lastScannedAt = scanned
				r.mu.Unlock()
				r.Events.Publish("usage_updated", map[string]any{"scannedAt": scanned})
			}
		}()
	}
	if flags.quota && cfg.Display.RefreshQuota {
		wg.Add(1)
		go func() {
			defer wg.Done()
			defer func() {
				r.mu.Lock()
				r.lastQuotaRefresh = time.Now()
				r.mu.Unlock()
			}()
			results := quota.FetchAll(r.DataDir, FetchersFor(cfg))
			r.ApplyQuotaResults(results)
		}()
	}
	wg.Wait()
	if flags.usage {
		debug.FreeOSMemory()
	}
	r.runKeeperChecks()
	if r.Logger != nil {
		r.Logger.Printf("refresh complete")
	}
}

// FetchersFor builds the configured read-only quota fetchers.
func FetchersFor(cfg config.Config) []quota.Fetcher {
	var fetchers []quota.Fetcher
	order := cfg.QuotaProviderOrder(quota.ProviderIDs)
	for _, id := range cfg.EnabledQuotaProviders(order) {
		switch id {
		case "claude":
			fetchers = append(fetchers, quota.NewClaudeFetcher(cfg.Display.AccountDisplay.ShowsAccount()))
		case "codex":
			fetchers = append(fetchers, quota.NewCodexFetcher())
		case "opencode":
			fetchers = append(fetchers, quota.NewOpenCodeFetcher(cfg.Display.AccountDisplay.ShowsAccount()))
		case "antigravity":
			fetchers = append(fetchers, quota.NewAntigravityFetcher())
		}
	}
	return fetchers
}

func (r *Runtime) ApplyQuotaResults(results []quota.FetchResult) {
	r.mu.Lock()
	var recoveries []notification.Recovery
	for _, result := range results {
		if result.Snapshot != nil {
			r.snapshots[result.Provider] = *result.Snapshot
			delete(r.quotaErrors, result.Provider)
			for _, window := range result.Snapshot.Windows {
				key := result.Provider + ":" + window.ID
				if window.UsedPercent >= 100 {
					r.exhausted[key] = true
				} else if r.exhausted[key] {
					delete(r.exhausted, key)
					recoveries = append(recoveries, notification.Recovery{
						Provider: result.Provider, WindowLabel: window.Label,
						RemainingPercent: maxFloat(100-window.UsedPercent, 0),
					})
				}
			}
		}
		if result.Err != nil {
			r.quotaErrors[result.Provider] = QuotaError{Message: result.Err.Error(), At: result.FetchedAt}
		}
	}
	if len(results) > 0 {
		r.lastFetchedAt = time.Now().UTC()
	}
	fetchedAt := r.lastFetchedAt
	r.mu.Unlock()

	for _, recovery := range recoveries {
		r.Events.Publish("quota_recovered", map[string]any{
			"provider": recovery.Provider, "window": recovery.WindowLabel,
			"remaining": recovery.RemainingPercent,
		})
	}
	if len(recoveries) > 0 {
		cfg, _ := r.Config.Load()
		if cfg.Display.NotificationLevel != config.NotificationOff {
			notification.NotifyQuotaRestored(cfg.Display.NotificationLevel, cfg.Display.NotificationSound, recoveries)
			if r.OnQuotaRecovery != nil {
				r.OnQuotaRecovery("TokenPulse", notification.Summarize(recoveries))
			}
		}
	}
	r.Events.Publish("quota_updated", map[string]any{"fetchedAt": fetchedAtMs(fetchedAt)})
}

func (r *Runtime) startKeeper(request keeper.Request) {
	if !r.beginTask() {
		return
	}
	go func() {
		defer r.execWG.Done()
		record := r.Keeper.Execute(request)
		r.Keeper.Finish(request, record)
		r.Events.Publish("keeper_updated", map[string]any{"agent": record.Agent})
	}()
}

func fetchedAtMs(value time.Time) int64 {
	if value.IsZero() {
		return 0
	}
	return value.UnixMilli()
}

func maxFloat(a, b float64) float64 {
	if a > b {
		return a
	}
	return b
}

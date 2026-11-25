package remote

import (
	"crypto/subtle"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/gorilla/websocket"
	qrcode "github.com/skip2/go-qrcode"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/notification"
	"github.com/fangwangme/tokenpulse/internal/paths"
	"github.com/fangwangme/tokenpulse/internal/pricing"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/quota"
	"github.com/fangwangme/tokenpulse/internal/report"
	"github.com/fangwangme/tokenpulse/internal/scan"
	"github.com/fangwangme/tokenpulse/internal/version"
	"github.com/fangwangme/tokenpulse/web"
)

type Options struct {
	Port        int
	HTTPSPort   int
	NoTailscale bool
	LAN         bool
	WebDir      string
}

type quotaError struct {
	Message string    `json:"message"`
	At      time.Time `json:"at"`
}

type Server struct {
	DataDir string
	Options Options
	Access  Access
	State   *StateStore
	Config  *config.Manager
	Scanner *scan.Scanner
	Report  *report.Service
	Pricing *pricing.Service
	Cache   *quota.Cache
	Keeper  *keeper.Engine
	Push    *pushStore
	WebDir  string

	mu            sync.Mutex
	snapshots     map[string]provider.QuotaSnapshot
	quotaErrors   map[string]quotaError
	lastScannedAt int64
	lastFetchedAt time.Time
	exhausted     map[string]bool
	refreshCh     chan refreshFlags
	running       bool
	lastRequested time.Time
	firstScan     chan struct{}
	firstOnce     sync.Once
	wsClients     map[*websocket.Conn]bool
	stop          chan struct{}
	logger        *log.Logger
}

type refreshFlags struct {
	usage bool
	quota bool
}

func NewServer(opts Options) (*Server, error) {
	dataDir, err := paths.DataDir()
	if err != nil {
		return nil, err
	}
	if version.IsTest() {
		// A test run never binds or configures the production ports, even if
		// a caller passed the production value explicitly.
		if opts.Port == 0 || opts.Port == 4120 {
			opts.Port = 14120
		}
		if opts.HTTPSPort == 0 || opts.HTTPSPort == 8444 {
			opts.HTTPSPort = 18444
		}
	}
	if opts.Port == 0 {
		opts.Port = 4120
	}
	if opts.HTTPSPort == 0 {
		opts.HTTPSPort = 8444
	}
	manager, err := config.New()
	if err != nil {
		return nil, err
	}
	cfg, err := manager.Load()
	if err != nil {
		return nil, err
	}
	scanner, err := scan.New(dataDir)
	if err != nil {
		return nil, err
	}
	logDir := filepath.Join(dataDir, "log")
	_ = os.MkdirAll(logDir, 0o755)
	logFile, _ := os.OpenFile(filepath.Join(logDir, "tokenpulse-"+time.Now().Format("2006-01-02")+".log"),
		os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	server := &Server{
		DataDir: dataDir, Options: opts, State: NewStateStore(dataDir), Config: manager,
		Scanner: scanner, Report: scanner.Report, Pricing: scanner.Pricing,
		Cache: quota.NewCache(dataDir), Keeper: keeper.NewEngine(cfg.Keeper, manager.KeeperStatePath()),
		Push: newPushStore(dataDir), WebDir: opts.WebDir,
		snapshots: map[string]provider.QuotaSnapshot{}, quotaErrors: map[string]quotaError{},
		exhausted: map[string]bool{}, refreshCh: make(chan refreshFlags, 1),
		firstScan: make(chan struct{}), wsClients: map[*websocket.Conn]bool{}, stop: make(chan struct{}),
	}
	if logFile != nil {
		server.logger = log.New(logFile, "", log.LstdFlags)
	}
	return server, nil
}

// Run starts the server and blocks until the process is stopped.
func (s *Server) Run() error {
	if _, err := s.State.Load(); err != nil {
		return err
	}
	release, err := s.acquireLock()
	if err != nil {
		return err
	}
	defer release()
	socket, err := s.listenLocalSocket()
	if err != nil {
		return err
	}
	defer socket.Close()
	defer os.Remove(filepath.Join(s.DataDir, "serve.sock"))

	s.Access = buildAccess(s.Options.Port, s.Options.HTTPSPort, s.Options.NoTailscale, s.Options.LAN)
	_ = os.WriteFile(filepath.Join(s.DataDir, "serve.url"), []byte(s.Access.URL), 0o600)
	listener, err := net.Listen("tcp", net.JoinHostPort(s.Access.BindHost, strconv.Itoa(s.Options.Port)))
	if err != nil {
		return err
	}
	defer listener.Close()

	token, err := s.State.Token()
	if err != nil {
		return err
	}
	s.printBanner(token)
	go s.refreshLoop()
	go s.serveUnix(socket)

	server := &http.Server{Handler: s.rootHandler()}
	return server.Serve(listener)
}

func (s *Server) printBanner(token string) {
	urlWithToken := strings.TrimSuffix(s.Access.URL, "/") + "/#token=" + token
	fmt.Printf("TokenPulse %s\n", version.String())
	fmt.Printf("Access: %s\n", s.Access.Mode)
	fmt.Printf("Pairing URL: %s\n", urlWithToken)
	if s.Access.Warning != "" {
		fmt.Printf("%s\n", s.Access.Warning)
	}
	if code, err := qrcode.New(urlWithToken, qrcode.Medium); err == nil {
		fmt.Println(code.ToSmallString(false))
	}
}

func (s *Server) acquireLock() (func(), error) {
	path := filepath.Join(s.DataDir, "serve.lock")
	if err := os.MkdirAll(s.DataDir, 0o755); err != nil {
		return nil, err
	}
	file, err := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o600)
	if err != nil {
		if !errors.Is(err, os.ErrExist) {
			return nil, err
		}
		pid := 0
		if content, readErr := os.ReadFile(path); readErr == nil {
			pid, _ = strconv.Atoi(strings.TrimSpace(string(content)))
		}
		if pid > 0 {
			if process, findErr := os.FindProcess(pid); findErr == nil {
				if signalErr := process.Signal(syscall.Signal(0)); signalErr == nil {
					return nil, fmt.Errorf("tokenpulse serve is already running (pid %d)", pid)
				}
			}
		}
		_ = os.Remove(path)
		file, err = os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o600)
		if err != nil {
			return nil, err
		}
	}
	_, _ = fmt.Fprintf(file, "%d\n", os.Getpid())
	file.Close()
	return func() { _ = os.Remove(path) }, nil
}

func (s *Server) listenLocalSocket() (net.Listener, error) {
	path := filepath.Join(s.DataDir, "serve.sock")
	if conn, err := net.DialTimeout("unix", path, 200*time.Millisecond); err == nil {
		conn.Close()
		return nil, errors.New("serve.sock is already accepting connections")
	}
	_ = os.Remove(path)
	listener, err := net.Listen("unix", path)
	if err != nil {
		return nil, err
	}
	if err := os.Chmod(path, 0o600); err != nil {
		listener.Close()
		return nil, err
	}
	return listener, nil
}

func (s *Server) serveUnix(listener net.Listener) {
	server := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		s.handle(w, r, true)
	})}
	_ = server.Serve(listener)
}

func (s *Server) rootHandler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !s.hostAccepted(r.Host) {
			http.Error(w, `{"error":"host not accepted"}`, http.StatusForbidden)
			return
		}
		s.handle(w, r, false)
	})
}

func (s *Server) hostAccepted(host string) bool {
	name := host
	if parsed, _, err := net.SplitHostPort(host); err == nil {
		name = parsed
	}
	name = strings.Trim(name, "[]")
	return s.Access.Hosts[name]
}

func (s *Server) handle(w http.ResponseWriter, r *http.Request, local bool) {
	if strings.HasPrefix(r.URL.Path, "/api/") || r.URL.Path == "/ws" {
		if local {
			if r.Method == http.MethodPost && r.URL.Path == "/api/reprice" {
				s.handleReprice(w, r)
				return
			}
			s.route(w, r)
			return
		}
		if r.URL.Path == "/api/pair" {
			s.handlePair(w, r)
			return
		}
		if !s.authenticate(r) {
			writeError(w, http.StatusUnauthorized, "unauthenticated")
			return
		}
		if stateChanging(r.Method) && !bearerAuthorized(r, s) && !s.originAccepted(r) {
			writeError(w, http.StatusForbidden, "origin rejected")
			return
		}
		s.route(w, r)
		return
	}
	s.serveWeb(w, r)
}

func (s *Server) authenticate(r *http.Request) bool {
	if bearerAuthorized(r, s) {
		return true
	}
	cookie, err := r.Cookie("tokenpulse_session")
	if err != nil {
		return false
	}
	return s.State.ValidSession(cookie.Value)
}

func bearerAuthorized(r *http.Request, s *Server) bool {
	header := r.Header.Get("Authorization")
	if !strings.HasPrefix(header, "Bearer ") {
		return false
	}
	token, err := s.State.Token()
	if err != nil {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(strings.TrimPrefix(header, "Bearer ")), []byte(token)) == 1
}

func (s *Server) originAccepted(r *http.Request) bool {
	origin := r.Header.Get("Origin")
	if origin == "" {
		return false
	}
	parsed, err := url.Parse(origin)
	if err != nil {
		return false
	}
	return s.hostAccepted(parsed.Host)
}

func stateChanging(method string) bool {
	return method == http.MethodPost || method == http.MethodPatch || method == http.MethodDelete
}

func (s *Server) route(w http.ResponseWriter, r *http.Request) {
	switch {
	case r.Method == http.MethodGet && r.URL.Path == "/api/state":
		s.handleState(w, r)
	case r.Method == http.MethodGet && r.URL.Path == "/api/usage":
		s.handleUsage(w, r)
	case r.Method == http.MethodGet && r.URL.Path == "/api/usage/day":
		s.handleDay(w, r)
	case r.Method == http.MethodGet && r.URL.Path == "/api/usage/model":
		s.handleModel(w, r)
	case r.Method == http.MethodGet && r.URL.Path == "/api/quota":
		s.handleQuota(w, r)
	case r.Method == http.MethodPost && r.URL.Path == "/api/refresh":
		s.handleRefresh(w, r)
	case r.Method == http.MethodGet && r.URL.Path == "/api/settings":
		s.handleSettingsGet(w, r)
	case r.Method == http.MethodPatch && r.URL.Path == "/api/settings":
		s.handleSettingsPatch(w, r)
	case r.Method == http.MethodGet && r.URL.Path == "/api/keeper":
		s.handleKeeperGet(w, r)
	case r.Method == http.MethodPost && r.URL.Path == "/api/keeper/ping":
		s.handleKeeperPing(w, r)
	case r.Method == http.MethodGet && r.URL.Path == "/api/push/key":
		s.handlePushKey(w, r)
	case r.Method == http.MethodPost && r.URL.Path == "/api/push/subscription":
		s.handlePushAdd(w, r)
	case r.Method == http.MethodDelete && r.URL.Path == "/api/push/subscription":
		s.handlePushDelete(w, r)
	case r.URL.Path == "/ws":
		s.handleWS(w, r)
	default:
		writeError(w, http.StatusNotFound, "not found")
	}
}

func writeError(w http.ResponseWriter, status int, message string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": message})
}

func writeJSON(w http.ResponseWriter, value any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(value)
}

func (s *Server) waitFirstScan() {
	select {
	case <-s.firstScan:
	case <-time.After(2 * time.Minute):
	}
}

func (s *Server) handleState(w http.ResponseWriter, r *http.Request) {
	s.waitFirstScan()
	cfg, _ := s.Config.Load()
	agents := append([]string(nil), report.AllAgents...)
	if !cfg.Display.ScanAntigravity {
		agents = remove(agents, "antigravity")
	}
	s.mu.Lock()
	scannedAt := s.lastScannedAt
	fetchedAt := s.lastFetchedAt
	s.mu.Unlock()
	writeJSON(w, map[string]any{
		"version":         version.String(),
		"scannedAt":       scannedAt,
		"fetchedAt":       fetchedAtMs(fetchedAt),
		"pricesAvailable": s.Pricing.Available(),
		"agents":          apiAgentNames(agents),
		"settings":        currentSettings(cfg),
		"keeperEngine":    cfg.Keeper.Enabled,
	})
}

func fetchedAtMs(value time.Time) int64 {
	if value.IsZero() {
		return 0
	}
	return value.UnixMilli()
}

// apiAgentName is the name phones see: Antigravity is agy everywhere in the API.
func apiAgentName(name string) string {
	if name == "antigravity" {
		return "agy"
	}
	return name
}

func internalAgentName(name string) string {
	if name == "agy" {
		return "antigravity"
	}
	return name
}

func apiAgentNames(values []string) []string {
	out := make([]string, len(values))
	for i, value := range values {
		out[i] = apiAgentName(value)
	}
	return out
}

func remove(values []string, target string) []string {
	out := values[:0]
	for _, value := range values {
		if value != target {
			out = append(out, value)
		}
	}
	return out
}

func apiAgents(raw string) ([]string, error) {
	if strings.TrimSpace(raw) == "" {
		return report.AllAgents, nil
	}
	var out []string
	for _, agent := range strings.Split(raw, ",") {
		agent = strings.TrimSpace(agent)
		agent = internalAgentName(agent)
		if agent == "" {
			continue
		}
		out = append(out, agent)
	}
	if len(out) == 0 {
		return nil, errors.New("no agents selected")
	}
	return out, nil
}

func (s *Server) handleUsage(w http.ResponseWriter, r *http.Request) {
	s.waitFirstScan()
	agents, err := apiAgents(r.URL.Query().Get("agents"))
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	value, err := s.Report.Report(report.Params{
		Preset: r.URL.Query().Get("preset"), From: r.URL.Query().Get("from"),
		To: r.URL.Query().Get("to"), Agents: agents,
	})
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	value.PricesAvailable = s.Pricing.Available()
	for i := range value.Models {
		value.Models[i].Agents = apiAgentNames(value.Models[i].Agents)
	}
	writeJSON(w, value)
}

func (s *Server) handleDay(w http.ResponseWriter, r *http.Request) {
	s.waitFirstScan()
	agents, err := apiAgents(r.URL.Query().Get("agents"))
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	value, err := s.Report.Day(r.URL.Query().Get("date"), agents)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	for i := range value.Agents {
		value.Agents[i].Agent = apiAgentName(value.Agents[i].Agent)
	}
	writeJSON(w, value)
}

func (s *Server) handleModel(w http.ResponseWriter, r *http.Request) {
	s.waitFirstScan()
	agents, err := apiAgents(r.URL.Query().Get("agents"))
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	value, err := s.Report.Model(r.URL.Query().Get("model"), r.URL.Query().Get("from"), r.URL.Query().Get("to"), agents)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	for i := range value.Agents {
		value.Agents[i].Agent = apiAgentName(value.Agents[i].Agent)
	}
	writeJSON(w, value)
}

func (s *Server) handleQuota(w http.ResponseWriter, r *http.Request) {
	s.waitFirstScan()
	cfg, _ := s.Config.Load()
	enabled := map[string]bool{}
	for _, id := range cfg.EnabledQuotaProviders(quota.ProviderIDs) {
		enabled[id] = true
	}
	s.mu.Lock()
	snapshots := map[string]provider.QuotaSnapshot{}
	for id, snapshot := range s.snapshots {
		snapshots[id] = snapshot
	}
	failures := map[string]quotaError{}
	for id, failure := range s.quotaErrors {
		failures[id] = failure
	}
	fetchedAt := s.lastFetchedAt
	s.mu.Unlock()
	type providerView struct {
		Provider string                  `json:"provider"`
		Enabled  bool                    `json:"enabled"`
		Snapshot *provider.QuotaSnapshot `json:"snapshot"`
		Error    *quotaError             `json:"error"`
	}
	providers := make([]providerView, 0, len(quota.ProviderIDs))
	for _, id := range quota.ProviderIDs {
		view := providerView{Provider: apiAgentName(id), Enabled: enabled[id]}
		if snapshot, ok := snapshots[id]; ok {
			copy := snapshot
			quota.Normalize(&copy)
			copy.Provider = apiAgentName(copy.Provider)
			view.Snapshot = &copy
		}
		if failure, ok := failures[id]; ok {
			copy := failure
			view.Error = &copy
		}
		providers = append(providers, view)
	}
	writeJSON(w, map[string]any{"providers": providers, "fetchedAt": fetchedAtMs(fetchedAt)})
}

func (s *Server) handleRefresh(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Usage *bool `json:"usage"`
		Quota *bool `json:"quota"`
	}
	_ = json.NewDecoder(io.LimitReader(r.Body, 4096)).Decode(&body)
	usage, quotaWanted := true, true
	if body.Usage != nil {
		usage = *body.Usage
	}
	if body.Quota != nil {
		quotaWanted = *body.Quota
	}
	status, retry := s.requestRefresh(usage, quotaWanted, true)
	if status == http.StatusTooManyRequests {
		w.Header().Set("Retry-After", strconv.Itoa(retry))
		writeError(w, status, "refresh rate limited")
		return
	}
	w.WriteHeader(http.StatusAccepted)
	_ = json.NewEncoder(w).Encode(map[string]bool{"accepted": true})
}

func (s *Server) requestRefresh(usage, quotaWanted, requested bool) (int, int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := time.Now()
	if requested {
		if !s.lastRequested.IsZero() && now.Sub(s.lastRequested) < 10*time.Second {
			return http.StatusTooManyRequests, int((10*time.Second - now.Sub(s.lastRequested)).Seconds()) + 1
		}
		s.lastRequested = now
	}
	if s.running {
		return http.StatusAccepted, 0
	}
	select {
	case s.refreshCh <- refreshFlags{usage: usage, quota: quotaWanted}:
	default:
	}
	return http.StatusAccepted, 0
}

func (s *Server) refreshLoop() {
	flags := refreshFlags{usage: true, quota: true}
	timer := time.NewTimer(0)
	defer timer.Stop()
	for {
		select {
		case <-s.stop:
			return
		case <-timer.C:
			flags = refreshFlags{usage: true, quota: true}
		case requested := <-s.refreshCh:
			flags = requested
		}
		s.doRefresh(flags)
		cfg, err := s.Config.Load()
		interval := 300 * time.Second
		if err == nil {
			if cfg.Display.AutoRefreshSecs > 0 {
				interval = time.Duration(cfg.Display.AutoRefreshSecs) * time.Second
			} else if err := s.logAutoRefreshZero(); err != nil {
				// startup already printed the reason
			}
		}
		timer.Reset(interval)
	}
}

func (s *Server) logAutoRefreshZero() error {
	if s.logger != nil {
		s.logger.Printf("display.auto_refresh_secs is 0; serve refreshes every 300s")
	}
	return nil
}

func (s *Server) doRefresh(flags refreshFlags) {
	s.mu.Lock()
	s.running = true
	s.mu.Unlock()
	defer func() {
		s.mu.Lock()
		s.running = false
		s.mu.Unlock()
		s.firstOnce.Do(func() { close(s.firstScan) })
	}()

	cfg, err := s.Config.Load()
	if err != nil {
		cfg = config.Default()
	}
	s.Keeper.UpdateConfig(cfg.Keeper)
	if s.logger != nil {
		s.logger.Printf("refresh start usage=%v quota=%v", flags.usage, flags.quota)
	}
	var wg sync.WaitGroup
	if flags.usage {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if err := s.Scanner.Run(scan.Options{
				Providers: scan.ProvidersFromConfig(cfg), ScanAntigravity: cfg.Display.ScanAntigravity,
			}); err != nil {
				if s.logger != nil {
					s.logger.Printf("usage refresh failed: %v", err)
				}
				return
			}
			if scanned, err := s.Report.ScannedAtValue(); err == nil {
				s.mu.Lock()
				s.lastScannedAt = scanned
				s.mu.Unlock()
				s.broadcast("usage_updated", map[string]any{"scannedAt": scanned})
			}
		}()
	}
	if flags.quota && cfg.Display.RefreshQuota {
		wg.Add(1)
		go func() {
			defer wg.Done()
			results := quota.FetchAll(s.DataDir, fetchersFor(cfg))
			s.applyQuotaResults(results)
		}()
	}
	wg.Wait()

	s.mu.Lock()
	snapshots := make([]provider.QuotaSnapshot, 0, len(s.snapshots))
	for _, snapshot := range s.snapshots {
		snapshots = append(snapshots, snapshot)
	}
	s.mu.Unlock()
	for _, request := range s.Keeper.CheckScheduled(snapshots, time.Now()) {
		s.startKeeper(request)
	}
	for _, request := range s.Keeper.CheckQuota(snapshots, time.Now()) {
		s.startKeeper(request)
	}
	if s.logger != nil {
		s.logger.Printf("refresh complete")
	}
}

func fetchersFor(cfg config.Config) []quota.Fetcher {
	var fetchers []quota.Fetcher
	for _, id := range cfg.EnabledQuotaProviders(quota.ProviderIDs) {
		switch id {
		case "claude":
			fetchers = append(fetchers, quota.NewClaudeFetcher(cfg.Display.AccountDisplay.ShowsAccount()))
		case "codex":
			fetchers = append(fetchers, quota.NewCodexFetcher())
		case "antigravity":
			fetchers = append(fetchers, quota.NewAntigravityFetcher())
		}
	}
	return fetchers
}

func (s *Server) applyQuotaResults(results []quota.FetchResult) {
	s.mu.Lock()
	var recoveries []notification.Recovery
	for _, result := range results {
		if result.Snapshot != nil {
			s.snapshots[result.Provider] = *result.Snapshot
			delete(s.quotaErrors, result.Provider)
			for _, window := range result.Snapshot.Windows {
				key := result.Provider + ":" + window.ID
				if window.UsedPercent >= 100 {
					s.exhausted[key] = true
				} else if s.exhausted[key] {
					delete(s.exhausted, key)
					recoveries = append(recoveries, notification.Recovery{
						Provider: result.Provider, WindowLabel: window.Label,
						RemainingPercent: maxFloat(100-window.UsedPercent, 0),
					})
				}
			}
		}
		if result.Err != nil {
			s.quotaErrors[result.Provider] = quotaError{Message: result.Err.Error(), At: result.FetchedAt}
		}
	}
	if len(results) > 0 {
		s.lastFetchedAt = time.Now().UTC()
	}
	fetchedAt := s.lastFetchedAt
	s.mu.Unlock()
	for _, recovery := range recoveries {
		s.broadcast("quota_recovered", map[string]any{
			"provider": recovery.Provider, "window": recovery.WindowLabel,
			"remaining": recovery.RemainingPercent,
		})
	}
	if len(recoveries) > 0 {
		cfg, _ := s.Config.Load()
		if cfg.Display.NotificationLevel != config.NotificationOff {
			notification.NotifyQuotaRestored(cfg.Display.NotificationLevel, cfg.Display.NotificationSound, recoveries)
			s.Push.Send("TokenPulse", notification.Summarize(recoveries))
		}
	}
	s.broadcast("quota_updated", map[string]any{"fetchedAt": fetchedAtMs(fetchedAt)})
}

func maxFloat(a, b float64) float64 {
	if a > b {
		return a
	}
	return b
}

func (s *Server) startKeeper(request keeper.Request) {
	go func() {
		record := s.Keeper.Execute(request)
		s.Keeper.Finish(request, record)
		s.broadcast("keeper_updated", map[string]any{"agent": record.Agent})
	}()
}

func (s *Server) handleReprice(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Apply  bool   `json:"apply"`
		Yes    bool   `json:"yes"`
		Expect string `json:"expect"`
	}
	if err := json.NewDecoder(io.LimitReader(r.Body, 4096)).Decode(&body); err != nil {
		writeError(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	if body.Apply && !body.Yes {
		writeError(w, http.StatusBadRequest, "--apply requires --yes")
		return
	}
	if err := s.Pricing.Load(false); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	report, err := s.Pricing.Reprice(body.Apply, body.Expect)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	s.Report.Clear()
	if scanned, err := s.Report.ScannedAtValue(); err == nil {
		s.mu.Lock()
		s.lastScannedAt = scanned
		s.mu.Unlock()
		s.broadcast("usage_updated", map[string]any{"scannedAt": scanned})
	}
	writeJSON(w, report)
}

func (s *Server) handleSettingsGet(w http.ResponseWriter, r *http.Request) {
	cfg, err := s.Config.Load()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, settingsView(currentSettings(cfg)))
}

func (s *Server) handleSettingsPatch(w http.ResponseWriter, r *http.Request) {
	var updates map[string]json.RawMessage
	if err := json.NewDecoder(io.LimitReader(r.Body, 64*1024)).Decode(&updates); err != nil {
		writeError(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	snapshot, err := ApplySettings(s.Config, updates)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if cfg, err := s.Config.Load(); err == nil {
		s.Keeper.UpdateConfig(cfg.Keeper)
	}
	s.Report.Clear()
	s.broadcast("settings_updated", map[string]any{})
	writeJSON(w, settingsView(snapshot))
}

func (s *Server) handleKeeperGet(w http.ResponseWriter, r *http.Request) {
	cfg, err := s.Config.Load()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	s.mu.Lock()
	snapshots := make([]provider.QuotaSnapshot, 0, len(s.snapshots))
	for _, snapshot := range s.snapshots {
		snapshots = append(snapshots, snapshot)
	}
	s.mu.Unlock()
	home, _ := os.UserHomeDir()
	type targetView struct {
		Key       string     `json:"key"`
		Group     string     `json:"group,omitempty"`
		LastFired *time.Time `json:"lastFired"`
	}
	type agentView struct {
		SessionKeeperEnabled bool         `json:"sessionKeeperEnabled"`
		WeeklyKeeperEnabled  bool         `json:"weeklyKeeperEnabled"`
		DailyWakeupTime      string       `json:"dailyWakeupTime"`
		WeeklyAllGroups      bool         `json:"weeklyAllGroups"`
		Running              bool         `json:"running"`
		Targets              []targetView `json:"targets"`
	}
	agents := map[string]agentView{}
	for _, id := range keeper.Agents {
		agent := cfg.Keeper.Agents[id]
		snapshot := findSnapshot(snapshots, id)
		view := agentView{
			SessionKeeperEnabled: agent.SessionKeeperEnabled,
			WeeklyKeeperEnabled:  agent.WeeklyKeeperEnabled,
			DailyWakeupTime:      agent.DailyWakeupTime,
			WeeklyAllGroups:      agent.WeeklyAllGroups,
			Running:              s.Keeper.InFlight(id),
		}
		for _, target := range keeper.WeeklyTargets(id, snapshot, agent.WeeklyAllGroups) {
			last, ok := s.Keeper.State.WeeklyTriggered[target.Key]
			var lastPtr *time.Time
			if ok {
				lastPtr = &last
			}
			view.Targets = append(view.Targets, targetView{Key: target.Key, Group: target.Group, LastFired: lastPtr})
		}
		if len(view.Targets) == 0 {
			view.Targets = []targetView{{Key: id}}
		}
		for i := range view.Targets {
			view.Targets[i].Key = strings.Replace(view.Targets[i].Key, "antigravity", "agy", 1)
		}
		agents[apiAgentName(id)] = view
	}
	type executionView struct {
		Time    time.Time `json:"time"`
		Agent   string    `json:"agent"`
		Trigger string    `json:"trigger"`
		Status  string    `json:"status"`
		Model   string    `json:"model"`
		Reply   string    `json:"reply"`
	}
	executions := []executionView{}
	for _, record := range s.Keeper.History.Recent(20) {
		reply := record.OutputSnippet
		if home != "" {
			reply = strings.ReplaceAll(reply, home, "~")
		}
		status := "failed"
		if record.Success {
			status = "success"
		}
		executions = append(executions, executionView{
			Time: record.Timestamp, Agent: apiAgentName(record.Agent), Trigger: record.Trigger.String(),
			Status: status, Model: record.Model, Reply: reply,
		})
	}
	writeJSON(w, map[string]any{"engine": cfg.Keeper.Enabled, "agents": agents, "executions": executions})
}

func findSnapshot(snapshots []provider.QuotaSnapshot, agent string) *provider.QuotaSnapshot {
	for i := range snapshots {
		if strings.EqualFold(snapshots[i].Provider, agent) {
			return &snapshots[i]
		}
	}
	return nil
}

func (s *Server) handleKeeperPing(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Agent string `json:"agent"`
		Group string `json:"group"`
	}
	if err := json.NewDecoder(io.LimitReader(r.Body, 4096)).Decode(&body); err != nil {
		writeError(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	body.Agent = internalAgentName(body.Agent)
	valid := false
	for _, agent := range keeper.Agents {
		if body.Agent == agent {
			valid = true
		}
	}
	if !valid {
		writeError(w, http.StatusBadRequest, "agent is not driven by the Keeper")
		return
	}
	if body.Group != "" {
		if body.Agent != "antigravity" {
			writeError(w, http.StatusBadRequest, "only Antigravity has quota groups")
			return
		}
		s.mu.Lock()
		snapshot, ok := s.snapshots["antigravity"]
		s.mu.Unlock()
		found := false
		if ok {
			for _, group := range keeper.WeeklyGroups(&snapshot) {
				if group == body.Group {
					found = true
				}
			}
		}
		if !found {
			writeError(w, http.StatusBadRequest, "unknown Antigravity quota group")
			return
		}
	}
	request, err := s.Keeper.Manual(body.Agent, body.Group)
	if err != nil {
		writeError(w, http.StatusConflict, err.Error())
		return
	}
	s.startKeeper(request)
	w.WriteHeader(http.StatusAccepted)
	_ = json.NewEncoder(w).Encode(map[string]bool{"accepted": true})
}

func (s *Server) handlePushKey(w http.ResponseWriter, r *http.Request) {
	key, err := s.Push.PublicKey()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, map[string]string{"publicKey": key})
}

func (s *Server) handlePushAdd(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(io.LimitReader(r.Body, 16*1024))
	if err != nil || s.Push.Add(json.RawMessage(body)) != nil {
		writeError(w, http.StatusBadRequest, "invalid push subscription")
		return
	}
	w.WriteHeader(http.StatusCreated)
	_ = json.NewEncoder(w).Encode(map[string]bool{"stored": true})
}

func (s *Server) handlePushDelete(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Endpoint string `json:"endpoint"`
	}
	if err := json.NewDecoder(io.LimitReader(r.Body, 4096)).Decode(&body); err != nil || body.Endpoint == "" {
		writeError(w, http.StatusBadRequest, "invalid subscription")
		return
	}
	if err := s.Push.Remove(body.Endpoint); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	_ = json.NewEncoder(w).Encode(map[string]bool{"removed": true})
}

func (s *Server) handlePair(w http.ResponseWriter, r *http.Request) {
	if !s.pairAllowed(r) {
		writeError(w, http.StatusTooManyRequests, "too many pairing attempts")
		return
	}
	var body struct {
		Token string `json:"token"`
	}
	if err := json.NewDecoder(io.LimitReader(r.Body, 4096)).Decode(&body); err != nil || body.Token == "" {
		writeError(w, http.StatusUnauthorized, "invalid token")
		return
	}
	remoteToken, err := s.State.Token()
	if err != nil || subtle.ConstantTimeCompare([]byte(body.Token), []byte(remoteToken)) != 1 {
		writeError(w, http.StatusUnauthorized, "invalid token")
		return
	}
	session, err := s.State.CreateSession()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	cookie := &http.Cookie{
		Name: "tokenpulse_session", Value: session, Path: "/", HttpOnly: true,
		SameSite: http.SameSiteStrictMode, MaxAge: 400 * 24 * 60 * 60, Secure: s.Access.HTTPS,
	}
	http.SetCookie(w, cookie)
	_ = json.NewEncoder(w).Encode(map[string]bool{"paired": true})
}

var pairAttempts = struct {
	sync.Mutex
	times []time.Time
}{}

func (s *Server) pairAllowed(r *http.Request) bool {
	now := time.Now()
	pairAttempts.Lock()
	defer pairAttempts.Unlock()
	kept := pairAttempts.times[:0]
	for _, at := range pairAttempts.times {
		if now.Sub(at) < time.Minute {
			kept = append(kept, at)
		}
	}
	pairAttempts.times = kept
	if len(pairAttempts.times) >= 10 {
		return false
	}
	pairAttempts.times = append(pairAttempts.times, now)
	return true
}

var upgrader = websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}

func (s *Server) handleWS(w http.ResponseWriter, r *http.Request) {
	if r.Header.Get("Authorization") != "" {
		writeError(w, http.StatusUnauthorized, "websocket requires a session cookie")
		return
	}
	if !s.originAccepted(r) {
		writeError(w, http.StatusForbidden, "origin rejected")
		return
	}
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	s.mu.Lock()
	s.wsClients[conn] = true
	s.mu.Unlock()
	defer func() {
		s.mu.Lock()
		delete(s.wsClients, conn)
		s.mu.Unlock()
		conn.Close()
	}()
	for {
		if _, _, err := conn.ReadMessage(); err != nil {
			return
		}
	}
}

func (s *Server) broadcast(event string, data any) {
	payload, _ := json.Marshal(map[string]any{"type": event, "data": data})
	s.mu.Lock()
	clients := make([]*websocket.Conn, 0, len(s.wsClients))
	for client := range s.wsClients {
		clients = append(clients, client)
	}
	s.mu.Unlock()
	for _, client := range clients {
		_ = client.WriteMessage(websocket.TextMessage, payload)
	}
}

func (s *Server) serveWeb(w http.ResponseWriter, r *http.Request) {
	if s.WebDir != "" {
		dir := s.WebDir
		path := filepath.Join(dir, filepath.Clean("/"+r.URL.Path))
		if info, err := os.Stat(path); err == nil && !info.IsDir() {
			http.ServeFile(w, r, path)
			return
		}
		http.ServeFile(w, r, filepath.Join(dir, "index.html"))
		return
	}
	sub, err := fs.Sub(web.FS, "dist")
	if err != nil {
		http.NotFound(w, r)
		return
	}
	path := strings.TrimPrefix(filepath.Clean("/"+r.URL.Path), "/")
	if path != "." && path != "" {
		if info, err := fs.Stat(sub, path); err == nil && !info.IsDir() {
			http.FileServer(http.FS(sub)).ServeHTTP(w, r)
			return
		}
	}
	data, err := fs.ReadFile(sub, "index.html")
	if err != nil {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	_, _ = w.Write(data)
}

// Shutdown stops the background loop.
func (s *Server) Shutdown() {
	select {
	case <-s.stop:
	default:
		close(s.stop)
	}
}

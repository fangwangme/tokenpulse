package remote

import (
	"context"
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
	"time"

	"github.com/gorilla/websocket"
	qrcode "github.com/skip2/go-qrcode"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/paths"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/quota"
	"github.com/fangwangme/tokenpulse/internal/report"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
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
	ManagedBy   string
	Quiet       bool
}

type Server struct {
	DataDir string
	Options Options
	Access  Access
	State   *StateStore
	Runtime *rt.Runtime
	Push    *pushStore
	WebDir  string

	mu           sync.Mutex
	pairMu       sync.Mutex
	pairTimes    map[string][]time.Time
	wsClients    map[*wsClient]bool
	wsValidation time.Duration
	stop         chan struct{}
	stopOnce     sync.Once
	httpServer   *http.Server
	unixServer   *http.Server
}

func NewServer(opts Options) (*Server, error) {
	dataDir, err := paths.DataDir()
	if err != nil {
		return nil, err
	}
	if version.IsTest() {
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
	scanner, err := scan.New(dataDir)
	if err != nil {
		return nil, err
	}
	logDir := filepath.Join(dataDir, "log")
	_ = os.MkdirAll(logDir, 0o755)
	logFile, _ := os.OpenFile(filepath.Join(logDir, "tokenpulse-"+time.Now().Format("2006-01-02")+".log"),
		os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	var logger *log.Logger
	if logFile != nil {
		logger = log.New(logFile, "", log.LstdFlags)
	}
	managedBy := opts.ManagedBy
	if managedBy == "" {
		managedBy = os.Getenv("TOKENPULSE_MANAGED_BY")
	}
	if managedBy == "" {
		managedBy = "daemon"
	}
	engine, err := rt.New(dataDir, manager, scanner, managedBy, logger)
	if err != nil {
		return nil, err
	}
	push := newPushStore(dataDir)
	engine.OnQuotaRecovery = func(title, body string) {
		go push.Send(title, body)
	}
	return &Server{
		DataDir:   dataDir,
		Options:   opts,
		State:     NewStateStore(dataDir),
		Runtime:   engine,
		Push:      push,
		WebDir:    opts.WebDir,
		wsClients: map[*wsClient]bool{},
		stop:      make(chan struct{}),
	}, nil
}

// Run starts the server and blocks until stopped.
func (s *Server) Run() error {
	if _, err := s.State.Load(); err != nil {
		return err
	}
	lock, err := rt.AcquireLock(s.DataDir)
	if err != nil {
		return err
	}
	defer lock.Release()

	socketPath, err := SocketPath(s.DataDir)
	if err != nil {
		return err
	}
	socket, err := s.listenLocalSocket(socketPath)
	if err != nil {
		return err
	}
	defer socket.Close()
	defer os.Remove(socketPath)

	s.Access = buildAccess(s.Options.Port, s.Options.HTTPSPort, s.Options.NoTailscale, s.Options.LAN)
	_ = os.WriteFile(filepath.Join(s.DataDir, "serve.url"), []byte(s.Access.URL), 0o600)
	listener, err := net.Listen("tcp", net.JoinHostPort(s.Access.BindHost, strconv.Itoa(s.Options.Port)))
	if err != nil {
		s.Runtime.SetWebState("", err.Error())
		return err
	}
	defer listener.Close()
	s.Runtime.SetWebState(s.Access.URL, "")

	token, err := s.State.Token()
	if err != nil {
		return err
	}
	if !s.Options.Quiet {
		s.printBanner(token)
	}

	events, unsubscribe := s.Runtime.Events.Subscribe()
	defer unsubscribe()
	go s.forwardEvents(events)

	s.Runtime.WarmCache()
	s.Runtime.Start(rt.StartOptions{InitialRefresh: true, AutoRefresh: true})
	defer s.Runtime.Stop()

	httpSrv := &http.Server{
		Handler:           s.rootHandler(),
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       2 * time.Minute,
	}
	s.mu.Lock()
	s.httpServer = httpSrv
	s.mu.Unlock()

	go s.serveUnix(socket)

	err = httpSrv.Serve(listener)
	if errors.Is(err, http.ErrServerClosed) {
		return nil
	}
	return err
}

// StartLocalSocket exposes an in-process *runtime.Runtime over the data
// directory's local Unix socket (without opening any TCP port) so a second TUI
// or CLI command can attach cleanly.
func StartLocalSocket(engine *rt.Runtime, dataDir string) (func(), error) {
	srv := &Server{
		DataDir:   dataDir,
		State:     NewStateStore(dataDir),
		Runtime:   engine,
		Push:      newPushStore(dataDir),
		wsClients: map[*wsClient]bool{},
		stop:      make(chan struct{}),
	}
	socketPath, err := SocketPath(dataDir)
	if err != nil {
		return nil, err
	}
	listener, err := srv.listenLocalSocket(socketPath)
	if err != nil {
		return nil, err
	}
	events, unsubscribe := engine.Events.Subscribe()
	go srv.forwardEvents(events)
	go srv.serveUnix(listener)

	var once sync.Once
	stopFn := func() {
		once.Do(func() {
			unsubscribe()
			srv.stopOnce.Do(func() {
				close(srv.stop)
				srv.mu.Lock()
				unixSrv := srv.unixServer
				srv.mu.Unlock()
				if unixSrv != nil {
					ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
					_ = unixSrv.Shutdown(ctx)
					cancel()
				}
			})
			_ = listener.Close()
			_ = os.Remove(socketPath)
		})
	}
	return stopFn, nil
}

func (s *Server) serveUnix(listener net.Listener) {
	unixSrv := &http.Server{
		Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			s.handle(w, r, true)
		}),
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       2 * time.Minute,
	}
	s.mu.Lock()
	s.unixServer = unixSrv
	s.mu.Unlock()
	_ = unixSrv.Serve(listener)
}

func (s *Server) forwardEvents(events <-chan rt.Event) {
	for {
		select {
		case <-s.stop:
			return
		case ev, ok := <-events:
			if !ok {
				return
			}
			s.broadcast(ev.Type, ev.Data)
		}
	}
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

func (s *Server) listenLocalSocket(path string) (net.Listener, error) {
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
	name = strings.ToLower(strings.TrimSuffix(strings.Trim(name, "[]"), "."))
	if s.Access.Hosts[name] {
		return true
	}
	for allowed, ok := range s.Access.Hosts {
		if ok && strings.ToLower(strings.TrimSuffix(strings.Trim(allowed, "[]"), ".")) == name {
			return true
		}
	}
	return false
}

func (s *Server) handle(w http.ResponseWriter, r *http.Request, local bool) {
	if strings.HasPrefix(r.URL.Path, "/api/") || r.URL.Path == "/ws" {
		if local {
			switch {
			case r.Method == http.MethodPost && r.URL.Path == "/api/reprice":
				s.handleReprice(w, r)
				return
			case r.Method == http.MethodPost && r.URL.Path == "/api/shutdown":
				s.handleShutdown(w, r)
				return
			case r.Method == http.MethodGet && r.URL.Path == "/api/config":
				s.handleConfigGet(w, r)
				return
			}
			s.route(w, r, true)
			return
		}
		if r.URL.Path == "/api/pair" {
			if r.Method != http.MethodPost {
				writeError(w, http.StatusMethodNotAllowed, "method not allowed")
				return
			}
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
		s.route(w, r, false)
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
	if err != nil || parsed.Host == "" {
		return false
	}
	scheme := strings.ToLower(parsed.Scheme)
	if scheme != "http" && scheme != "https" {
		return false
	}
	if scheme != s.expectedScheme() {
		return false
	}
	originHost, originPort, ok := splitHostPort(parsed.Host, scheme)
	if !ok {
		return false
	}
	requestHost, requestPort, ok := splitHostPort(r.Host, scheme)
	if !ok {
		return false
	}
	return originHost == requestHost && originPort == requestPort && s.hostAccepted(originHost)
}

func (s *Server) expectedScheme() string {
	if s.Access.URL != "" {
		if parsed, err := url.Parse(s.Access.URL); err == nil && parsed.Scheme != "" {
			return strings.ToLower(parsed.Scheme)
		}
	}
	if s.Access.HTTPS {
		return "https"
	}
	return "http"
}

func splitHostPort(hostport, scheme string) (string, string, bool) {
	host, port, err := net.SplitHostPort(hostport)
	if err != nil {
		host = strings.Trim(hostport, "[]")
		port = "80"
		if scheme == "https" {
			port = "443"
		}
	} else {
		host = strings.Trim(host, "[]")
	}
	if host == "" || port == "" {
		return "", "", false
	}
	return strings.ToLower(strings.TrimSuffix(host, ".")), port, true
}

func stateChanging(method string) bool {
	return method == http.MethodPost || method == http.MethodPatch || method == http.MethodDelete
}

func (s *Server) route(w http.ResponseWriter, r *http.Request, local bool) {
	switch {
	case r.Method == http.MethodGet && r.URL.Path == "/api/state":
		s.handleState(w, r)
	case r.Method == http.MethodGet && r.URL.Path == "/api/status":
		s.handleStatus(w, r)
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
		s.handleSettingsPatch(w, r, local)
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
		s.handleWS(w, r, local)
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

func (s *Server) handleStatus(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.Runtime.Status())
}

func (s *Server) handleConfigGet(w http.ResponseWriter, r *http.Request) {
	cfg, err := s.Runtime.Config.Load()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, cfg)
}

func (s *Server) handleShutdown(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]bool{"stopping": true})
	go func() {
		time.Sleep(50 * time.Millisecond)
		s.Shutdown()
	}()
}

func (s *Server) handleState(w http.ResponseWriter, r *http.Request) {
	cfg, _ := s.Runtime.Config.Load()
	agents := append([]string(nil), report.AllAgents...)
	if !cfg.Display.ScanAntigravity {
		agents = remove(agents, "antigravity")
	}
	status := s.Runtime.Status()
	writeJSON(w, map[string]any{
		"version":         version.String(),
		"scannedAt":       status.ScannedAt,
		"fetchedAt":       status.FetchedAt,
		"pricesAvailable": s.Runtime.Pricing.Available(),
		"agents":          apiAgentNames(agents),
		"settings":        currentSettings(cfg),
		"keeperEngine":    cfg.Keeper.Enabled,
	})
}

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
		agent = internalAgentName(strings.TrimSpace(agent))
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
	s.Runtime.WaitFirstScan(2 * time.Minute)
	agents, err := apiAgents(r.URL.Query().Get("agents"))
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	value, err := s.Runtime.Usage(report.Params{
		Preset: r.URL.Query().Get("preset"), From: r.URL.Query().Get("from"),
		To: r.URL.Query().Get("to"), Agents: agents,
	})
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	for i := range value.Models {
		value.Models[i].Agents = apiAgentNames(value.Models[i].Agents)
	}
	writeJSON(w, value)
}

func (s *Server) handleDay(w http.ResponseWriter, r *http.Request) {
	s.Runtime.WaitFirstScan(2 * time.Minute)
	agents, err := apiAgents(r.URL.Query().Get("agents"))
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	value, err := s.Runtime.UsageDay(r.URL.Query().Get("date"), agents)
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
	s.Runtime.WaitFirstScan(2 * time.Minute)
	agents, err := apiAgents(r.URL.Query().Get("agents"))
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	value, err := s.Runtime.UsageModel(r.URL.Query().Get("model"), r.URL.Query().Get("from"), r.URL.Query().Get("to"), agents)
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
	view := s.Runtime.Quota()
	for i := range view.Providers {
		view.Providers[i].Provider = apiAgentName(view.Providers[i].Provider)
		if view.Providers[i].Snapshot != nil {
			copy := *view.Providers[i].Snapshot
			copy.Provider = apiAgentName(copy.Provider)
			view.Providers[i].Snapshot = &copy
		}
	}
	writeJSON(w, view)
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
	writeJSON(w, s.Runtime.RequestRefresh(usage, quotaWanted))
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
	rep, err := s.Runtime.Reprice(body.Apply, body.Yes, body.Expect)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, rep)
}

func (s *Server) handleSettingsGet(w http.ResponseWriter, r *http.Request) {
	cfg, err := s.Runtime.Config.Load()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, settingsView(currentSettings(cfg)))
}

func (s *Server) handleSettingsPatch(w http.ResponseWriter, r *http.Request, local bool) {
	var updates map[string]json.RawMessage
	if err := json.NewDecoder(io.LimitReader(r.Body, 64*1024)).Decode(&updates); err != nil {
		writeError(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	_, intervalChanged := updates["display.auto_refresh_secs"]
	if local {
		cfg, err := s.Runtime.UpdateSettings(func(cfg *config.Config) error {
			for key, raw := range updates {
				if err := rt.ApplyLocalSetting(cfg, key, decodeValue(raw)); err != nil {
					return err
				}
			}
			return nil
		}, intervalChanged)
		if err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		writeJSON(w, settingsView(currentSettings(cfg)))
		return
	}
	changes, err := validatePhoneUpdates(updates)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	cfg, err := s.Runtime.UpdateSettings(func(cfg *config.Config) error {
		for _, ch := range changes {
			if err := applyOne(cfg, ch.key, ch.value); err != nil {
				return err
			}
		}
		return nil
	}, intervalChanged)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, settingsView(currentSettings(cfg)))
}

func (s *Server) handleKeeperGet(w http.ResponseWriter, r *http.Request) {
	cfg, err := s.Runtime.Config.Load()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	snapshots := s.Runtime.KeeperSnapshots()
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
			Running:              s.Runtime.Keeper.InFlight(id),
		}
		for _, target := range keeper.WeeklyTargets(id, snapshot, agent.WeeklyAllGroups) {
			last, ok := s.Runtime.Keeper.WeeklyTriggered(target.Key)
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
	for _, record := range s.Runtime.KeeperRecords(20) {
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
		snapshot := findSnapshot(s.Runtime.KeeperSnapshots(), "antigravity")
		found := false
		if snapshot != nil {
			for _, group := range keeper.WeeklyGroups(snapshot) {
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
	if _, err := s.Runtime.TriggerKeeperPing(body.Agent, body.Group); err != nil {
		writeError(w, http.StatusConflict, err.Error())
		return
	}
	w.Header().Set("Content-Type", "application/json")
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
	w.Header().Set("Content-Type", "application/json")
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
	writeJSON(w, map[string]bool{"removed": true})
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
	writeJSON(w, map[string]bool{"paired": true})
}

func (s *Server) pairAllowed(r *http.Request) bool {
	address := r.RemoteAddr
	if host, _, err := net.SplitHostPort(address); err == nil {
		address = host
	}
	now := time.Now()
	s.pairMu.Lock()
	defer s.pairMu.Unlock()
	if s.pairTimes == nil {
		s.pairTimes = map[string][]time.Time{}
	}
	kept := s.pairTimes[address][:0]
	for _, at := range s.pairTimes[address] {
		if now.Sub(at) < time.Minute {
			kept = append(kept, at)
		}
	}
	if len(kept) >= 10 {
		s.pairTimes[address] = kept
		return false
	}
	s.pairTimes[address] = append(kept, now)
	return true
}

var upgrader = websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}

const defaultWSValidation = 15 * time.Second

type wsClient struct {
	conn *websocket.Conn
	mu   sync.Mutex
}

func (c *wsClient) write(payload []byte) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if err := c.conn.SetWriteDeadline(time.Now().Add(5 * time.Second)); err != nil {
		return err
	}
	return c.conn.WriteMessage(websocket.TextMessage, payload)
}

func (s *Server) handleWS(w http.ResponseWriter, r *http.Request, local bool) {
	session := ""
	if !local {
		if r.Header.Get("Authorization") != "" {
			writeError(w, http.StatusUnauthorized, "websocket requires a session cookie")
			return
		}
		if !s.originAccepted(r) {
			writeError(w, http.StatusForbidden, "origin rejected")
			return
		}
		cookie, err := r.Cookie("tokenpulse_session")
		if err != nil {
			writeError(w, http.StatusUnauthorized, "unauthenticated")
			return
		}
		session = cookie.Value
	}
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	client := &wsClient{conn: conn}
	s.mu.Lock()
	s.wsClients[client] = true
	s.mu.Unlock()
	defer func() {
		s.mu.Lock()
		delete(s.wsClients, client)
		s.mu.Unlock()
		conn.Close()
	}()

	readDone := make(chan struct{})
	go func() {
		defer close(readDone)
		for {
			if _, _, err := conn.ReadMessage(); err != nil {
				return
			}
		}
	}()
	if local {
		select {
		case <-readDone:
		case <-s.stop:
		}
		return
	}
	interval := s.wsValidation
	if interval <= 0 {
		interval = defaultWSValidation
	}
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-readDone:
			return
		case <-s.stop:
			return
		case <-ticker.C:
			if !s.State.ValidSession(session) {
				return
			}
		}
	}
}

func (s *Server) broadcast(event string, data any) {
	payload, _ := json.Marshal(map[string]any{"type": event, "data": data})
	s.mu.Lock()
	clients := make([]*wsClient, 0, len(s.wsClients))
	for client := range s.wsClients {
		clients = append(clients, client)
	}
	s.mu.Unlock()
	var failed []*wsClient
	for _, client := range clients {
		if err := client.write(payload); err != nil {
			failed = append(failed, client)
		}
	}
	if len(failed) == 0 {
		return
	}
	s.mu.Lock()
	for _, client := range failed {
		delete(s.wsClients, client)
	}
	s.mu.Unlock()
	for _, client := range failed {
		_ = client.conn.Close()
	}
}

func (s *Server) serveWeb(w http.ResponseWriter, r *http.Request) {
	if strings.HasPrefix(r.URL.Path, "/assets/") {
		w.Header().Set("Cache-Control", "public, max-age=31536000, immutable")
	} else if r.URL.Path == "/" || r.URL.Path == "/index.html" || r.URL.Path == "/sw.js" {
		w.Header().Set("Cache-Control", "no-cache")
	}
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

// Shutdown stops HTTP/Unix listeners and the underlying runtime cleanly.
func (s *Server) Shutdown() {
	s.stopOnce.Do(func() {
		close(s.stop)
		if s.Runtime != nil {
			s.Runtime.Stop()
		}
		s.mu.Lock()
		httpSrv := s.httpServer
		unixSrv := s.unixServer
		s.mu.Unlock()
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		if httpSrv != nil {
			_ = httpSrv.Shutdown(ctx)
		}
		if unixSrv != nil {
			_ = unixSrv.Shutdown(ctx)
		}
	})
}

// Keep unused import check satisfied if quota is referenced via Runtime.
var _ = quota.ProviderIDs

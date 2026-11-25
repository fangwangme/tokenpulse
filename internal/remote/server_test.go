package remote

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"

	"github.com/gorilla/websocket"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/pricing"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/report"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

func testServer(t *testing.T) *Server {
	t.Helper()
	dir := t.TempDir()
	manager := config.NewWithPath(filepath.Join(dir, "config.toml"))
	cfg, err := manager.Load()
	if err != nil {
		t.Fatal(err)
	}
	server := &Server{
		DataDir: dir,
		Config:  manager,
		State:   NewStateStore(dir),
		Access: Access{
			Mode: "tailscale-serve", HTTPS: true,
			Hosts: map[string]bool{"localhost": true, "127.0.0.1": true, "tokenpulse.test": true},
		},
		Keeper:      keeper.NewEngine(cfg.Keeper, manager.KeeperStatePath()),
		Report:      report.NewService(filepath.Join(dir, "usage.db")),
		Pricing:     pricing.NewService(dir, filepath.Join(dir, "usage.db")),
		snapshots:   map[string]provider.QuotaSnapshot{},
		quotaErrors: map[string]quotaError{},
		exhausted:   map[string]bool{},
		firstScan:   make(chan struct{}),
		wsClients:   map[*websocket.Conn]bool{},
		refreshCh:   make(chan refreshFlags, 1),
		stop:        make(chan struct{}),
	}
	return server
}

func request(method, target, body string) *http.Request {
	r := httptest.NewRequest(method, target, strings.NewReader(body))
	r.Host = "tokenpulse.test"
	return r
}

func perform(server *Server, r *http.Request) *httptest.ResponseRecorder {
	recorder := httptest.NewRecorder()
	server.rootHandler().ServeHTTP(recorder, r)
	return recorder
}

func pair(t *testing.T, server *Server) *http.Cookie {
	t.Helper()
	token, err := server.State.Token()
	if err != nil {
		t.Fatal(err)
	}
	recorder := perform(server, request(http.MethodPost, "https://tokenpulse.test/api/pair", `{"token":"`+token+`"}`))
	if recorder.Code != http.StatusOK {
		t.Fatalf("pair = %d %s", recorder.Code, recorder.Body.String())
	}
	for _, cookie := range recorder.Result().Cookies() {
		if cookie.Name == "tokenpulse_session" {
			return cookie
		}
	}
	t.Fatal("pair set no session cookie")
	return nil
}

// Protects remote.md "Authentication": API calls need a session, pairing
// exchanges only the remote token, and the cookie is not the token.
func TestPairAndSessionAuth(t *testing.T) {
	server := testServer(t)
	token, _ := server.State.Token()

	if got := perform(server, request(http.MethodGet, "https://tokenpulse.test/api/settings", "")); got.Code != http.StatusUnauthorized {
		t.Fatalf("unpaired request = %d", got.Code)
	}
	if got := perform(server, request(http.MethodGet, "https://tokenpulse.test/api/settings?token="+token, "")); got.Code != http.StatusUnauthorized {
		t.Fatalf("query token accepted: %d", got.Code)
	}
	if got := perform(server, request(http.MethodPost, "https://tokenpulse.test/api/pair", `{"token":"wrong"}`)); got.Code != http.StatusUnauthorized {
		t.Fatalf("wrong pair token = %d", got.Code)
	}

	cookie := pair(t, server)
	if cookie.Value == token || !cookie.HttpOnly || cookie.Path != "/" ||
		cookie.SameSite != http.SameSiteStrictMode || !cookie.Secure || cookie.MaxAge != 400*24*60*60 {
		t.Fatalf("cookie = %+v", cookie)
	}

	r := request(http.MethodGet, "https://tokenpulse.test/api/settings", "")
	r.AddCookie(cookie)
	if got := perform(server, r); got.Code != http.StatusOK {
		t.Fatalf("paired request = %d %s", got.Code, got.Body.String())
	}

	// A new remote token deletes every session.
	if _, err := server.State.ResetToken(); err != nil {
		t.Fatal(err)
	}
	r = request(http.MethodGet, "https://tokenpulse.test/api/settings", "")
	r.AddCookie(cookie)
	if got := perform(server, r); got.Code != http.StatusUnauthorized {
		t.Fatalf("old cookie survived reset-token: %d", got.Code)
	}
}

// Protects remote.md "Authentication": state-changing cookie requests need an
// accepted Origin, Bearer clients may omit it, and Host must be accepted.
func TestOriginAndHostChecks(t *testing.T) {
	server := testServer(t)
	cookie := pair(t, server)

	body := `{"display.quota_display_mode":"used"}`
	r := request(http.MethodPatch, "https://tokenpulse.test/api/settings", body)
	r.AddCookie(cookie)
	if got := perform(server, r); got.Code != http.StatusForbidden {
		t.Fatalf("missing Origin = %d", got.Code)
	}

	r = request(http.MethodPatch, "https://tokenpulse.test/api/settings", body)
	r.AddCookie(cookie)
	r.Header.Set("Origin", "https://foreign.example")
	if got := perform(server, r); got.Code != http.StatusForbidden {
		t.Fatalf("foreign Origin = %d", got.Code)
	}

	token, _ := server.State.Token()
	r = request(http.MethodPatch, "https://tokenpulse.test/api/settings", body)
	r.Header.Set("Authorization", "Bearer "+token)
	if got := perform(server, r); got.Code != http.StatusOK {
		t.Fatalf("bearer without Origin = %d %s", got.Code, got.Body.String())
	}

	r = request(http.MethodGet, "https://tokenpulse.test/", "")
	r.Host = "evil.example"
	if got := perform(server, r); got.Code != http.StatusForbidden {
		t.Fatalf("foreign Host = %d", got.Code)
	}
}

// Protects remote.md "What the phone may change": a request is validated whole,
// sensitive keys are refused, and the file stays byte-identical on refusal.
func TestSettingsWhitelist(t *testing.T) {
	server := testServer(t)
	cookie := pair(t, server)
	path := server.Config.Path

	send := func(body string) *httptest.ResponseRecorder {
		r := request(http.MethodPatch, "https://tokenpulse.test/api/settings", body)
		r.AddCookie(cookie)
		r.Header.Set("Origin", "https://tokenpulse.test")
		return perform(server, r)
	}
	before, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	for _, body := range []string{
		`{"keeper.agents.claude.command":"rm -rf /"}`,
		`{"keeper.agents.codex.prompt":"hello"}`,
		`{"providers.antigravity.path":"/tmp/x"}`,
		`{"display.quota_display_mode":"bogus"}`,
		`{"display.auto_refresh_secs":42}`,
		`{"keeper.agents.agy.daily_wakeup_time":"24:00"}`,
		`{"providers.claude.enabled":false,"display.quota_display_mode":"bogus"}`,
		`{"unknown.key":true}`,
	} {
		if got := send(body); got.Code != http.StatusBadRequest {
			t.Errorf("%s = %d %s", body, got.Code, got.Body.String())
		}
		after, _ := os.ReadFile(path)
		if !bytes.Equal(before, after) {
			t.Fatalf("%s changed the config", body)
		}
	}

	got := send(`{"providers.agy.enabled":false,"keeper.agents.agy.daily_wakeup_time":"23:59","display.quota_display_mode":"used"}`)
	if got.Code != http.StatusOK {
		t.Fatalf("valid patch = %d %s", got.Code, got.Body.String())
	}
	var response settingsResponse
	if err := json.Unmarshal(got.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	if _, ok := response.Settings["providers.agy.enabled"]; !ok {
		t.Fatal("response keys must use agy")
	}
	cfg, err := server.Config.Load()
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Providers["antigravity"].Enabled || cfg.Display.QuotaDisplayMode != config.QuotaUsed ||
		cfg.Keeper.Agents["antigravity"].DailyWakeupTime != "23:59" {
		t.Fatalf("settings not applied: %+v", cfg)
	}
}

// Protects remote.md "HTTP API": Keeper pings reject unknown agents and groups.
func TestKeeperPingValidation(t *testing.T) {
	server := testServer(t)
	cookie := pair(t, server)
	send := func(body string) int {
		r := request(http.MethodPost, "https://tokenpulse.test/api/keeper/ping", body)
		r.AddCookie(cookie)
		r.Header.Set("Origin", "https://tokenpulse.test")
		return perform(server, r).Code
	}
	if got := send(`{"agent":"gemini"}`); got != http.StatusBadRequest {
		t.Fatalf("unknown agent = %d", got)
	}
	if got := send(`{"agent":"codex","group":"Gemini"}`); got != http.StatusBadRequest {
		t.Fatalf("group on codex = %d", got)
	}
	if got := send(`{"agent":"agy","group":"Gemini"}`); got != http.StatusBadRequest {
		t.Fatalf("unknown group = %d", got)
	}
}

// Protects remote.md "HTTP API": the wire names Antigravity agy on output.
func TestAPIAgentNames(t *testing.T) {
	server := testServer(t)
	close(server.firstScan)
	token, _ := server.State.Token()
	store := usage.NewStore(server.Report.DBPath)
	db, err := store.Open()
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	date := time.Now().In(time.Local).Format("2006-01-02")
	_, err = db.Exec(`INSERT INTO usage_messages (source, client, provider_id, model_id, canonical_model_id,
        session_id, message_key, timestamp_ms, date, input_tokens, output_tokens, cache_read_tokens,
        cache_write_tokens, reasoning_tokens, total_tokens, cost_usd, pricing_day, parser_version, unpriced)
        VALUES ('antigravity','antigravity-cli','google','gemini-4-carbon','gemini-4-carbon',
        's','m',?1,?2,10,5,0,0,0,15,0.1,?2,'antigravity-v7',0)`, time.Now().UnixMilli(), date)
	if err != nil {
		t.Fatal(err)
	}
	r := request(http.MethodGet, "https://tokenpulse.test/api/usage?preset=today&agents=agy", "")
	r.Header.Set("Authorization", "Bearer "+token)
	recorder := perform(server, r)
	if recorder.Code != http.StatusOK {
		t.Fatalf("usage = %d %s", recorder.Code, recorder.Body.String())
	}
	var payload struct {
		Models []struct {
			Agents []string `json:"agents"`
		} `json:"models"`
	}
	if err := json.Unmarshal(recorder.Body.Bytes(), &payload); err != nil {
		t.Fatal(err)
	}
	if len(payload.Models) != 1 || len(payload.Models[0].Agents) != 1 || payload.Models[0].Agents[0] != "agy" {
		t.Fatalf("models = %+v", payload.Models)
	}

	r = request(http.MethodGet, "https://tokenpulse.test/api/quota", "")
	r.Header.Set("Authorization", "Bearer "+token)
	recorder = perform(server, r)
	var quotaPayload struct {
		Providers []struct {
			Provider string `json:"provider"`
		} `json:"providers"`
	}
	if err := json.Unmarshal(recorder.Body.Bytes(), &quotaPayload); err != nil {
		t.Fatal(err)
	}
	found := false
	for _, provider := range quotaPayload.Providers {
		if provider.Provider == "agy" {
			found = true
		}
		if provider.Provider == "antigravity" {
			t.Fatal("quota API exposed the internal agent name")
		}
	}
	if !found {
		t.Fatal("quota API omitted agy")
	}
}

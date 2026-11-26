package remote

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"

	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
	"github.com/fangwangme/tokenpulse/internal/scan"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

func testServer(t *testing.T) *Server {
	t.Helper()
	dir := t.TempDir()
	manager := config.NewWithPath(filepath.Join(dir, "config.toml"))
	scanner, err := scan.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	engine, err := rt.New(dir, manager, scanner, "daemon", nil)
	if err != nil {
		t.Fatal(err)
	}
	server := &Server{
		DataDir: dir,
		State:   NewStateStore(dir),
		Runtime: engine,
		Push:    newPushStore(dir),
		Access: Access{
			Mode: "tailscale-serve", HTTPS: true,
			Hosts: map[string]bool{"localhost": true, "127.0.0.1": true, "tokenpulse.test": true},
		},
		wsClients:     map[*wsClient]bool{},
		allowShutdown: true,
		stop:          make(chan struct{}),
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
	path := server.Runtime.Config.Path

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
	cfg, err := server.Runtime.Config.Load()
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
	server.Runtime.MarkScanned(1, time.Now())
	token, _ := server.State.Token()
	store := usage.NewStore(server.Runtime.Report.DBPath)
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

func TestRefreshGateAllowsOneManualRefreshPerMinute(t *testing.T) {
	now := time.Date(2026, 11, 26, 12, 0, 0, 0, time.UTC)
	allowUsage, allowQuota, retry := rt.RefreshGate(now, time.Time{}, time.Time{}, true, true)
	if !allowUsage || !allowQuota || retry != 0 {
		t.Fatalf("first refresh = %v %v %d", allowUsage, allowQuota, retry)
	}

	allowUsage, allowQuota, retry = rt.RefreshGate(now.Add(10*time.Second), now, now, true, true)
	if allowUsage || allowQuota || retry != 50 {
		t.Fatalf("throttled refresh = %v %v %d", allowUsage, allowQuota, retry)
	}

	allowUsage, allowQuota, _ = rt.RefreshGate(now.Add(59*time.Second), now, now, true, false)
	if allowUsage || allowQuota {
		t.Fatal("quota was not requested but still allowed")
	}

	allowUsage, _, _ = rt.RefreshGate(now.Add(time.Minute), now, now, true, false)
	if !allowUsage {
		t.Fatal("refresh exactly one minute later should be allowed")
	}
}

func TestStatusAnswersBeforeFirstScan(t *testing.T) {
	server := testServer(t)
	server.Runtime.MarkScanned(123, time.Unix(456, 0))
	token, err := server.State.Token()
	if err != nil {
		t.Fatal(err)
	}
	r := request(http.MethodGet, "https://tokenpulse.test/api/status", "")
	r.Header.Set("Authorization", "Bearer "+token)
	done := make(chan *httptest.ResponseRecorder, 1)
	go func() { done <- perform(server, r) }()
	select {
	case recorder := <-done:
		if recorder.Code != http.StatusOK {
			t.Fatalf("status = %d %s", recorder.Code, recorder.Body.String())
		}
		if !strings.Contains(recorder.Body.String(), `"scannedAt":123`) {
			t.Fatalf("body = %s", recorder.Body.String())
		}
	case <-time.After(time.Second):
		t.Fatal("/api/status waited for the first scan")
	}
}

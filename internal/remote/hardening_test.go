package remote

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gorilla/websocket"
)

// Protects remote.md "Authentication": Origin must match the request's scheme,
// host and effective port, not just the host name.
func TestOriginRequiresSchemeHostAndPort(t *testing.T) {
	server := testServer(t)
	cookie := pair(t, server)
	body := `{"display.quota_display_mode":"used"}`
	cases := []struct {
		host, origin string
		want         int
	}{
		{"tokenpulse.test", "https://tokenpulse.test:3000", http.StatusForbidden},
		{"tokenpulse.test", "http://tokenpulse.test", http.StatusForbidden},
		{"tokenpulse.test:8444", "https://tokenpulse.test", http.StatusForbidden},
		{"tokenpulse.test:8444", "https://tokenpulse.test:8444", http.StatusOK},
	}
	for _, test := range cases {
		r := request(http.MethodPatch, "https://tokenpulse.test/api/settings", body)
		r.Host = test.host
		r.AddCookie(cookie)
		r.Header.Set("Origin", test.origin)
		if got := perform(server, r); got.Code != test.want {
			t.Errorf("Host %s Origin %s = %d, want %d", test.host, test.origin, got.Code, test.want)
		}
	}
}

// Protects remote.md "Pairing": pairing is POST-only and limited per remote IP,
// so one client cannot lock everyone else out.
func TestPairMethodAndPerIPLimit(t *testing.T) {
	server := testServer(t)
	r := request(http.MethodGet, "https://tokenpulse.test/api/pair", "")
	if got := perform(server, r); got.Code != http.StatusMethodNotAllowed {
		t.Fatalf("GET pair = %d", got.Code)
	}
	for i := 0; i < 10; i++ {
		r = request(http.MethodPost, "https://tokenpulse.test/api/pair", `{"token":"wrong"}`)
		r.RemoteAddr = "10.0.0.1:1234"
		if got := perform(server, r); got.Code != http.StatusUnauthorized {
			t.Fatalf("attempt %d = %d", i+1, got.Code)
		}
	}
	r = request(http.MethodPost, "https://tokenpulse.test/api/pair", `{"token":"wrong"}`)
	r.RemoteAddr = "10.0.0.1:9999"
	if got := perform(server, r); got.Code != http.StatusTooManyRequests {
		t.Fatalf("limited IP = %d", got.Code)
	}
	r = request(http.MethodPost, "https://tokenpulse.test/api/pair", `{"token":"wrong"}`)
	r.RemoteAddr = "10.0.0.2:1234"
	if got := perform(server, r); got.Code != http.StatusUnauthorized {
		t.Fatalf("other IP = %d", got.Code)
	}
}

// Protects remote.md "Local control": the Unix socket is trusted, so its
// WebSocket must not require a browser Origin header.
func TestLocalWebSocketSkipsOrigin(t *testing.T) {
	dir := t.TempDir()
	server := testServer(t)
	server.DataDir = dir
	socket := filepath.Join(dir, "serve.sock")
	listener, err := server.listenLocalSocket(socket)
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	go server.serveUnix(listener)

	client := NewLocalClient(dir)
	conn, err := client.WebSocket()
	if err != nil {
		t.Fatalf("local websocket: %v", err)
	}
	conn.Close()
}

// Protects remote.md "Live updates": concurrent broadcasts share one socket
// safely, with every writer taking the per-client lock and deadline.
func TestBroadcastConcurrentWriters(t *testing.T) {
	server := testServer(t)
	session, err := server.State.CreateSession()
	if err != nil {
		t.Fatal(err)
	}
	httpServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		server.handleWS(w, r, false)
	}))
	defer httpServer.Close()
	header := http.Header{}
	header.Set("Origin", "https://"+strings.TrimPrefix(httpServer.URL, "http://"))
	header.Set("Cookie", "tokenpulse_session="+session)
	conn, _, err := websocket.DefaultDialer.Dial("ws"+strings.TrimPrefix(httpServer.URL, "http")+"/ws", header)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()

	const writers = 20
	var group sync.WaitGroup
	for i := 0; i < writers; i++ {
		group.Add(1)
		go func() {
			defer group.Done()
			server.broadcast("test", map[string]any{"ok": true})
		}()
	}
	group.Wait()
	for i := 0; i < writers; i++ {
		if err := conn.SetReadDeadline(time.Now().Add(5 * time.Second)); err != nil {
			t.Fatal(err)
		}
		if _, _, err := conn.ReadMessage(); err != nil {
			t.Fatalf("read %d: %v", i, err)
		}
	}
}

func TestStateStoreRejectsCorruptFileAndCapsSessions(t *testing.T) {
	dir := t.TempDir()
	store := NewStateStore(dir)
	if err := os.WriteFile(store.Path, []byte("{broken"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := store.Load(); err == nil {
		t.Fatal("corrupt remote.json was accepted")
	}
	if content, _ := os.ReadFile(store.Path); string(content) != "{broken" {
		t.Fatalf("corrupt file was overwritten: %q", content)
	}
	if err := os.Remove(store.Path); err != nil {
		t.Fatal(err)
	}
	if _, err := store.Load(); err != nil {
		t.Fatal(err)
	}
	var tokens []string
	for i := 0; i < 25; i++ {
		token, err := store.CreateSession()
		if err != nil {
			t.Fatal(err)
		}
		tokens = append(tokens, token)
	}
	state, err := store.Load()
	if err != nil {
		t.Fatal(err)
	}
	if len(state.Sessions) != maxSessions {
		t.Fatalf("sessions = %d, want %d", len(state.Sessions), maxSessions)
	}
	if store.ValidSession(tokens[0]) {
		t.Fatal("oldest session survived the cap")
	}
	if !store.ValidSession(tokens[len(tokens)-1]) {
		t.Fatal("newest session was evicted")
	}
	expired := State{Token: state.Token, Sessions: []Session{{
		Hash: hashToken("expired"), CreatedAt: time.Now().Add(-sessionTTL - time.Hour),
	}}}
	data, _ := json.Marshal(expired)
	if err := os.WriteFile(store.Path, data, 0o600); err != nil {
		t.Fatal(err)
	}
	if store.ValidSession("expired") {
		t.Fatal("expired session was accepted")
	}
}

func TestPushAddReplacesRotatedKeys(t *testing.T) {
	store := newPushStore(t.TempDir())
	first := json.RawMessage(`{"endpoint":"https://push.example/1","keys":{"auth":"old","p256dh":"old"}}`)
	rotated := json.RawMessage(`{"endpoint":"https://push.example/1","keys":{"auth":"new","p256dh":"new"}}`)
	if err := store.Add(first); err != nil {
		t.Fatal(err)
	}
	if err := store.Add(rotated); err != nil {
		t.Fatal(err)
	}
	if len(store.subscriptions) != 1 {
		t.Fatalf("subscriptions = %d", len(store.subscriptions))
	}
	if !strings.Contains(string(store.subscriptions[0]), `"new"`) {
		t.Fatalf("keys were not rotated: %s", store.subscriptions[0])
	}
	payload := pushPayload("Title", "Body")
	var decoded map[string]string
	if err := json.Unmarshal(payload, &decoded); err != nil || decoded["title"] != "Title" || decoded["body"] != "Body" {
		t.Fatalf("payload = %s", payload)
	}
}

func TestTailscaleFallbackNeverBindsWildcard(t *testing.T) {
	access := tailscaleIPAccess(TailscaleInfo{}, 4120, map[string]bool{"localhost": true}, "warning")
	if access.BindHost != "127.0.0.1" || access.URL != "http://localhost:4120/" || !strings.Contains(access.Warning, "No Tailscale IP") {
		t.Fatalf("access = %+v", access)
	}
}

// Protects remote.md "Pairing": reset-token closes phones already connected,
// without restarting serve.
func TestResetTokenClosesWebSockets(t *testing.T) {
	server := testServer(t)
	server.wsValidation = 20 * time.Millisecond
	session, err := server.State.CreateSession()
	if err != nil {
		t.Fatal(err)
	}
	httpServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		server.handleWS(w, r, false)
	}))
	defer httpServer.Close()
	header := http.Header{}
	header.Set("Origin", "https://"+strings.TrimPrefix(httpServer.URL, "http://"))
	header.Set("Cookie", "tokenpulse_session="+session)
	conn, _, err := websocket.DefaultDialer.Dial("ws"+strings.TrimPrefix(httpServer.URL, "http")+"/ws", header)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	if _, err := server.State.ResetToken(); err != nil {
		t.Fatal(err)
	}
	_ = conn.SetReadDeadline(time.Now().Add(2 * time.Second))
	for {
		if _, _, err := conn.ReadMessage(); err != nil {
			return
		}
	}
}

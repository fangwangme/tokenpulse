package quota

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func writeFile(t *testing.T, path string, data []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatal(err)
	}
}

func changed(t *testing.T, path string, before []byte) bool {
	t.Helper()
	after, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return !bytes.Equal(before, after)
}

// Protects quota.md "Credentials": an expired Claude login names Claude and
// tells the user to open it, and the credential is never rewritten.
func TestClaudeExpiredCredentialReadOnly(t *testing.T) {
	home := t.TempDir()
	path := filepath.Join(home, ".claude", ".credentials.json")
	body := []byte(`{"claudeAiOauth":{"accessToken":"secret","refreshToken":"refresh","expiresAt":1,"subscriptionType":"max"}}`)
	writeFile(t, path, body)
	fetcher := NewClaudeFetcher(false)
	fetcher.Home = home
	fetcher.Keychain = false
	fetcher.Now = func() time.Time { return time.Date(2026, 11, 25, 12, 0, 0, 0, time.UTC) }
	_, err := fetcher.Fetch()
	if err == nil || !strings.Contains(err.Error(), "Claude") || !strings.Contains(err.Error(), "open Claude") {
		t.Fatalf("error = %v", err)
	}
	if changed(t, path, body) {
		t.Fatal("Claude credential was written")
	}
}

func jwt(exp int64) string {
	header := base64.RawURLEncoding.EncodeToString([]byte(`{"alg":"none"}`))
	payload, _ := json.Marshal(map[string]any{"exp": exp, "email": "fixture@example.test"})
	return header + "." + base64.RawURLEncoding.EncodeToString(payload) + ".sig"
}

// Protects quota.md "Credentials": an expired Codex access token is reported
// without refreshing or writing auth.json.
func TestCodexExpiredCredentialReadOnly(t *testing.T) {
	home := t.TempDir()
	path := filepath.Join(home, ".codex", "auth.json")
	body, _ := json.Marshal(map[string]any{"tokens": map[string]any{
		"access_token": jwt(time.Now().Add(-time.Hour).Unix()), "refresh_token": "refresh", "id_token": jwt(time.Now().Add(time.Hour).Unix()),
	}})
	writeFile(t, path, body)
	t.Setenv("CODEX_HOME", filepath.Join(home, ".codex"))
	fetcher := NewCodexFetcher()
	fetcher.Home = home
	_, err := fetcher.Fetch()
	if err == nil || !strings.Contains(err.Error(), "Codex") || !strings.Contains(err.Error(), "open Codex") {
		t.Fatalf("error = %v", err)
	}
	if changed(t, path, body) {
		t.Fatal("Codex auth.json was written")
	}
}

// Protects quota.md "Credentials": expired Antigravity login names Antigravity
// and leaves the token file byte-identical.
func TestAntigravityExpiredCredentialReadOnly(t *testing.T) {
	home := t.TempDir()
	path := filepath.Join(home, "antigravity-token.json")
	body := []byte(`{"token":{"access_token":"secret","expiry":"2020-01-01T00:00:00Z"},"id_token":"id"}`)
	writeFile(t, path, body)
	t.Setenv("TOKENPULSE_ANTIGRAVITY_AUTH_PATH", path)
	fetcher := NewAntigravityFetcher()
	fetcher.Home = home
	fetcher.SkipLanguageServer = true
	fetcher.SkipAgyCLI = true
	_, err := fetcher.Fetch()
	if err == nil || !strings.Contains(err.Error(), "Antigravity") || !strings.Contains(err.Error(), "open Antigravity") {
		t.Fatalf("error = %v", err)
	}
	if changed(t, path, body) {
		t.Fatal("Antigravity credential was written")
	}
}

// Protects quota.md "Credentials" and quota-providers.md: a successful fetch
// is read-only for Claude and Codex.
func TestSuccessfulFetchDoesNotWriteCredentials(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/usage":
			if strings.Contains(r.Header.Get("Authorization"), "codex") {
				w.Write([]byte(`{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":18000,"reset_at":1764300000}}}`))
				return
			}
			w.Write([]byte(`{"five_hour":{"utilization":10}}`))
		case "/credits":
			w.Write([]byte(`{"credits":[]}`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()

	home := t.TempDir()
	claudePath := filepath.Join(home, ".claude", ".credentials.json")
	claudeBody := []byte(`{"claudeAiOauth":{"accessToken":"claude-secret","expiresAt":4102444800000,"subscriptionType":"max"}}`)
	writeFile(t, claudePath, claudeBody)
	claude := NewClaudeFetcher(false)
	claude.Home = home
	claude.Keychain = false
	claude.UsageURL = server.URL + "/usage"
	claude.Now = func() time.Time { return time.Date(2026, 11, 25, 12, 0, 0, 0, time.UTC) }
	if _, err := claude.Fetch(); err != nil {
		t.Fatal(err)
	}
	if changed(t, claudePath, claudeBody) {
		t.Fatal("Claude credential was written")
	}

	codexPath := filepath.Join(home, ".codex", "auth.json")
	codexBody, _ := json.Marshal(map[string]any{"tokens": map[string]any{
		"access_token": jwt(time.Now().Add(time.Hour).Unix()), "refresh_token": "refresh",
	}})
	writeFile(t, codexPath, codexBody)
	t.Setenv("CODEX_HOME", filepath.Join(home, ".codex"))
	codex := NewCodexFetcher()
	codex.Home = home
	codex.UsageURL = server.URL + "/usage"
	codex.CreditsURL = server.URL + "/credits"
	if _, err := codex.Fetch(); err != nil {
		t.Fatal(err)
	}
	if changed(t, codexPath, codexBody) {
		t.Fatal("Codex auth.json was written")
	}
}

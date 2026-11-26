package quota

import (
	"database/sql"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

const openCodeUsageFixture = `{
  "usage": {
    "rolling": {"status": "ok", "percent": 30, "resetsAt": "2026-11-21T15:00:00.000Z"},
    "weekly": {"status": "rate-limited", "percent": 80, "resetsAt": "2026-11-23T00:00:00.000Z"},
    "monthly": {"status": "ok", "percent": 50.5, "resetsAt": "2026-12-01T00:00:00.000Z"}
  }
}`

func TestOpenCodeUsageParsing(t *testing.T) {
	snapshot, err := parseOpenCodeUsage([]byte(openCodeUsageFixture))
	if err != nil {
		t.Fatal(err)
	}
	if snapshot.Provider != "opencode" || len(snapshot.Windows) != 3 {
		t.Fatalf("snapshot = %+v", snapshot)
	}
	rolling, weekly, monthly := snapshot.Windows[0], snapshot.Windows[1], snapshot.Windows[2]
	if rolling.Label != "5h" || rolling.UsedPercent != 30 ||
		rolling.PeriodMs == nil || *rolling.PeriodMs != 18_000_000 {
		t.Errorf("rolling = %+v", rolling)
	}
	if weekly.Label != "Weekly" || weekly.UsedPercent != 100 ||
		weekly.PeriodMs == nil || *weekly.PeriodMs != 604_800_000 {
		t.Errorf("weekly = %+v", weekly)
	}
	if monthly.Label != "Monthly" || monthly.UsedPercent != 50.5 ||
		monthly.PeriodMs == nil || *monthly.PeriodMs != 2_592_000_000 {
		t.Errorf("monthly = %+v", monthly)
	}
	if rolling.ResetsAt == nil || !rolling.ResetsAt.Equal(time.Date(2026, 11, 21, 15, 0, 0, 0, time.UTC)) {
		t.Errorf("rolling reset = %v", rolling.ResetsAt)
	}
}

func TestOpenCodeUsageParsingToleratesMissingData(t *testing.T) {
	for name, body := range map[string]string{
		"empty usage":   `{"usage":{}}`,
		"missing usage": `{}`,
	} {
		t.Run(name, func(t *testing.T) {
			snapshot, err := parseOpenCodeUsage([]byte(body))
			if err != nil || snapshot != nil {
				t.Fatalf("snapshot=%+v err=%v", snapshot, err)
			}
		})
	}

	snapshot, err := parseOpenCodeUsage([]byte(`{"usage":{"weekly":{"status":"ok","percent":42,"resetsAt":""}}}`))
	if err != nil {
		t.Fatal(err)
	}
	if snapshot == nil || len(snapshot.Windows) != 1 {
		t.Fatalf("snapshot = %+v", snapshot)
	}
	weekly := snapshot.Windows[0]
	if weekly.Label != "Weekly" || weekly.UsedPercent != 42 || weekly.ResetsAt != nil {
		t.Fatalf("weekly = %+v", weekly)
	}
}

func writeOpenCodeCredential(t *testing.T, path, value string) {
	t.Helper()
	db, err := sql.Open("sqlite", "file:"+path+"?_pragma=busy_timeout(5000)")
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	if _, err := db.Exec(`CREATE TABLE credential (
        id TEXT PRIMARY KEY, integration_id TEXT, label TEXT NOT NULL, value TEXT NOT NULL,
        active INTEGER, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL)`); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated)
        VALUES ('cred_1', 'opencode', 'Personal', ?, 1, 1, 1)`, value); err != nil {
		t.Fatal(err)
	}
}

func TestOpenCodeFetchReadOnly(t *testing.T) {
	var gotOrg, gotAgent string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotOrg = r.Header.Get("x-opencode-org-id")
		gotAgent = r.Header.Get("User-Agent")
		if r.Header.Get("Authorization") != "Bearer secret-key" {
			t.Errorf("authorization = %q", r.Header.Get("Authorization"))
		}
		w.Write([]byte(openCodeUsageFixture))
	}))
	defer server.Close()

	path := filepath.Join(t.TempDir(), "opencode.db")
	value, _ := json.Marshal(map[string]any{
		"type": "key",
		"key":  "secret-key",
		"metadata": map[string]any{
			"orgID": "wrk_1",
			"email": "demo@example.test",
		},
	})
	writeOpenCodeCredential(t, path, string(value))
	before, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	fetcher := &OpenCodeFetcher{
		DBPath: path, UsageURL: server.URL, ShowAccount: true,
		Now: func() time.Time { return time.Date(2026, 11, 25, 12, 0, 0, 0, time.UTC) },
	}
	snapshot, err := fetcher.Fetch()
	if err != nil {
		t.Fatal(err)
	}
	if snapshot.Account != "demo@example.test" || snapshot.FetchedAt.IsZero() {
		t.Fatalf("snapshot = %+v", snapshot)
	}
	if gotOrg != "wrk_1" || !strings.HasPrefix(gotAgent, "tokenpulse/") {
		t.Fatalf("headers org=%q agent=%q", gotOrg, gotAgent)
	}
	after, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(before) != string(after) {
		t.Fatal("OpenCode credential database was written")
	}
}

func TestOpenCodeFetchOmitsMissingExpiredAndNotSubscribed(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusForbidden)
	}))
	defer server.Close()

	t.Run("no database", func(t *testing.T) {
		fetcher := &OpenCodeFetcher{DBPath: filepath.Join(t.TempDir(), "missing.db"), UsageURL: server.URL, Now: time.Now}
		snapshot, err := fetcher.Fetch()
		if err != nil || snapshot != nil {
			t.Fatalf("snapshot=%+v err=%v", snapshot, err)
		}
	})

	t.Run("expired oauth", func(t *testing.T) {
		path := filepath.Join(t.TempDir(), "opencode.db")
		value, _ := json.Marshal(map[string]any{
			"type": "oauth", "access": "secret", "expires": time.Now().Add(-time.Hour).UnixMilli(),
		})
		writeOpenCodeCredential(t, path, string(value))
		fetcher := &OpenCodeFetcher{DBPath: path, UsageURL: server.URL, Now: time.Now}
		snapshot, err := fetcher.Fetch()
		if err != nil || snapshot != nil {
			t.Fatalf("snapshot=%+v err=%v", snapshot, err)
		}
	})

	t.Run("no Go subscription", func(t *testing.T) {
		path := filepath.Join(t.TempDir(), "opencode.db")
		value, _ := json.Marshal(map[string]any{"type": "key", "key": "secret"})
		writeOpenCodeCredential(t, path, string(value))
		fetcher := &OpenCodeFetcher{DBPath: path, UsageURL: server.URL, Now: time.Now}
		snapshot, err := fetcher.Fetch()
		if err != nil || snapshot != nil {
			t.Fatalf("snapshot=%+v err=%v", snapshot, err)
		}
	})
}

func TestOpenCodeFetchOAuthAndHTTPStatus(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.UTC)

	t.Run("valid oauth", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.Header.Get("Authorization") != "Bearer oauth-token" {
				t.Errorf("authorization = %q", r.Header.Get("Authorization"))
			}
			w.Write([]byte(openCodeUsageFixture))
		}))
		defer server.Close()

		path := filepath.Join(t.TempDir(), "opencode.db")
		value, _ := json.Marshal(map[string]any{
			"type": "oauth", "access": "oauth-token", "expires": now.Add(time.Hour).UnixMilli(),
		})
		writeOpenCodeCredential(t, path, string(value))
		fetcher := &OpenCodeFetcher{DBPath: path, UsageURL: server.URL, Now: func() time.Time { return now }}
		snapshot, err := fetcher.Fetch()
		if err != nil || snapshot == nil || len(snapshot.Windows) != 3 {
			t.Fatalf("snapshot=%+v err=%v", snapshot, err)
		}
	})

	for name, status := range map[string]int{"unauthorized": http.StatusUnauthorized, "server error": http.StatusInternalServerError} {
		t.Run(name, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.WriteHeader(status)
			}))
			defer server.Close()

			path := filepath.Join(t.TempDir(), "opencode.db")
			writeOpenCodeCredential(t, path, `{"type":"key","key":"secret"}`)
			fetcher := &OpenCodeFetcher{DBPath: path, UsageURL: server.URL, Now: func() time.Time { return now }}
			snapshot, err := fetcher.Fetch()
			if status == http.StatusUnauthorized {
				if err != nil || snapshot != nil {
					t.Fatalf("snapshot=%+v err=%v", snapshot, err)
				}
				return
			}
			if err == nil || snapshot != nil {
				t.Fatalf("snapshot=%+v err=%v", snapshot, err)
			}
		})
	}

	t.Run("external credential", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			t.Error("external credential made a usage request")
		}))
		defer server.Close()

		path := filepath.Join(t.TempDir(), "opencode.db")
		writeOpenCodeCredential(t, path, `{"type":"external"}`)
		fetcher := &OpenCodeFetcher{DBPath: path, UsageURL: server.URL, Now: func() time.Time { return now }}
		snapshot, err := fetcher.Fetch()
		if err != nil || snapshot != nil {
			t.Fatalf("snapshot=%+v err=%v", snapshot, err)
		}
	})
}

type silentFetcher struct{}

func (silentFetcher) ProviderID() string                      { return "silent" }
func (silentFetcher) Fetch() (*provider.QuotaSnapshot, error) { return nil, nil }

type snapshotFetcher struct{}

func (snapshotFetcher) ProviderID() string { return "claude" }
func (snapshotFetcher) Fetch() (*provider.QuotaSnapshot, error) {
	return &provider.QuotaSnapshot{Provider: "claude"}, nil
}

func TestFetchAllMarksProviderWithNothingToReportAbsent(t *testing.T) {
	results := FetchAll(t.TempDir(), []Fetcher{silentFetcher{}, snapshotFetcher{}})
	if len(results) != 2 {
		t.Fatalf("results = %+v", results)
	}
	if results[0].Provider != "silent" || !results[0].Absent || results[0].Err != nil {
		t.Fatalf("silent result = %+v", results[0])
	}
	if results[1].Provider != "claude" || results[1].Absent || results[1].Snapshot == nil {
		t.Fatalf("claude result = %+v", results[1])
	}
}

func TestOpenCodeCorruptDBReturnsErrorNotAbsent(t *testing.T) {
	path := filepath.Join(t.TempDir(), "opencode.db")
	if err := os.WriteFile(path, []byte("not a sqlite database"), 0o600); err != nil {
		t.Fatal(err)
	}
	fetcher := &OpenCodeFetcher{DBPath: path, UsageURL: "http://127.0.0.1:1"}
	snapshot, err := fetcher.Fetch()
	if err == nil || snapshot != nil {
		t.Fatalf("Fetch() = (%+v, %v), want error", snapshot, err)
	}
	results := FetchAll(t.TempDir(), []Fetcher{fetcher})
	if len(results) != 1 || results[0].Absent || results[0].Err == nil {
		t.Fatalf("FetchAll() = %+v, want error without Absent", results)
	}
}

func TestOpenCodeChannelDatabaseAndNilNow(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer beta-key" {
			t.Errorf("authorization = %q", r.Header.Get("Authorization"))
		}
		w.Write([]byte(openCodeUsageFixture))
	}))
	defer server.Close()

	xdg := t.TempDir()
	t.Setenv("XDG_DATA_HOME", xdg)
	t.Setenv("OPENCODE_DB", "")
	dir := filepath.Join(xdg, "opencode")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	writeOpenCodeCredential(t, filepath.Join(dir, "opencode-beta.db"), `{"type":"key","key":"beta-key"}`)

	fetcher := &OpenCodeFetcher{Home: t.TempDir(), UsageURL: server.URL}
	snapshot, err := fetcher.Fetch()
	if err != nil || snapshot == nil || len(snapshot.Windows) != 3 {
		t.Fatalf("Fetch() = (%+v, %v)", snapshot, err)
	}
}

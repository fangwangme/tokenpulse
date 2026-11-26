package quota

import (
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/version"
)

const openCodeUsageURL = "https://opencode.ai/inference/go/v1/usage"

// OpenCodeFetcher reads OpenCode's local credential and the Go usage endpoint.
// It never refreshes or writes the credential.
type OpenCodeFetcher struct {
	Home        string
	DBPath      string
	UsageURL    string
	ShowAccount bool
	Now         func() time.Time
}

func NewOpenCodeFetcher(showAccount bool) *OpenCodeFetcher {
	home, _ := os.UserHomeDir()
	return &OpenCodeFetcher{Home: home, UsageURL: openCodeUsageURL, ShowAccount: showAccount, Now: time.Now}
}

func (f *OpenCodeFetcher) ProviderID() string { return "opencode" }

func (f *OpenCodeFetcher) now() time.Time {
	if f.Now != nil {
		return f.Now()
	}
	return time.Now()
}

// OpenCodeDatabasePath resolves the database OpenCode writes credentials to.
// OPENCODE_DB follows the usage parser: a relative value is inside the data
// directory, not the process working directory.
func OpenCodeDatabasePath(home string) string {
	dataDir := openCodeDataDir(home)
	if db := strings.TrimSpace(os.Getenv("OPENCODE_DB")); db != "" {
		if !filepath.IsAbs(db) {
			db = filepath.Join(dataDir, db)
		}
		return db
	}
	return filepath.Join(dataDir, "opencode.db")
}

func openCodeDatabasePaths(home string) []string {
	dataDir := openCodeDataDir(home)
	if db := strings.TrimSpace(os.Getenv("OPENCODE_DB")); db != "" {
		if !filepath.IsAbs(db) {
			db = filepath.Join(dataDir, db)
		}
		return []string{db}
	}
	paths := []string{filepath.Join(dataDir, "opencode.db")}
	entries, err := os.ReadDir(dataDir)
	if err != nil {
		return paths
	}
	var channels []string
	for _, entry := range entries {
		name := entry.Name()
		if entry.IsDir() || !strings.HasPrefix(name, "opencode-") || !strings.HasSuffix(name, ".db") {
			continue
		}
		channels = append(channels, filepath.Join(dataDir, name))
	}
	sort.Strings(channels)
	return append(paths, channels...)
}

func openCodeDataDir(home string) string {
	if xdg := strings.TrimSpace(os.Getenv("XDG_DATA_HOME")); xdg != "" {
		return filepath.Join(xdg, "opencode")
	}
	return filepath.Join(home, ".local", "share", "opencode")
}

type openCodeCredential struct {
	Type     string         `json:"type"`
	Key      string         `json:"key"`
	Access   string         `json:"access"`
	Expires  int64          `json:"expires"`
	Metadata map[string]any `json:"metadata"`
}

func (f *OpenCodeFetcher) credential() (string, map[string]any, bool, error) {
	paths := []string{f.DBPath}
	if f.DBPath == "" {
		paths = openCodeDatabasePaths(f.Home)
	}
	var lastErr error
	for _, path := range paths {
		token, metadata, ok, err := f.credentialFromDB(path)
		if err != nil {
			lastErr = err
			continue
		}
		if ok {
			return token, metadata, true, nil
		}
	}
	return "", nil, false, lastErr
}

func (f *OpenCodeFetcher) credentialFromDB(path string) (string, map[string]any, bool, error) {
	if _, err := os.Stat(path); err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return "", nil, false, nil
		}
		return "", nil, false, fmt.Errorf("OpenCode credential database could not be accessed: %w", err)
	}
	db, err := sql.Open("sqlite", "file:"+path+"?mode=ro&_pragma=busy_timeout(5000)")
	if err != nil {
		return "", nil, false, fmt.Errorf("OpenCode credential database could not be opened: %w", err)
	}
	defer db.Close()
	var hasTable int
	err = db.QueryRow(`SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'credential'`).Scan(&hasTable)
	if errors.Is(err, sql.ErrNoRows) {
		return "", nil, false, nil
	}
	if err != nil {
		return "", nil, false, fmt.Errorf("OpenCode credential database could not be read: %w", err)
	}
	var raw string
	err = db.QueryRow(`SELECT value FROM credential
        WHERE integration_id = 'opencode' AND active = 1
        ORDER BY time_created DESC, id DESC LIMIT 1`).Scan(&raw)
	if errors.Is(err, sql.ErrNoRows) {
		return "", nil, false, nil
	}
	if err != nil {
		return "", nil, false, fmt.Errorf("OpenCode credential could not be read: %w", err)
	}
	var credential openCodeCredential
	if err := json.Unmarshal([]byte(raw), &credential); err != nil {
		return "", nil, false, fmt.Errorf("OpenCode credential format is not recognized: %w", err)
	}
	switch credential.Type {
	case "key":
		if credential.Key == "" {
			return "", nil, false, nil
		}
		return credential.Key, credential.Metadata, true, nil
	case "oauth":
		expired := credential.Expires > 0 && credential.Expires <= f.now().UnixMilli()+300_000
		if credential.Access == "" || expired {
			return "", nil, false, nil
		}
		return credential.Access, credential.Metadata, true, nil
	default:
		return "", nil, false, nil
	}
}

type openCodeMeter struct {
	Status   string  `json:"status"`
	Percent  float64 `json:"percent"`
	ResetsAt *string `json:"resetsAt"`
}

type openCodeUsageResponse struct {
	Usage struct {
		Rolling *openCodeMeter `json:"rolling"`
		Weekly  *openCodeMeter `json:"weekly"`
		Monthly *openCodeMeter `json:"monthly"`
	} `json:"usage"`
}

func parseOpenCodeUsage(body []byte) (*provider.QuotaSnapshot, error) {
	var raw openCodeUsageResponse
	if err := json.Unmarshal(body, &raw); err != nil {
		return nil, fmt.Errorf("OpenCode Go usage response could not be parsed: %w", err)
	}
	fiveHours := int64((5 * time.Hour) / time.Millisecond)
	sevenDays := int64((7 * 24 * time.Hour) / time.Millisecond)
	thirtyDays := int64((30 * 24 * time.Hour) / time.Millisecond)
	windows := make([]provider.RateWindow, 0, 3)
	add := func(label string, period *int64, meter *openCodeMeter) {
		if meter == nil {
			return
		}
		used := math.Max(0, math.Min(100, meter.Percent))
		if meter.Status == "rate-limited" {
			used = 100
		}
		window := provider.RateWindow{Label: label, UsedPercent: used, PeriodMs: period}
		if meter.ResetsAt != nil {
			if text := strings.TrimSpace(*meter.ResetsAt); text != "" {
				if reset, err := time.Parse(time.RFC3339Nano, text); err == nil {
					window.ResetsAt = &reset
				}
			}
		}
		windows = append(windows, window)
	}
	add("5h", &fiveHours, raw.Usage.Rolling)
	add("Weekly", &sevenDays, raw.Usage.Weekly)
	add("Monthly", &thirtyDays, raw.Usage.Monthly)
	if len(windows) == 0 {
		return nil, nil
	}
	return &provider.QuotaSnapshot{Provider: "opencode", Windows: windows}, nil
}

func (f *OpenCodeFetcher) Fetch() (*provider.QuotaSnapshot, error) {
	token, metadata, ok, err := f.credential()
	if err != nil {
		return nil, err
	}
	if !ok {
		// No credential, an expired login, or no Go subscription means there is
		// nothing to show; an error card would be noise for every OpenCode user.
		return nil, nil
	}
	req, err := http.NewRequest(http.MethodGet, f.UsageURL, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/json")
	req.Header.Set("User-Agent", "tokenpulse/"+version.Version)
	if orgID := stringMetadata(metadata, "orgID"); orgID != "" {
		req.Header.Set("x-opencode-org-id", orgID)
	}
	client := &http.Client{Timeout: 20 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("OpenCode Go usage request failed: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden {
		return nil, nil
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("OpenCode Go usage request failed: HTTP %d", resp.StatusCode)
	}
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	snapshot, err := parseOpenCodeUsage(body)
	if err != nil {
		return nil, err
	}
	if snapshot == nil {
		return nil, nil
	}
	if f.ShowAccount {
		snapshot.Account = stringMetadata(metadata, "email")
	}
	snapshot.FetchedAt = f.now().UTC()
	return snapshot, nil
}

func stringMetadata(metadata map[string]any, key string) string {
	value, _ := metadata[key].(string)
	return value
}

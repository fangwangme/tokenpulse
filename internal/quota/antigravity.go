package quota

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

const (
	antigravityExpiry = "Antigravity: login expired — open Antigravity to refresh it"
	agyTimeout        = 15 * time.Second
)

// AntigravityFetcher reads quota from a running language server, the agy CLI,
// or Cloud Code with an unexpired cached token. It never refreshes or writes an
// agent credential.
type AntigravityFetcher struct {
	Home   string
	Now    func() time.Time
	Client *http.Client

	// SkipLanguageServer and SkipAgyCLI keep tests away from the machine's
	// real Antigravity installations.
	SkipLanguageServer bool
	SkipAgyCLI         bool
}

func NewAntigravityFetcher() *AntigravityFetcher {
	home, _ := os.UserHomeDir()
	return &AntigravityFetcher{Home: home, Now: time.Now, Client: &http.Client{Timeout: 20 * time.Second}}
}

func (f *AntigravityFetcher) ProviderID() string { return "antigravity" }

type agyQuotaResponse struct {
	Groups []agyQuotaGroup `json:"groups"`
}

type agyQuotaGroup struct {
	DisplayName string           `json:"displayName"`
	Name        string           `json:"name"`
	Buckets     []agyQuotaBucket `json:"buckets"`
}

// agyQuotaBucket accepts the camelCase language-server shape and the
// snake_case shape the agy CLI prints.
type agyQuotaBucket struct {
	BucketID          string
	Window            string
	RemainingFraction float64
	ResetTime         string
}

func (b *agyQuotaBucket) UnmarshalJSON(data []byte) error {
	var camel struct {
		BucketID          string   `json:"bucketId"`
		Window            string   `json:"window"`
		RemainingFraction *float64 `json:"remainingFraction"`
		ResetTime         *string  `json:"resetTime"`
	}
	var snake struct {
		BucketID          string   `json:"bucket_id"`
		Window            string   `json:"window"`
		RemainingFraction *float64 `json:"remaining_fraction"`
		ResetTime         *string  `json:"reset_time"`
	}
	if err := json.Unmarshal(data, &camel); err != nil {
		return err
	}
	if err := json.Unmarshal(data, &snake); err != nil {
		return err
	}
	b.BucketID = camel.BucketID
	if b.BucketID == "" {
		b.BucketID = snake.BucketID
	}
	b.Window = camel.Window
	if b.Window == "" {
		b.Window = snake.Window
	}
	if camel.RemainingFraction != nil {
		b.RemainingFraction = *camel.RemainingFraction
	} else if snake.RemainingFraction != nil {
		b.RemainingFraction = *snake.RemainingFraction
	}
	if camel.ResetTime != nil {
		b.ResetTime = *camel.ResetTime
	} else if snake.ResetTime != nil {
		b.ResetTime = *snake.ResetTime
	}
	return nil
}

func (f *AntigravityFetcher) Fetch() (*provider.QuotaSnapshot, error) {
	var connections []usage.AntigravityConnectionInfo
	if !f.SkipLanguageServer {
		connections, _ = usage.DetectAntigravityConnectionsForQuota()
	}
	ordered := orderConnections(connections)
	for _, connection := range ordered {
		response, err := usage.AntigravityRPC(connection, "RetrieveUserQuotaSummary", map[string]any{}, 20*time.Second)
		if err != nil {
			continue
		}
		summary, ok := extractQuotaGroups(response)
		if !ok {
			continue
		}
		windows := antigravityWindows(summary.groups)
		if len(windows) == 0 {
			continue
		}
		snapshot := &provider.QuotaSnapshot{Provider: "antigravity", Windows: windows, FetchedAt: f.Now().UTC()}
		if status, err := usage.AntigravityRPC(connection, "GetUserStatus", map[string]any{}, 10*time.Second); err == nil {
			snapshot.Account, snapshot.Plan = antigravityUserStatus(status)
		}
		return snapshot, nil
	}

	if !f.SkipAgyCLI {
		if snapshot := f.fetchAgyCLI(); snapshot != nil {
			return snapshot, nil
		}
	}

	creds, expired, found := f.loadCredentials()
	if !found {
		if len(connections) > 0 {
			return nil, errors.New("Antigravity quota unavailable: language server did not answer and no agy CLI is installed")
		}
		return nil, errors.New("Antigravity credentials not found")
	}
	if expired {
		return nil, errors.New(antigravityExpiry)
	}
	snapshot, err := f.fetchCloudCode(creds)
	if err != nil {
		return nil, err
	}
	return snapshot, nil
}

func orderConnections(connections []usage.AntigravityConnectionInfo) []usage.AntigravityConnectionInfo {
	var out []usage.AntigravityConnectionInfo
	for _, kind := range []string{"desktop", "cli"} {
		for _, connection := range connections {
			if connection.Runtime == kind {
				out = append(out, connection)
			}
		}
	}
	for _, connection := range connections {
		if connection.Runtime != "desktop" && connection.Runtime != "cli" {
			out = append(out, connection)
		}
	}
	return out
}

type quotaGroup struct {
	label   string
	buckets []struct {
		id                string
		window            string
		remainingFraction float64
		resetTime         string
	}
}

type quotaGroups struct{ groups []quotaGroup }

func extractQuotaGroups(response map[string]any) (quotaGroups, bool) {
	raw := response
	if inner, ok := response["response"].(map[string]any); ok {
		raw = inner
	}
	groupsRaw, ok := raw["groups"].([]any)
	if !ok {
		return quotaGroups{}, false
	}
	var groups []quotaGroup
	for _, groupRaw := range groupsRaw {
		groupMap, _ := groupRaw.(map[string]any)
		if groupMap == nil {
			continue
		}
		displayName, _ := groupMap["displayName"].(string)
		if displayName == "" {
			displayName, _ = groupMap["name"].(string)
		}
		group := quotaGroup{label: cleanGroupLabel(displayName)}
		bucketsRaw, _ := groupMap["buckets"].([]any)
		for _, bucketRaw := range bucketsRaw {
			bucketMap, _ := bucketRaw.(map[string]any)
			if bucketMap == nil {
				continue
			}
			bucketID, _ := bucketMap["bucketId"].(string)
			window, _ := bucketMap["window"].(string)
			remaining, _ := bucketMap["remainingFraction"].(float64)
			reset, _ := bucketMap["resetTime"].(string)
			if reset == "" {
				if r, ok := bucketMap["reset_time"].(string); ok {
					reset = r
				}
			}
			if window == "" {
				if w, ok := bucketMap["window"].(string); ok {
					window = w
				}
			}
			if remaining == 0 {
				if r, ok := bucketMap["remaining_fraction"].(float64); ok {
					remaining = r
				}
			}
			group.buckets = append(group.buckets, struct {
				id                string
				window            string
				remainingFraction float64
				resetTime         string
			}{bucketID, window, remaining, reset})
		}
		groups = append(groups, group)
	}
	return quotaGroups{groups: groups}, true
}

func cleanGroupLabel(displayName string) string {
	trimmed := strings.TrimSpace(displayName)
	lower := strings.ToLower(trimmed)
	if lower == "gemini models" || lower == "gemini" {
		return "Gemini"
	}
	if strings.Contains(lower, "claude") || strings.Contains(lower, "gpt") {
		return "Claude"
	}
	cleaned := trimmed
	if strings.HasSuffix(lower, " models") {
		cleaned = strings.TrimSpace(trimmed[:len(trimmed)-len(" models")])
	}
	if cleaned == "" {
		return "Usage"
	}
	return cleaned
}

func modelFamily(label string) string {
	if label == "Usage" {
		return ""
	}
	return label
}

func antigravityWindows(groups []quotaGroup) []provider.RateWindow {
	type built struct {
		groupRank  int
		groupIdx   int
		windowRank int
		window     provider.RateWindow
	}
	var windows []built
	for groupIdx, group := range groups {
		rank := 2
		if group.label == "Gemini" {
			rank = 0
		} else if group.label == "Claude" {
			rank = 1
		}
		for _, bucket := range group.buckets {
			suffix, windowRank, period := windowDescriptor(bucket.window)
			remaining := bucket.remainingFraction
			if remaining < 0 {
				remaining = 0
			}
			if remaining > 1 {
				remaining = 1
			}
			window := provider.RateWindow{
				ID: bucket.id, Label: group.label + " (" + suffix + ")",
				Family: modelFamily(group.label), UsedPercent: (1 - remaining) * 100,
				PeriodMs: period,
			}
			if reset, ok := parseTime(bucket.resetTime); ok {
				window.ResetsAt = &reset
			}
			windows = append(windows, built{rank, groupIdx, windowRank, window})
		}
	}
	stableSort(windows, func(a, b built) bool {
		if a.groupRank != b.groupRank {
			return a.groupRank < b.groupRank
		}
		if a.groupIdx != b.groupIdx {
			return a.groupIdx < b.groupIdx
		}
		return a.windowRank < b.windowRank
	})
	out := make([]provider.RateWindow, 0, len(windows))
	for _, item := range windows {
		out = append(out, item.window)
	}
	return out
}

func stableSort[T any](values []T, less func(a, b T) bool) {
	for i := 1; i < len(values); i++ {
		for j := i; j > 0 && less(values[j], values[j-1]); j-- {
			values[j], values[j-1] = values[j-1], values[j]
		}
	}
}

func windowDescriptor(window string) (string, int, *int64) {
	switch strings.ToLower(window) {
	case "5h":
		period := int64(5 * 60 * 60 * 1000)
		return "5h", 0, &period
	case "weekly":
		period := int64(7 * 24 * 60 * 60 * 1000)
		return "7d", 1, &period
	default:
		return window, 2, nil
	}
}

func antigravityUserStatus(response map[string]any) (string, string) {
	status, _ := response["userStatus"].(map[string]any)
	if status == nil {
		return "", ""
	}
	account, _ := status["email"].(string)
	var plan string
	if tier, ok := status["userTier"].(map[string]any); ok {
		plan, _ = tier["name"].(string)
	}
	if strings.TrimSpace(plan) == "" {
		if planStatus, ok := status["planStatus"].(map[string]any); ok {
			if planInfo, ok := planStatus["planInfo"].(map[string]any); ok {
				plan, _ = planInfo["planName"].(string)
			}
		}
	}
	return strings.TrimSpace(account), strings.TrimSpace(plan)
}

// ── agy CLI ──

func (f *AntigravityFetcher) fetchAgyCLI() *provider.QuotaSnapshot {
	if !agySupported() {
		return nil
	}
	cmd := exec.Command("agy", "-p", "/usage", "--output-format", "json")
	cmd.Stdin = nil
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	done := make(chan error, 1)
	if err := cmd.Start(); err != nil {
		return nil
	}
	go func() { done <- cmd.Wait() }()
	select {
	case err := <-done:
		if err != nil {
			return nil
		}
	case <-time.After(agyTimeout):
		_ = cmd.Process.Kill()
		return nil
	}
	var wrapper struct {
		Command struct {
			Data agyQuotaResponse `json:"data"`
		} `json:"command"`
	}
	if json.Unmarshal(stdout.Bytes(), &wrapper) != nil {
		return nil
	}
	var groups []quotaGroup
	for _, group := range wrapper.Command.Data.Groups {
		label := group.DisplayName
		if label == "" {
			label = group.Name
		}
		qg := quotaGroup{label: cleanGroupLabel(label)}
		for _, bucket := range group.Buckets {
			qg.buckets = append(qg.buckets, struct {
				id                string
				window            string
				remainingFraction float64
				resetTime         string
			}{bucket.BucketID, bucket.Window, bucket.RemainingFraction, bucket.ResetTime})
		}
		groups = append(groups, qg)
	}
	windows := antigravityWindows(groups)
	if len(windows) == 0 {
		return nil
	}
	snapshot := &provider.QuotaSnapshot{Provider: "antigravity", Windows: windows, FetchedAt: f.Now().UTC()}
	if idToken := f.readCachedIDToken(); idToken != "" {
		snapshot.Account = jwtEmail(idToken)
	}
	return snapshot
}

func agySupported() bool {
	cmd := exec.Command("agy", "--version")
	cmd.Stdin = nil
	out, err := cmd.Output()
	if err != nil {
		return false
	}
	version := strings.TrimSpace(string(out))
	parts := strings.Split(version, ".")
	if len(parts) < 2 {
		return false
	}
	major, err1 := strconv.Atoi(parts[0])
	minor, err2 := strconv.Atoi(parts[1])
	if err1 != nil || err2 != nil {
		return false
	}
	return major > 1 || (major == 1 && minor >= 4)
}

// ── Cloud Code ──

type antigravityCredentials struct {
	Token struct {
		AccessToken string `json:"access_token"`
		Expiry      string `json:"expiry"`
	} `json:"token"`
	IDToken string `json:"id_token"`
}

func (f *AntigravityFetcher) tokenPath() string {
	if override := os.Getenv("TOKENPULSE_ANTIGRAVITY_AUTH_PATH"); override != "" {
		return override
	}
	return filepath.Join(f.Home, ".gemini", "antigravity-cli", "antigravity-oauth-token")
}

func (f *AntigravityFetcher) loadCredentials() (antigravityCredentials, bool, bool) {
	cachePath := f.tokenPath()
	if content, err := os.ReadFile(cachePath); err == nil {
		var creds antigravityCredentials
		if json.Unmarshal(content, &creds) == nil && creds.Token.AccessToken != "" {
			return creds, f.tokenExpired(creds.Token.Expiry), true
		}
	}
	if runtime.GOOS == "darwin" {
		if out, err := exec.Command("/usr/bin/security", "find-generic-password", "-s", "gemini", "-a", "antigravity", "-w").Output(); err == nil {
			value := strings.TrimSpace(string(out))
			if payload, ok := strings.CutPrefix(value, "go-keyring-base64:"); ok {
				if decoded, err := base64.StdEncoding.DecodeString(payload); err == nil {
					var creds antigravityCredentials
					if json.Unmarshal(decoded, &creds) == nil && creds.Token.AccessToken != "" {
						return creds, f.tokenExpired(creds.Token.Expiry), true
					}
				}
			}
		}
	}
	return antigravityCredentials{}, false, false
}

func (f *AntigravityFetcher) tokenExpired(expiry string) bool {
	t, err := time.Parse(time.RFC3339Nano, expiry)
	if err != nil {
		return true
	}
	return !t.After(f.Now().Add(5 * time.Minute))
}

func (f *AntigravityFetcher) readCachedIDToken() string {
	content, err := os.ReadFile(f.tokenPath())
	if err != nil {
		return ""
	}
	var value struct {
		IDToken string `json:"id_token"`
	}
	if json.Unmarshal(content, &value) != nil {
		return ""
	}
	return value.IDToken
}

func (f *AntigravityFetcher) fetchCloudCode(creds antigravityCredentials) (*provider.QuotaSnapshot, error) {
	userAgent := "antigravity/hub/2.9.1 " + runtime.GOOS + "/" + runtime.GOARCH
	loadBody := map[string]any{"metadata": map[string]any{
		"ideType": "ANTIGRAVITY", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI",
	}}
	projectID, plan := f.loadCodeAssist(creds.Token.AccessToken, userAgent, loadBody)
	body := map[string]any{}
	if projectID != "" {
		body["project"] = projectID
	}
	response, status, err := f.postJSON("https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
		creds.Token.AccessToken, userAgent, body)
	if err != nil {
		return nil, err
	}
	if status == http.StatusUnauthorized || status == http.StatusForbidden {
		return nil, errors.New(antigravityExpiry)
	}
	if status != http.StatusOK {
		return nil, fmt.Errorf("Antigravity Quota API error %d", status)
	}
	var responseMap map[string]any
	if json.Unmarshal(response, &responseMap) != nil {
		return nil, errors.New("Failed to parse Antigravity quota response")
	}
	groups, ok := extractQuotaGroups(responseMap)
	if !ok {
		var unwrapped agyQuotaResponse
		if json.Unmarshal(response, &unwrapped) != nil {
			return nil, errors.New("Failed to parse Antigravity quota response")
		}
		for _, group := range unwrapped.Groups {
			label := group.DisplayName
			if label == "" {
				label = group.Name
			}
			qg := quotaGroup{label: cleanGroupLabel(label)}
			for _, bucket := range group.Buckets {
				qg.buckets = append(qg.buckets, struct {
					id                string
					window            string
					remainingFraction float64
					resetTime         string
				}{bucket.BucketID, bucket.Window, bucket.RemainingFraction, bucket.ResetTime})
			}
			groups.groups = append(groups.groups, qg)
		}
	}
	windows := antigravityWindows(groups.groups)
	if len(windows) == 0 {
		return nil, errors.New("No active quota windows found in API response")
	}
	return &provider.QuotaSnapshot{
		Provider: "antigravity", Plan: plan, Account: jwtEmail(creds.IDToken),
		Windows: windows, FetchedAt: f.Now().UTC(),
	}, nil
}

func (f *AntigravityFetcher) loadCodeAssist(accessToken, userAgent string, body map[string]any) (string, string) {
	response, status, err := f.postJSON("https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
		accessToken, userAgent, body)
	if err != nil || status != http.StatusOK {
		return "", ""
	}
	var parsed struct {
		Project  any `json:"cloudaicompanionProject"`
		PaidTier *struct {
			Name string `json:"name"`
		} `json:"paidTier"`
		CurrentTier *struct {
			Name string `json:"name"`
		} `json:"currentTier"`
	}
	if json.Unmarshal(response, &parsed) != nil {
		return "", ""
	}
	project := ""
	switch v := parsed.Project.(type) {
	case string:
		project = strings.TrimSpace(v)
	case map[string]any:
		if id, ok := v["id"].(string); ok {
			project = strings.TrimSpace(id)
		}
	}
	plan := ""
	if parsed.PaidTier != nil {
		plan = strings.TrimSpace(parsed.PaidTier.Name)
	}
	if plan == "" && parsed.CurrentTier != nil {
		plan = strings.TrimSpace(parsed.CurrentTier.Name)
	}
	return project, plan
}

func (f *AntigravityFetcher) postJSON(url, accessToken, userAgent string, body map[string]any) ([]byte, int, error) {
	data, err := json.Marshal(body)
	if err != nil {
		return nil, 0, err
	}
	req, err := http.NewRequest(http.MethodPost, url, bytes.NewReader(data))
	if err != nil {
		return nil, 0, err
	}
	req.Header.Set("Authorization", "Bearer "+accessToken)
	req.Header.Set("User-Agent", userAgent)
	req.Header.Set("Content-Type", "application/json")
	resp, err := f.Client.Do(req)
	if err != nil {
		return nil, 0, err
	}
	defer resp.Body.Close()
	payload, err := io.ReadAll(resp.Body)
	return payload, resp.StatusCode, err
}

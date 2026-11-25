package quota

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

const (
	codexUsageURL   = "https://chatgpt.com/backend-api/wham/usage"
	codexCreditsURL = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits"
	codexExpiry     = "Codex: login expired — open Codex to refresh it"
)

// CodexFetcher reads Codex's auth.json and usage endpoints. It never refreshes
// or writes the credential.
type CodexFetcher struct {
	Home       string
	UsageURL   string
	CreditsURL string
	Now        func() time.Time
}

func NewCodexFetcher() *CodexFetcher {
	home, _ := os.UserHomeDir()
	return &CodexFetcher{Home: home, UsageURL: codexUsageURL, CreditsURL: codexCreditsURL, Now: time.Now}
}

func (f *CodexFetcher) ProviderID() string { return "codex" }

type codexCredentials struct {
	Tokens *struct {
		AccessToken  string `json:"access_token"`
		RefreshToken string `json:"refresh_token"`
		IDToken      string `json:"id_token"`
	} `json:"tokens"`
	OpenAIAPIKey string `json:"OPENAI_API_KEY"`
}

func (f *CodexFetcher) authPaths() []string {
	var paths []string
	if codexHome := strings.TrimSpace(os.Getenv("CODEX_HOME")); codexHome != "" {
		paths = append(paths, filepath.Join(codexHome, "auth.json"))
	}
	paths = append(paths, filepath.Join(f.Home, ".config", "codex", "auth.json"))
	alternate := filepath.Join(f.Home, ".codex", "auth.json")
	duplicate := false
	for _, path := range paths {
		if path == alternate {
			duplicate = true
		}
	}
	if !duplicate {
		paths = append(paths, alternate)
	}
	return paths
}

func (f *CodexFetcher) load() (string, codexCredentials, bool) {
	for _, path := range f.authPaths() {
		content, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		var creds codexCredentials
		if json.Unmarshal(content, &creds) != nil {
			continue
		}
		if creds.Tokens != nil || creds.OpenAIAPIKey != "" {
			return path, creds, true
		}
	}
	return "", codexCredentials{}, false
}

func (f *CodexFetcher) Fetch() (*provider.QuotaSnapshot, error) {
	_, creds, ok := f.load()
	if !ok {
		return nil, errors.New("Codex credentials not found")
	}
	if creds.Tokens == nil || creds.Tokens.AccessToken == "" {
		return nil, errors.New("No tokens found in Codex credentials")
	}
	tokens := creds.Tokens
	if exp, ok := jwtExpiry(tokens.AccessToken); ok {
		if exp <= f.Now().Unix()+300 {
			return nil, errors.New(codexExpiry)
		}
	}
	client := &http.Client{Timeout: 20 * time.Second}
	req, _ := http.NewRequest(http.MethodGet, f.UsageURL, nil)
	req.Header.Set("Authorization", "Bearer "+tokens.AccessToken)
	req.Header.Set("Accept", "application/json")
	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("Codex usage request failed: %w", err)
	}
	if resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden {
		resp.Body.Close()
		return nil, errors.New(codexExpiry)
	}
	if resp.StatusCode != http.StatusOK {
		resp.Body.Close()
		return nil, fmt.Errorf("Codex Quota API error %d", resp.StatusCode)
	}
	body, err := io.ReadAll(resp.Body)
	resp.Body.Close()
	if err != nil {
		return nil, err
	}
	snapshot, err := parseCodexUsage(body)
	if err != nil {
		return nil, err
	}
	snapshot.Account = jwtEmail(tokens.IDToken)
	snapshot.RateLimitResetCredits = f.fetchResetCredits(tokens.AccessToken, client)
	return snapshot, nil
}

func (f *CodexFetcher) fetchResetCredits(accessToken string, client *http.Client) []provider.RateLimitResetCredit {
	req, err := http.NewRequest(http.MethodGet, f.CreditsURL, nil)
	if err != nil {
		return nil
	}
	req.Header.Set("Authorization", "Bearer "+accessToken)
	req.Header.Set("Accept", "application/json")
	resp, err := client.Do(req)
	if err != nil {
		return nil
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil
	}
	var body struct {
		Credits []struct {
			ID        string     `json:"id"`
			ResetType string     `json:"reset_type"`
			Status    string     `json:"status"`
			GrantedAt *time.Time `json:"granted_at"`
			ExpiresAt *time.Time `json:"expires_at"`
		} `json:"credits"`
	}
	if json.NewDecoder(resp.Body).Decode(&body) != nil {
		return nil
	}
	var out []provider.RateLimitResetCredit
	for _, credit := range body.Credits {
		if credit.Status != "available" {
			continue
		}
		out = append(out, provider.RateLimitResetCredit{
			ID: credit.ID, ResetType: credit.ResetType, Status: credit.Status,
			GrantedAt: credit.GrantedAt, ExpiresAt: credit.ExpiresAt,
		})
	}
	return out
}

func parseCodexUsage(body []byte) (*provider.QuotaSnapshot, error) {
	var response struct {
		RateLimit *struct {
			PrimaryWindow *struct {
				UsedPercent        any    `json:"used_percent"`
				LimitWindowSeconds *int64 `json:"limit_window_seconds"`
				ResetAfterSeconds  *int64 `json:"reset_after_seconds"`
				ResetAt            *int64 `json:"reset_at"`
			} `json:"primary_window"`
			SecondaryWindow *struct {
				UsedPercent        any    `json:"used_percent"`
				LimitWindowSeconds *int64 `json:"limit_window_seconds"`
				ResetAfterSeconds  *int64 `json:"reset_after_seconds"`
				ResetAt            *int64 `json:"reset_at"`
			} `json:"secondary_window"`
		} `json:"rate_limit"`
		PlanType string `json:"plan_type"`
	}
	if err := json.Unmarshal(body, &response); err != nil {
		return nil, fmt.Errorf("Failed to parse Codex quota response: %w", err)
	}
	snapshot := &provider.QuotaSnapshot{Provider: "codex", Plan: response.PlanType, Windows: []provider.RateWindow{}, FetchedAt: time.Now().UTC()}
	if response.RateLimit == nil {
		return snapshot, nil
	}
	add := func(position, label string, used any, seconds *int64, resetAfter *int64, resetAt *int64) {
		if used == nil {
			return
		}
		window := provider.RateWindow{
			Label:       codexWindowLabel(position, seconds),
			UsedPercent: flexNumber(used),
		}
		if seconds != nil && *seconds > 0 {
			period := *seconds * 1000
			window.PeriodMs = &period
		}
		if resetAt != nil {
			t := time.Unix(*resetAt, 0).UTC()
			window.ResetsAt = &t
		} else if resetAfter != nil {
			t := time.Now().UTC().Add(time.Duration(*resetAfter) * time.Second)
			window.ResetsAt = &t
		}
		snapshot.Windows = append(snapshot.Windows, window)
	}
	if primary := response.RateLimit.PrimaryWindow; primary != nil {
		add("primary", "Primary window", primary.UsedPercent, primary.LimitWindowSeconds, primary.ResetAfterSeconds, primary.ResetAt)
	}
	if secondary := response.RateLimit.SecondaryWindow; secondary != nil {
		add("secondary", "Secondary window", secondary.UsedPercent, secondary.LimitWindowSeconds, secondary.ResetAfterSeconds, secondary.ResetAt)
	}
	return snapshot, nil
}

func flexNumber(value any) float64 {
	switch v := value.(type) {
	case float64:
		return v
	case string:
		parsed, _ := strconv.ParseFloat(v, 64)
		return parsed
	}
	return 0
}

func codexWindowLabel(position string, seconds *int64) string {
	if seconds != nil {
		switch *seconds {
		case 18_000:
			return "Session (5h)"
		case 604_800:
			return "Weekly (7d)"
		}
		if *seconds > 0 {
			return "Window (" + formatWindowDuration(*seconds) + ")"
		}
	}
	if position == "primary" {
		return "Primary window"
	}
	return "Secondary window"
}

func formatWindowDuration(seconds int64) string {
	const day = 24 * 60 * 60
	const hour = 60 * 60
	const minute = 60
	switch {
	case seconds%day == 0:
		return fmt.Sprintf("%dd", seconds/day)
	case seconds%hour == 0:
		return fmt.Sprintf("%dh", seconds/hour)
	case seconds%minute == 0:
		return fmt.Sprintf("%dm", seconds/minute)
	default:
		return fmt.Sprintf("%ds", seconds)
	}
}

func jwtPayload(token string) map[string]any {
	parts := strings.Split(token, ".")
	if len(parts) < 2 {
		return nil
	}
	payload, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return nil
	}
	var value map[string]any
	if json.Unmarshal(payload, &value) != nil {
		return nil
	}
	return value
}

func jwtExpiry(token string) (int64, bool) {
	payload := jwtPayload(token)
	if payload == nil {
		return 0, false
	}
	switch v := payload["exp"].(type) {
	case float64:
		return int64(v), true
	}
	return 0, false
}

func jwtEmail(token string) string {
	payload := jwtPayload(token)
	if payload == nil {
		return ""
	}
	email, _ := payload["email"].(string)
	return strings.TrimSpace(email)
}

package quota

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"os/user"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

const claudeUsageURL = "https://api.anthropic.com/api/oauth/usage"

const claudeExpiry = "Claude: login expired — open Claude Code to refresh it"

// ClaudeFetcher reads Claude Code's credential and usage endpoint. It never
// refreshes or writes the credential.
type ClaudeFetcher struct {
	Home        string
	UsageURL    string
	ShowAccount bool
	Keychain    bool
	Now         func() time.Time
}

func NewClaudeFetcher(showAccount bool) *ClaudeFetcher {
	home, _ := os.UserHomeDir()
	return &ClaudeFetcher{
		Home: home, UsageURL: claudeUsageURL, ShowAccount: showAccount,
		Keychain: true, Now: time.Now,
	}
}

func (f *ClaudeFetcher) ProviderID() string { return "claude" }

type claudeCredentials struct {
	ClaudeAIOAuth struct {
		AccessToken      string   `json:"accessToken"`
		RefreshToken     string   `json:"refreshToken"`
		ExpiresAt        float64  `json:"expiresAt"`
		SubscriptionType string   `json:"subscriptionType"`
		Scopes           []string `json:"scopes"`
	} `json:"claudeAiOauth"`
}

type claudeCandidate struct {
	source string
	creds  claudeCredentials
}

func (f *ClaudeFetcher) candidates() []claudeCandidate {
	var out []claudeCandidate
	appendUnique := func(candidate claudeCandidate) {
		for _, existing := range out {
			if existing.creds.ClaudeAIOAuth.AccessToken == candidate.creds.ClaudeAIOAuth.AccessToken &&
				existing.creds.ClaudeAIOAuth.ExpiresAt == candidate.creds.ClaudeAIOAuth.ExpiresAt {
				return
			}
		}
		out = append(out, candidate)
	}
	if f.Keychain && runtime.GOOS == "darwin" {
		if current, err := user.Current(); err == nil && current.Username != "" {
			if creds, ok := readClaudeKeychain(current.Username); ok {
				appendUnique(claudeCandidate{"current-user keychain", creds})
			}
		}
		if creds, ok := readClaudeKeychain(""); ok {
			appendUnique(claudeCandidate{"legacy keychain", creds})
		}
	}
	if creds, ok := readClaudeFile(filepath.Join(f.Home, ".claude", ".credentials.json")); ok {
		appendUnique(claudeCandidate{"credentials file", creds})
	}
	return out
}

func readClaudeFile(path string) (claudeCredentials, bool) {
	content, err := os.ReadFile(path)
	if err != nil {
		return claudeCredentials{}, false
	}
	var creds claudeCredentials
	if json.Unmarshal(content, &creds) != nil || creds.ClaudeAIOAuth.AccessToken == "" {
		return claudeCredentials{}, false
	}
	return creds, true
}

func readClaudeKeychain(account string) (claudeCredentials, bool) {
	args := []string{"find-generic-password", "-s", "Claude Code-credentials"}
	if account != "" {
		args = append(args, "-a", account)
	}
	args = append(args, "-w")
	out, err := exec.Command("/usr/bin/security", args...).Output()
	if err != nil {
		return claudeCredentials{}, false
	}
	var creds claudeCredentials
	if json.Unmarshal([]byte(strings.TrimSpace(string(out))), &creds) != nil || creds.ClaudeAIOAuth.AccessToken == "" {
		return claudeCredentials{}, false
	}
	return creds, true
}

func (f *ClaudeFetcher) expired(creds claudeCredentials) bool {
	if creds.ClaudeAIOAuth.ExpiresAt == 0 {
		return false
	}
	now := f.Now().UnixMilli()
	return int64(creds.ClaudeAIOAuth.ExpiresAt) <= now+300_000
}

func (f *ClaudeFetcher) Fetch() (*provider.QuotaSnapshot, error) {
	candidates := f.candidates()
	if len(candidates) == 0 {
		return nil, errors.New("Claude credentials not found")
	}
	var lastErr error
	tried := false
	for _, candidate := range candidates {
		if f.expired(candidate.creds) {
			lastErr = errors.New(claudeExpiry)
			continue
		}
		tried = true
		snapshot, err := f.fetchWithToken(candidate.creds)
		if err == nil {
			return snapshot, nil
		}
		lastErr = err
	}
	if !tried && lastErr == nil {
		lastErr = errors.New(claudeExpiry)
	}
	if lastErr == nil {
		lastErr = errors.New(claudeExpiry)
	}
	return nil, lastErr
}

func (f *ClaudeFetcher) fetchWithToken(creds claudeCredentials) (*provider.QuotaSnapshot, error) {
	req, err := http.NewRequest(http.MethodGet, f.UsageURL, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+creds.ClaudeAIOAuth.AccessToken)
	req.Header.Set("anthropic-beta", "oauth-2025-04-20")
	req.Header.Set("Accept", "application/json")
	client := &http.Client{Timeout: 20 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("Claude usage request failed: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden {
		return nil, errors.New(claudeExpiry)
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("Claude usage request failed with HTTP %d", resp.StatusCode)
	}
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	snapshot, err := parseClaudeUsage(body)
	if err != nil {
		return nil, err
	}
	snapshot.Plan = strings.TrimSpace(creds.ClaudeAIOAuth.SubscriptionType)
	if f.ShowAccount {
		snapshot.Account = readClaudeAccount(f.Home)
	}
	return snapshot, nil
}

func readClaudeAccount(home string) string {
	content, err := os.ReadFile(filepath.Join(home, ".claude.json"))
	if err != nil {
		return ""
	}
	var global struct {
		OAuthAccount struct {
			EmailAddress string `json:"emailAddress"`
		} `json:"oauthAccount"`
	}
	if json.Unmarshal(content, &global) != nil {
		return ""
	}
	return strings.TrimSpace(global.OAuthAccount.EmailAddress)
}

func parseClaudeUsage(body []byte) (*provider.QuotaSnapshot, error) {
	var response struct {
		FiveHour *struct {
			Utilization float64 `json:"utilization"`
			ResetsAt    string  `json:"resets_at"`
		} `json:"five_hour"`
		SevenDay *struct {
			Utilization float64 `json:"utilization"`
			ResetsAt    string  `json:"resets_at"`
		} `json:"seven_day"`
		SevenDaySonnet *struct {
			Utilization float64 `json:"utilization"`
			ResetsAt    string  `json:"resets_at"`
		} `json:"seven_day_sonnet"`
		SevenDayOpus *struct {
			Utilization float64 `json:"utilization"`
			ResetsAt    string  `json:"resets_at"`
		} `json:"seven_day_opus"`
		Limits []struct {
			Kind     string  `json:"kind"`
			Percent  float64 `json:"percent"`
			ResetsAt string  `json:"resets_at"`
			Scope    *struct {
				Model *struct {
					DisplayName string `json:"display_name"`
				} `json:"model"`
			} `json:"scope"`
		} `json:"limits"`
		ExtraUsage *struct {
			IsEnabled     bool     `json:"is_enabled"`
			UsedCredits   *float64 `json:"used_credits"`
			MonthlyLimit  *float64 `json:"monthly_limit"`
			DecimalPlaces *int     `json:"decimal_places"`
			Currency      string   `json:"currency"`
		} `json:"extra_usage"`
	}
	if err := json.Unmarshal(body, &response); err != nil {
		return nil, fmt.Errorf("Failed to parse Claude quota response: %w", err)
	}
	snapshot := &provider.QuotaSnapshot{Provider: "claude", Windows: []provider.RateWindow{}, FetchedAt: time.Now().UTC()}
	week := int64(7 * 24 * 60 * 60 * 1000)
	fiveHours := int64(5 * 60 * 60 * 1000)
	add := func(label, family string, used float64, resets string, period int64) {
		window := provider.RateWindow{Label: label, Family: family, UsedPercent: used, PeriodMs: &period}
		if t, ok := parseTime(resets); ok {
			window.ResetsAt = &t
		}
		snapshot.Windows = append(snapshot.Windows, window)
	}
	if response.FiveHour != nil {
		add("Session (5h)", "", response.FiveHour.Utilization, response.FiveHour.ResetsAt, fiveHours)
	}
	if response.SevenDay != nil {
		add("Weekly (7d)", "", response.SevenDay.Utilization, response.SevenDay.ResetsAt, week)
	}
	if response.SevenDaySonnet != nil {
		add("Sonnet (7d)", "Sonnet", response.SevenDaySonnet.Utilization, response.SevenDaySonnet.ResetsAt, week)
	}
	if response.SevenDayOpus != nil {
		add("Opus (7d)", "Opus", response.SevenDayOpus.Utilization, response.SevenDayOpus.ResetsAt, week)
	}
	for _, limit := range response.Limits {
		name := ""
		if limit.Scope != nil && limit.Scope.Model != nil {
			name = limit.Scope.Model.DisplayName
		}
		if name == "Fable" && limit.Kind == "weekly_scoped" {
			add("Fable (7d)", "Fable", limit.Percent, limit.ResetsAt, week)
		}
	}
	if extra := response.ExtraUsage; extra != nil && extra.IsEnabled {
		scale := 100.0
		if extra.DecimalPlaces != nil && *extra.DecimalPlaces >= 0 {
			scale = 1
			for i := 0; i < *extra.DecimalPlaces; i++ {
				scale *= 10
			}
		}
		credit := &provider.CreditInfo{Currency: extra.Currency}
		if extra.UsedCredits != nil {
			credit.Used = *extra.UsedCredits / scale
		}
		if extra.MonthlyLimit != nil {
			limit := *extra.MonthlyLimit / scale
			credit.Limit = &limit
		}
		if credit.Currency == "" {
			credit.Currency = "USD"
		}
		snapshot.ExtraUsage = credit
	}
	return snapshot, nil
}

func parseTime(value string) (time.Time, bool) {
	if value == "" {
		return time.Time{}, false
	}
	if t, err := time.Parse(time.RFC3339Nano, value); err == nil {
		return t.UTC(), true
	}
	return time.Time{}, false
}

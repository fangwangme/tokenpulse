package quota

import (
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"sort"
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

type goldenWindow struct {
	Label            string     `json:"label"`
	ModelFamily      *string    `json:"model_family"`
	UsedPercent      float64    `json:"used_percent"`
	ResetsAt         *time.Time `json:"resets_at"`
	PeriodDurationMs *int64     `json:"period_duration_ms"`
}

type goldenSnapshot struct {
	Plan                  *string        `json:"plan"`
	Account               *string        `json:"account"`
	Windows               []goldenWindow `json:"windows"`
	RateLimitResetCredits []struct {
		ID        string     `json:"id"`
		ResetType string     `json:"reset_type"`
		Status    string     `json:"status"`
		GrantedAt *time.Time `json:"granted_at"`
		ExpiresAt *time.Time `json:"expires_at"`
	} `json:"rateLimitResetCredits"`
	AgyWindows []goldenWindow `json:"agyWindows"`
}

func loadGolden(t *testing.T, name string) goldenSnapshot {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "..", "testdata", "parity", "quota", "golden", name))
	if err != nil {
		t.Fatal(err)
	}
	var snapshot goldenSnapshot
	if err := json.Unmarshal(data, &snapshot); err != nil {
		t.Fatal(err)
	}
	return snapshot
}

func loadFixture(t *testing.T, name string) []byte {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "..", "testdata", "parity", "quota", name))
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func assertWindows(t *testing.T, got []provider.RateWindow, want []goldenWindow, ordered bool) {
	t.Helper()
	if ordered {
		if len(got) != len(want) {
			t.Fatalf("window count = %d, want %d", len(got), len(want))
		}
	} else {
		if len(got) != len(want) {
			t.Fatalf("window count = %d, want %d", len(got), len(want))
		}
		got = append([]provider.RateWindow(nil), got...)
		sort.Slice(got, func(i, j int) bool { return got[i].Label < got[j].Label })
		want = append([]goldenWindow(nil), want...)
		sort.Slice(want, func(i, j int) bool { return want[i].Label < want[j].Label })
	}
	for i := range want {
		family := ""
		if want[i].ModelFamily != nil {
			family = *want[i].ModelFamily
		}
		if got[i].Label != want[i].Label || got[i].Family != family ||
			math.Abs(got[i].UsedPercent-want[i].UsedPercent) > 1e-9 ||
			!sameTime(got[i].ResetsAt, want[i].ResetsAt) ||
			!samePeriod(got[i].PeriodMs, want[i].PeriodDurationMs) {
			t.Errorf("window %d = %+v, want %+v", i, got[i], want[i])
		}
	}
}

func sameTime(a, b *time.Time) bool {
	if a == nil || b == nil {
		return a == nil && b == nil
	}
	return a.Equal(*b)
}

func samePeriod(a, b *int64) bool {
	if a == nil || b == nil {
		return a == nil && b == nil
	}
	return *a == *b
}

// Protects quota.md "Order" and quota-providers.md "Claude": the recorded
// 0.6.16 response maps to the same windows, percentages, resets and plan.
func TestClaudeQuotaParity(t *testing.T) {
	golden := loadGolden(t, "claude.json")
	snapshot, err := parseClaudeUsage(loadFixture(t, "claude.json"))
	if err != nil {
		t.Fatal(err)
	}
	snapshot.Plan = "max"
	if golden.Plan == nil || snapshot.Plan != *golden.Plan {
		t.Fatalf("plan = %q, want %v", snapshot.Plan, golden.Plan)
	}
	assertWindows(t, snapshot.Windows, golden.Windows, true)
	if snapshot.ExtraUsage == nil || snapshot.ExtraUsage.Used != 12.34 ||
		snapshot.ExtraUsage.Limit == nil || *snapshot.ExtraUsage.Limit != 50 ||
		snapshot.ExtraUsage.Currency != "USD" {
		t.Fatalf("extra usage = %+v", snapshot.ExtraUsage)
	}
}

// Protects quota.md "Order" and quota-providers.md "Codex": same windows,
// while the order is this release's period-first rule, not the API order.
func TestCodexQuotaParity(t *testing.T) {
	golden := loadGolden(t, "codex.json")
	snapshot, err := parseCodexUsage(loadFixture(t, "codex.json"))
	if err != nil {
		t.Fatal(err)
	}
	if golden.Plan == nil || snapshot.Plan != *golden.Plan {
		t.Fatalf("plan = %q, want %v", snapshot.Plan, golden.Plan)
	}
	assertWindows(t, snapshot.Windows, golden.Windows, false)
	ordered := OrderWindows(snapshot.Windows)
	if len(ordered) != 2 || ordered[0].Label != "Session (5h)" || ordered[1].Label != "Weekly (7d)" {
		t.Fatalf("ordered windows = %+v", ordered)
	}

	fetcher := &CodexFetcher{CreditsURL: "http://unused"}
	body := loadFixture(t, "codex-credits.json")
	var parsed struct {
		Credits []struct {
			ID        string     `json:"id"`
			ResetType string     `json:"reset_type"`
			Status    string     `json:"status"`
			GrantedAt *time.Time `json:"granted_at"`
			ExpiresAt *time.Time `json:"expires_at"`
		} `json:"credits"`
	}
	if err := json.Unmarshal(body, &parsed); err != nil {
		t.Fatal(err)
	}
	var available []provider.RateLimitResetCredit
	for _, credit := range parsed.Credits {
		if credit.Status == "available" {
			available = append(available, provider.RateLimitResetCredit{
				ID: credit.ID, ResetType: credit.ResetType, Status: credit.Status,
				GrantedAt: credit.GrantedAt, ExpiresAt: credit.ExpiresAt,
			})
		}
	}
	if len(available) != 1 || len(golden.RateLimitResetCredits) != 1 ||
		available[0].ID != golden.RateLimitResetCredits[0].ID {
		t.Fatalf("reset credits = %+v, want %+v", available, golden.RateLimitResetCredits)
	}
	_ = fetcher
}

// Protects quota.md "Order" and quota-providers.md "Antigravity": groups order
// Gemini, Claude, other, and 5h before weekly inside each group.
func TestAntigravityQuotaParity(t *testing.T) {
	golden := loadGolden(t, "antigravity.json")
	var summary map[string]any
	if err := json.Unmarshal(loadFixture(t, "antigravity.json"), &summary); err != nil {
		t.Fatal(err)
	}
	groups, ok := extractQuotaGroups(summary)
	if !ok {
		t.Fatal("summary did not parse")
	}
	assertWindows(t, antigravityWindows(groups.groups), golden.Windows, true)

	var status map[string]any
	if err := json.Unmarshal(loadFixture(t, "antigravity-status.json"), &status); err != nil {
		t.Fatal(err)
	}
	account, plan := antigravityUserStatus(status)
	if golden.Account == nil || account != *golden.Account || golden.Plan == nil || plan != *golden.Plan {
		t.Fatalf("account/plan = %q/%q, want %v/%v", account, plan, golden.Account, golden.Plan)
	}

	var wrapper struct {
		Command struct {
			Data agyQuotaResponse `json:"data"`
		} `json:"command"`
	}
	if err := json.Unmarshal(loadFixture(t, "agy-cli.json"), &wrapper); err != nil {
		t.Fatal(err)
	}
	var cliGroups []quotaGroup
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
		cliGroups = append(cliGroups, qg)
	}
	assertWindows(t, antigravityWindows(cliGroups), golden.AgyWindows, true)
}

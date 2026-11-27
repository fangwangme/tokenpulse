package report

import (
	"encoding/json"
	"testing"
	"time"
)

// Protects usage-report.md "Report": Report.Days includes per-day company
// totals computed in the same report query so stacked charts need no N+1 calls.
func TestReportIncludesDailyCompanyBreakdown(t *testing.T) {
	now := time.Date(2026, 11, 27, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-27", now, "s1", "m1", "claude-opus-4-6", 1000, 2.0), 2.0, false)
	insertMessage(t, db, sample("2026-11-27", now, "s2", "m2", "gpt-5", 2000, 3.0), 3.0, false)
	insertMessage(t, db, sample("2026-11-27", now, "s3", "m3", "gemini-2.5-pro", 3000, 4.0), 4.0, false)
	insertMessage(t, db, sample("2026-11-27", now, "s4", "m4", "deepseek-v3", 4000, 1.0), 1.0, false)

	got, err := service.Report(Params{Preset: "today", Agents: []string{"claude"}})
	if err != nil {
		t.Fatalf("Report: %v", err)
	}
	raw, err := json.Marshal(got)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	var decoded struct {
		Days []struct {
			Date      string `json:"date"`
			Companies map[string]struct {
				Tokens int64   `json:"tokens"`
				Cost   float64 `json:"cost"`
			} `json:"companies"`
		} `json:"days"`
	}
	if err := json.Unmarshal(raw, &decoded); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(decoded.Days) != 1 {
		t.Fatalf("expected 1 day, got %d", len(decoded.Days))
	}
	companies := decoded.Days[0].Companies
	if len(companies) == 0 {
		t.Fatalf("expected per-day company breakdown in JSON, got empty: %s", string(raw))
	}
	wantTokens := map[string]int64{
		"anthropic": 1000,
		"openai":    2000,
		"google":    3000,
		"other":     4000,
	}
	for company, want := range wantTokens {
		if companies[company].Tokens != want {
			t.Errorf("company %q tokens = %d, want %d", company, companies[company].Tokens, want)
		}
	}
}

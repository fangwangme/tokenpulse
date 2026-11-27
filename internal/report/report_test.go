package report

import (
	"database/sql"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

func newReport(t *testing.T, now time.Time) (*Service, *sql.DB, string) {
	t.Helper()
	path := t.TempDir() + "/usage.db"
	store := usage.NewStore(path)
	db, err := store.Open()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	service := NewService(path)
	service.Now = func() time.Time { return now }
	return service, db, path
}

func insertMessage(t *testing.T, db *sql.DB, msg provider.UnifiedMessage, cost float64, unpriced bool) {
	t.Helper()
	client := msg.ClientDetail
	if client == "" {
		client = msg.Client
	}
	flag := 0
	if unpriced {
		flag = 1
	}
	_, err := db.Exec(`INSERT INTO usage_messages (source, client, provider_id, model_id, canonical_model_id,
        session_id, message_key, timestamp_ms, date, input_tokens, output_tokens, cache_read_tokens,
        cache_write_tokens, reasoning_tokens, total_tokens, cost_usd, pricing_day, parser_version,
        served_model, unpriced) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		msg.Client, client, msg.ProviderID, msg.ModelID, msg.ModelID, msg.SessionID, msg.MessageKey,
		msg.Timestamp, msg.Date, msg.Tokens.Input, msg.Tokens.Output, msg.Tokens.CacheRead,
		msg.Tokens.CacheWrite, msg.Tokens.Reasoning, msg.TotalTokens(), cost, msg.PricingDay,
		msg.ParserVersion, nil, flag)
	if err != nil {
		t.Fatal(err)
	}
}

func sample(day string, at time.Time, session, key, model string, tokens, cost float64) provider.UnifiedMessage {
	stamp := time.Date(at.Year(), at.Month(), at.Day(), at.Hour(), at.Minute(), 0, 0, time.Local)
	message := provider.NewMessage("claude", model, "anthropic", session, key, stamp.UnixMilli(),
		provider.TokenBreakdown{Input: int64(tokens * 0.6), Output: int64(tokens * 0.4)})
	message.Date = day
	message.PricingDay = day
	message.ParserVersion = "claude-v4"
	_ = cost
	return message
}

// Protects usage-report.md "Presets and ranges".
func TestPresetsResolveFromToday(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-01-02", time.Date(2026, 1, 2, 9, 0, 0, 0, time.Local), "s1", "m1", "claude-opus-4-6", 100, 1), 1, false)
	insertMessage(t, db, sample("2026-11-25", now, "s2", "m2", "claude-sonnet-4-5", 50, 1), 1, false)

	cases := []struct {
		preset   string
		from, to string
	}{
		{"today", "2026-11-25", "2026-11-25"},
		{"7d", "2026-11-19", "2026-11-25"},
		{"30d", "2026-10-27", "2026-11-25"},
		{"year", "2026-01-01", "2026-11-25"},
		{"all", "2026-01-02", "2026-11-25"},
	}
	for _, tc := range cases {
		result, err := service.Report(Params{Preset: tc.preset, Agents: []string{"claude"}})
		if err != nil {
			t.Fatalf("%s: %v", tc.preset, err)
		}
		if result.Range.From != tc.from || result.Range.To != tc.to {
			t.Errorf("%s range = %s..%s, want %s..%s", tc.preset, result.Range.From, result.Range.To, tc.from, tc.to)
		}
	}
}

// Protects usage-report.md "Presets and ranges": custom limits.
func TestCustomRangeRejectsOutsideFirstDayAndToday(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-03-01", time.Date(2026, 3, 1, 9, 0, 0, 0, time.Local), "s1", "m1", "claude-opus-4-6", 100, 1), 1, false)

	if _, err := service.Report(Params{Preset: "custom", From: "2026-02-28", To: "2026-03-02", Agents: []string{"claude"}}); err == nil ||
		!strings.Contains(err.Error(), "2026-03-01") || !strings.Contains(err.Error(), "2026-11-25") {
		t.Fatalf("expected both limits in the error, got %v", err)
	}
	if _, err := service.Report(Params{Preset: "custom", From: "2026-11-26", To: "2026-11-27", Agents: []string{"claude"}}); err == nil {
		t.Fatal("expected a future endpoint to be rejected")
	}
	result, err := service.Report(Params{Preset: "custom", From: "2026-11-20", To: "2026-11-10", Agents: []string{"claude"}})
	if err != nil || result.Range.From != "2026-11-10" || result.Range.To != "2026-11-20" {
		t.Fatalf("reverse custom range must swap: %+v %v", result.Range, err)
	}
}

// Protects usage-report.md "Report": empty agents is a request error.
func TestEmptyAgentsRejected(t *testing.T) {
	service, _, _ := newReport(t, time.Now())
	if _, err := service.Report(Params{Preset: "today"}); err == nil {
		t.Fatal("expected an error for no agents")
	}
}

// Protects usage-report.md "Comparison".
func TestComparisonForTodayStopsAtTheCurrentMinute(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	previous := time.Date(2026, 11, 18, 0, 0, 0, 0, time.Local)
	insertMessage(t, db, sample("2026-11-18", previous.Add(10*time.Hour), "s1", "p-morning", "claude-opus-4-6", 100, 1), 1, false)
	insertMessage(t, db, sample("2026-11-18", previous.Add(13*time.Hour), "s1", "p-afternoon", "claude-opus-4-6", 200, 2), 2, false)
	insertMessage(t, db, sample("2026-11-25", now.Add(-2*time.Hour), "s2", "t-morning", "claude-opus-4-6", 50, 1), 1, false)
	insertMessage(t, db, sample("2026-11-25", now.Add(2*time.Hour), "s2", "t-afternoon", "claude-opus-4-6", 50, 1), 1, false)

	result, err := service.Report(Params{Preset: "today", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	if result.Comparison == nil || result.Comparison.UntilMinutes == nil || *result.Comparison.UntilMinutes != 12*60 {
		t.Fatalf("expected a comparison with untilMinutes=720, got %+v", result.Comparison)
	}
	if result.Comparison.Totals.Tokens != 100 {
		t.Fatalf("comparison counted a future message: %d tokens", result.Comparison.Totals.Tokens)
	}
}

// Protects usage-report.md "Comparison": a period compares the N days before.
func TestPeriodComparisonUsesPreviousWindow(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-10", time.Date(2026, 11, 10, 9, 0, 0, 0, time.Local), "s1", "a", "claude-opus-4-6", 100, 1), 1, false)
	insertMessage(t, db, sample("2026-11-15", time.Date(2026, 11, 15, 9, 0, 0, 0, time.Local), "s2", "b", "claude-opus-4-6", 300, 3), 3, false)
	insertMessage(t, db, sample("2026-11-20", time.Date(2026, 11, 20, 9, 0, 0, 0, time.Local), "s3", "c", "claude-opus-4-6", 50, 1), 1, false)
	result, err := service.Report(Params{Preset: "7d", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	if result.Comparison == nil {
		t.Fatal("expected a period comparison")
	}
	if result.Comparison.From != "2026-11-12" || result.Comparison.To != "2026-11-18" {
		t.Fatalf("comparison range = %s..%s", result.Comparison.From, result.Comparison.To)
	}
	if result.Comparison.Totals.Tokens != 300 {
		t.Fatalf("comparison tokens = %d", result.Comparison.Totals.Tokens)
	}
}

// Protects usage-report.md "Heatmap layout".
func TestHeatmapLayoutFollowsRangeLength(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-01-02", time.Date(2026, 1, 2, 9, 0, 0, 0, time.Local), "s", "m", "claude-opus-4-6", 10, 1), 1, false)
	cases := []struct{ from, to, layout string }{
		{"2026-11-19", "2026-11-25", "strip"},
		{"2026-11-10", "2026-11-25", "week-rows"},
		{"2026-09-01", "2026-11-25", "week-columns"},
	}
	for _, tc := range cases {
		result, err := service.Report(Params{Preset: "custom", From: tc.from, To: tc.to, Agents: []string{"claude"}})
		if err != nil {
			t.Fatal(err)
		}
		if result.Layout != tc.layout {
			t.Errorf("%s..%s layout = %s, want %s", tc.from, tc.to, result.Layout, tc.layout)
		}
	}
}

// Protects usage-report.md "Numbers": range sessions are not the sum of days.
func TestSessionsAreDeduplicatedAcrossDays(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-24", time.Date(2026, 11, 24, 9, 0, 0, 0, time.Local), "same-session", "a", "claude-opus-4-6", 10, 1), 1, false)
	insertMessage(t, db, sample("2026-11-25", time.Date(2026, 11, 25, 9, 0, 0, 0, time.Local), "same-session", "b", "claude-opus-4-6", 10, 1), 1, false)
	result, err := service.Report(Params{Preset: "7d", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	if result.Totals.Sessions != 1 {
		t.Fatalf("range sessions = %d, want 1", result.Totals.Sessions)
	}
	if len(result.Days) != 2 {
		t.Fatalf("days = %d", len(result.Days))
	}
}

// Protects usage-report.md "Model detail": perDay zero fill.
func TestModelDetailZeroFillsEveryDay(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-20", time.Date(2026, 11, 20, 9, 0, 0, 0, time.Local), "s", "a", "claude-opus-4-6", 10, 1), 1, false)
	detail, err := service.Model("claude-opus-4-6", "2026-11-19", "2026-11-22", []string{"claude"})
	if err != nil {
		t.Fatal(err)
	}
	if len(detail.PerDay) != 4 {
		t.Fatalf("perDay entries = %d, want 4", len(detail.PerDay))
	}
	if detail.PerDay[0].Date != "2026-11-19" || detail.PerDay[0].Tokens != 0 || detail.PerDay[1].Tokens != 10 {
		t.Fatalf("perDay = %+v", detail.PerDay)
	}
}

// Protects usage-report.md "JSON" field names and omitted untilMinutes.
func TestReportJSONFieldNames(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-25", now, "s", "a", "claude-opus-4-6", 10, 1), 1, false)
	result, err := service.Report(Params{Preset: "7d", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	data, err := json.Marshal(result)
	if err != nil {
		t.Fatal(err)
	}
	text := string(data)
	for _, field := range []string{`"range"`, `"firstDay"`, `"scannedAt"`, `"pricesAvailable"`, `"totals"`, `"comparison"`, `"days"`, `"models"`, `"cacheRead"`, `"lastUsed"`} {
		if !strings.Contains(text, field) {
			t.Errorf("missing %s in %s", field, text)
		}
	}
	if strings.Contains(text, "untilMinutes") {
		t.Error("untilMinutes must be omitted when absent")
	}
}

// Protects usage-report.md "Caching": keyed by scannedAt, cleared explicitly.
func TestReportCacheKeyAndClear(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-25", now, "s", "a", "claude-opus-4-6", 10, 1), 1, false)
	first, err := service.Report(Params{Preset: "today", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	insertMessage(t, db, sample("2026-11-25", now, "s", "b", "claude-opus-4-6", 90, 9), 9, false)
	cached, err := service.Report(Params{Preset: "today", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	if cached.Totals.Tokens != first.Totals.Tokens {
		t.Fatal("report was not served from the cache")
	}
	service.Clear()
	fresh, err := service.Report(Params{Preset: "today", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	if fresh.Totals.Tokens == first.Totals.Tokens {
		t.Fatal("Clear did not drop the cached report")
	}
}

// Protects usage-report.md "Presets and ranges": all with no usage starts today.
func TestAllWithoutUsageStartsToday(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, _, _ := newReport(t, now)
	result, err := service.Report(Params{Preset: "all", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	if result.Range.From != "2026-11-25" || result.Range.To != "2026-11-25" || result.FirstDay != nil {
		t.Fatalf("range = %+v, firstDay = %v", result.Range, result.FirstDay)
	}
}

// Protects usage-report.md "Presets and ranges": a missing custom endpoint is
// today and reverse endpoints swap.
func TestCustomMissingEndpointDefaultsToToday(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-20", time.Date(2026, 11, 20, 9, 0, 0, 0, time.Local), "s", "a", "claude-opus-4-6", 10, 1), 1, false)
	result, err := service.Report(Params{Preset: "custom", From: "2026-11-20", Agents: []string{"claude"}})
	if err != nil || result.Range.From != "2026-11-20" || result.Range.To != "2026-11-25" {
		t.Fatalf("from-only range = %+v, %v", result.Range, err)
	}
	result, err = service.Report(Params{Preset: "custom", To: "2026-11-22", Agents: []string{"claude"}})
	if err != nil || result.Range.From != "2026-11-22" || result.Range.To != "2026-11-25" {
		t.Fatalf("to-only range = %+v, %v", result.Range, err)
	}
}

// Protects usage-report.md "Comparison": year/all carry none, a non-today
// single day compares the whole previous week, and no comparison is made when
// it would start before the first day with usage.
func TestComparisonPresenceRules(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-13", time.Date(2026, 11, 13, 9, 0, 0, 0, time.Local), "s", "first", "claude-opus-4-6", 10, 1), 1, false)
	insertMessage(t, db, sample("2026-11-18", time.Date(2026, 11, 18, 9, 0, 0, 0, time.Local), "s", "a", "claude-opus-4-6", 10, 1), 1, false)
	insertMessage(t, db, sample("2026-11-20", time.Date(2026, 11, 20, 9, 0, 0, 0, time.Local), "s", "b", "claude-opus-4-6", 10, 1), 1, false)
	for _, preset := range []string{"year", "all"} {
		result, err := service.Report(Params{Preset: preset, Agents: []string{"claude"}})
		if err != nil {
			t.Fatal(err)
		}
		if result.Comparison != nil {
			t.Fatalf("%s comparison = %+v", preset, result.Comparison)
		}
	}
	result, err := service.Report(Params{Preset: "custom", From: "2026-11-20", To: "2026-11-20", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	if result.Comparison == nil || result.Comparison.UntilMinutes != nil ||
		result.Comparison.From != "2026-11-13" || result.Comparison.To != "2026-11-13" {
		t.Fatalf("single-day comparison = %+v", result.Comparison)
	}
	// The earliest day with usage is 2026-11-13; a range starting then would
	// compare against 2026-11-06..2026-11-12, before the first day.
	result, err = service.Report(Params{Preset: "custom", From: "2026-11-13", To: "2026-11-14", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	if result.Comparison != nil {
		t.Fatalf("comparison before first day = %+v", result.Comparison)
	}
}

// Protects usage-report.md "Report": days are newest first and models by
// tokens descending; a model stays unpriced when any message is unpriced.
func TestReportOrderingAndUnpriced(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-20", time.Date(2026, 11, 20, 9, 0, 0, 0, time.Local), "s", "small", "claude-haiku-4-5", 10, 1), 1, false)
	insertMessage(t, db, sample("2026-11-21", time.Date(2026, 11, 21, 9, 0, 0, 0, time.Local), "s", "big", "claude-opus-4-6", 100, 1), 1, true)
	result, err := service.Report(Params{Preset: "7d", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Days) != 2 || result.Days[0].Date != "2026-11-21" || result.Days[1].Date != "2026-11-20" {
		t.Fatalf("days = %+v", result.Days)
	}
	if len(result.Models) != 2 || result.Models[0].Model != "claude-opus-4-6" || !result.Models[0].Unpriced {
		t.Fatalf("models = %+v", result.Models)
	}
	if result.Models[0].Company != "anthropic" || result.Models[0].LastUsed != "2026-11-21" {
		t.Fatalf("model fields = %+v", result.Models[0])
	}
}

// Protects usage-report.md "Day detail": agents and models descend by tokens.
func TestDayDetailOrdering(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	claudeSmall := sample("2026-11-24", time.Date(2026, 11, 24, 9, 0, 0, 0, time.Local), "s1", "a", "claude-haiku-4-5", 10, 1)
	claudeBig := sample("2026-11-24", time.Date(2026, 11, 24, 9, 1, 0, 0, time.Local), "s1", "b", "claude-opus-4-6", 100, 1)
	codex := provider.NewMessage("codex", "gpt-5", "openai", "s2", "c", time.Date(2026, 11, 24, 9, 2, 0, 0, time.Local).UnixMilli(),
		provider.TokenBreakdown{Input: 50})
	codex.Date = "2026-11-24"
	codex.PricingDay = "2026-11-24"
	codex.ParserVersion = "codex-v4"
	insertMessage(t, db, claudeSmall, 1, false)
	insertMessage(t, db, claudeBig, 1, false)
	insertMessage(t, db, codex, 1, false)
	detail, err := service.Day("2026-11-24", []string{"claude", "codex"})
	if err != nil {
		t.Fatal(err)
	}
	if len(detail.Agents) != 2 || detail.Agents[0].Agent != "claude" || detail.Agents[0].Models[0].Model != "claude-opus-4-6" {
		t.Fatalf("day = %+v", detail)
	}
	if detail.Agents[1].Agent != "codex" || detail.Agents[1].Totals.Tokens != 50 {
		t.Fatalf("day codex = %+v", detail.Agents[1])
	}
}

// Protects usage-report.md "Model detail": agents carry totals, company is set,
// and lastUsed is the latest date in the range.
func TestModelDetailFields(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-20", time.Date(2026, 11, 20, 9, 0, 0, 0, time.Local), "s1", "a", "claude-opus-4-6", 10, 1), 1, false)
	insertMessage(t, db, sample("2026-11-22", time.Date(2026, 11, 22, 9, 0, 0, 0, time.Local), "s2", "b", "claude-opus-4-6", 10, 1), 1, false)
	detail, err := service.Model("claude-opus-4-6", "2026-11-20", "2026-11-22", []string{"claude"})
	if err != nil {
		t.Fatal(err)
	}
	if detail.Company != "anthropic" || detail.LastUsed != "2026-11-22" ||
		len(detail.Agents) != 1 || detail.Agents[0].Agent != "claude" || detail.Agents[0].Tokens != 20 {
		t.Fatalf("detail = %+v", detail)
	}
}

// Protects usage-report.md "Model detail": a reversed custom range is padded
// and queried with the normalized dates, not just in the totals.
func TestModelDetailReversedRangeUsesNormalizedDates(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-20", time.Date(2026, 11, 20, 9, 0, 0, 0, time.Local), "s1", "a", "claude-opus-4-6", 10, 1), 1, false)
	insertMessage(t, db, sample("2026-11-21", time.Date(2026, 11, 21, 9, 0, 0, 0, time.Local), "s2", "b", "claude-opus-4-6", 10, 1), 1, false)
	detail, err := service.Model("claude-opus-4-6", "2026-11-22", "2026-11-19", []string{"claude"})
	if err != nil {
		t.Fatal(err)
	}
	if detail.Totals.Tokens != 20 || detail.LastUsed != "2026-11-21" {
		t.Fatalf("totals=%+v lastUsed=%q", detail.Totals, detail.LastUsed)
	}
	if len(detail.Agents) != 1 || detail.Agents[0].Tokens != 20 {
		t.Fatalf("agents = %+v", detail.Agents)
	}
	if len(detail.PerDay) != 4 || detail.PerDay[1].Tokens != 10 || detail.PerDay[2].Tokens != 10 {
		t.Fatalf("perDay = %+v", detail.PerDay)
	}
}

// Protects usage-report.md "Day detail": agent sessions are distinct across
// that agent's models, not the sum of each model's distinct sessions.
func TestDayDetailSessionCountSpansModels(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service, db, _ := newReport(t, now)
	insertMessage(t, db, sample("2026-11-20", time.Date(2026, 11, 20, 9, 0, 0, 0, time.Local), "shared", "a", "claude-opus-4-6", 10, 1), 1, false)
	insertMessage(t, db, sample("2026-11-20", time.Date(2026, 11, 20, 10, 0, 0, 0, time.Local), "shared", "b", "claude-sonnet-4-5", 10, 1), 1, false)
	detail, err := service.Day("2026-11-20", []string{"claude"})
	if err != nil {
		t.Fatal(err)
	}
	if len(detail.Agents) != 1 || detail.Agents[0].Totals.Sessions != 1 {
		t.Fatalf("agent sessions = %+v", detail.Agents)
	}
	if len(detail.Agents[0].Models) != 2 || detail.Agents[0].Models[0].Totals.Sessions != 1 {
		t.Fatalf("model sessions = %+v", detail.Agents[0].Models)
	}
}

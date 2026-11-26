package tui

import (
	"database/sql"
	"errors"
	"fmt"
	"reflect"
	"regexp"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/report"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

// recordingClient answers usage calls from a real report.Service over a
// synthetic ledger and records every usage request the TUI makes.
type recordingClient struct {
	service *report.Service
	cfg     config.Config

	mu         sync.Mutex
	usageCalls []report.Params
	modelCalls []modelCall
}

type modelCall struct{ model, from, to string }

func (c *recordingClient) Status() (rt.StatusSnapshot, error) {
	return rt.StatusSnapshot{Running: true}, nil
}
func (c *recordingClient) Config() (config.Config, error) { return c.cfg, nil }
func (c *recordingClient) Usage(params report.Params) (report.Report, error) {
	c.mu.Lock()
	c.usageCalls = append(c.usageCalls, params)
	c.mu.Unlock()
	return c.service.Report(params)
}
func (c *recordingClient) UsageDay(date string, agents []string) (report.DayDetail, error) {
	return c.service.Day(date, agents)
}
func (c *recordingClient) UsageModel(model, from, to string, agents []string) (report.ModelDetail, error) {
	c.mu.Lock()
	c.modelCalls = append(c.modelCalls, modelCall{model, from, to})
	c.mu.Unlock()
	return c.service.Model(model, from, to, agents)
}
func (c *recordingClient) Quota() (rt.QuotaView, error) { return rt.QuotaView{}, nil }
func (c *recordingClient) KeeperRecords(int) ([]keeper.ExecutionRecord, error) {
	return nil, nil
}
func (c *recordingClient) Refresh(bool, bool) (rt.RefreshResult, error) {
	return rt.RefreshResult{Accepted: true}, nil
}
func (c *recordingClient) PatchSettings(map[string]any) (config.Config, error) {
	return c.cfg, errors.New("not supported in this test")
}
func (c *recordingClient) KeeperPing(string, string) error { return errors.New("no pings in tests") }
func (c *recordingClient) Subscribe() (<-chan rt.Event, func(), error) {
	return make(chan rt.Event), func() {}, nil
}
func (c *recordingClient) IsRemote() bool { return false }
func (c *recordingClient) Close() error   { return nil }

func (c *recordingClient) usage() []report.Params {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]report.Params(nil), c.usageCalls...)
}

func (c *recordingClient) models() []modelCall {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]modelCall(nil), c.modelCalls...)
}

type ledgerRow struct {
	date   time.Time
	model  string
	tokens int64
	cost   float64
}

func newLedger(t *testing.T, rows []ledgerRow) *report.Service {
	t.Helper()
	path := t.TempDir() + "/usage.db"
	db, err := usage.NewStore(path).Open()
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	for i, row := range rows {
		insertLedgerRow(t, db, i, row)
	}
	return report.NewService(path)
}

func insertLedgerRow(t *testing.T, db *sql.DB, index int, row ledgerRow) {
	t.Helper()
	day := row.date.Format("2006-01-02")
	stamp := time.Date(row.date.Year(), row.date.Month(), row.date.Day(), 12, 0, 0, 0, time.Local)
	_, err := db.Exec(`INSERT INTO usage_messages (source, client, provider_id, model_id, canonical_model_id,
        session_id, message_key, timestamp_ms, date, input_tokens, output_tokens, cache_read_tokens,
        cache_write_tokens, reasoning_tokens, total_tokens, cost_usd, pricing_day, parser_version,
        served_model, unpriced) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		"claude", "claude", "anthropic", row.model, row.model, fmt.Sprintf("s%d", index),
		fmt.Sprintf("m%d", index), stamp.UnixMilli(), day, row.tokens, 0, 0, 0, 0, row.tokens,
		row.cost, day, "claude-v4", nil, 0)
	if err != nil {
		t.Fatal(err)
	}
}

type harness struct {
	t      *testing.T
	m      tea.Model
	client *recordingClient
}

func startTUI(t *testing.T, service *report.Service) *harness {
	t.Helper()
	cfg := config.Default()
	client := &recordingClient{service: service, cfg: cfg}
	h := &harness{t: t, m: newModel(Options{Config: cfg, NoScan: true}, client), client: client}
	h.run(h.m.Init())
	h.send(tea.WindowSizeMsg{Width: 180, Height: 60})
	return h
}

// send delivers one message and drains the commands it produces. Commands
// that do not answer quickly (event subscriptions, timers) are dropped, and
// the drain stops after a bounded number of messages so repeating
// animation ticks cannot loop forever.
func (h *harness) send(msg tea.Msg) {
	h.t.Helper()
	next, cmd := h.m.Update(msg)
	h.m = next
	budget := 200
	h.drain(cmd, &budget)
}

func (h *harness) run(cmd tea.Cmd) {
	budget := 200
	h.drain(cmd, &budget)
}

var cmdType = reflect.TypeOf((tea.Cmd)(nil))

func (h *harness) drain(cmd tea.Cmd, budget *int) {
	if cmd == nil || *budget <= 0 {
		return
	}
	*budget--
	done := make(chan tea.Msg, 1)
	go func() { done <- cmd() }()
	var msg tea.Msg
	select {
	case msg = <-done:
	case <-time.After(300 * time.Millisecond):
		return
	}
	if msg == nil {
		return
	}
	if value := reflect.ValueOf(msg); value.Kind() == reflect.Slice && value.Type().Elem() == cmdType {
		for i := 0; i < value.Len(); i++ {
			if inner, ok := value.Index(i).Interface().(tea.Cmd); ok {
				h.drain(inner, budget)
			}
		}
		return
	}
	next, follow := h.m.Update(msg)
	h.m = next
	h.drain(follow, budget)
}

func (h *harness) key(keys ...string) {
	h.t.Helper()
	for _, key := range keys {
		switch key {
		case "tab":
			h.send(tea.KeyMsg{Type: tea.KeyTab})
		case "shift+tab":
			h.send(tea.KeyMsg{Type: tea.KeyShiftTab})
		default:
			h.send(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(key)})
		}
	}
}

var ansi = regexp.MustCompile(`\x1b\[[0-9;?]*[ -/]*[@-~]`)

func (h *harness) screen() []string {
	return strings.Split(ansi.ReplaceAllString(h.m.View(), ""), "\n")
}

// hasLine reports whether one screen line contains label and value, with
// value standing as its own token.
func hasLine(lines []string, label, value string) bool {
	token := regexp.MustCompile(`(^|\s)` + regexp.QuoteMeta(value) + `(\s|$)`)
	for _, line := range lines {
		if strings.Contains(line, label) && token.MatchString(line) {
			return true
		}
	}
	return false
}

func today() time.Time {
	now := time.Now()
	return time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.Local)
}

func ymd(day time.Time) string { return day.Format("2006-01-02") }

func compactTokens(value int64) string {
	switch {
	case value >= 1_000_000_000:
		return fmt.Sprintf("%.2f B", float64(value)/1_000_000_000)
	case value >= 1_000_000:
		return fmt.Sprintf("%.2f M", float64(value)/1_000_000)
	case value >= 1_000:
		return fmt.Sprintf("%.2f K", float64(value)/1_000)
	default:
		return fmt.Sprintf("%.2f", float64(value))
	}
}

// Protects tui-usage.md "Range and filter" and "Activity": the range keys
// move the shared range only on Overview, Models and Daily, and Activity loads
// its own fixed window.
func TestRangeKeysDriveRangePagesOnly(t *testing.T) {
	now := today()
	first := now.AddDate(0, 0, -500)
	h := startTUI(t, newLedger(t, []ledgerRow{
		{date: first, model: "claude-sonnet-4-5", tokens: 10},
		{date: now.AddDate(0, 0, -3), model: "claude-sonnet-4-5", tokens: 20},
		{date: now, model: "claude-sonnet-4-5", tokens: 30},
	}))

	calls := h.client.usage()
	if len(calls) == 0 || calls[0].Preset != "30d" {
		t.Fatalf("first usage request = %+v, want preset 30d", calls)
	}

	h.key("2")
	if last := lastRangeCall(h.client.usage()); last.Preset != "7d" {
		t.Fatalf("after `2` on Overview the range request = %+v, want preset 7d", last)
	}

	h.key("tab", "tab", "tab")
	windowStart := now.AddDate(0, 0, -364)
	if jan1 := time.Date(now.Year(), 1, 1, 0, 0, 0, 0, time.Local); jan1.Before(windowStart) {
		windowStart = jan1
	}
	var activity *report.Params
	for _, call := range h.client.usage() {
		if call.Preset == "custom" && call.From == ymd(windowStart) && call.To == ymd(now) {
			call := call
			activity = &call
		}
	}
	if activity == nil {
		t.Fatalf("Activity did not request %s..%s as a custom range; requests = %+v",
			ymd(windowStart), ymd(now), h.client.usage())
	}

	before := len(h.client.usage())
	h.key("3")
	for _, call := range h.client.usage()[before:] {
		if call.Preset == "30d" {
			t.Fatalf("`3` on Activity changed the shared range: %+v", call)
		}
	}

	h.key("shift+tab", "shift+tab", "shift+tab", "1")
	if last := lastRangeCall(h.client.usage()); last.Preset != "today" {
		t.Fatalf("after `1` back on Overview the range request = %+v, want preset today", last)
	}
}

func lastRangeCall(calls []report.Params) report.Params {
	for i := len(calls) - 1; i >= 0; i-- {
		if calls[i].Preset != "custom" {
			return calls[i]
		}
	}
	return report.Params{}
}

// Protects tui-usage.md "Daily": vs-7-days compares with the date seven days
// earlier even before the range start, and the extra request is clamped to
// the first data day.
func TestDailyComparesSevenDaysEarlier(t *testing.T) {
	now := today()
	first := now.AddDate(0, 0, -9)
	h := startTUI(t, newLedger(t, []ledgerRow{
		{date: first, model: "claude-sonnet-4-5", tokens: 100},
		{date: now.AddDate(0, 0, -7), model: "claude-sonnet-4-5", tokens: 400},
		{date: now.AddDate(0, 0, -2), model: "claude-sonnet-4-5", tokens: 200},
		{date: now.AddDate(0, 0, -1), model: "claude-sonnet-4-5", tokens: 50},
		{date: now, model: "claude-sonnet-4-5", tokens: 100},
	}))

	h.key("2", "tab", "tab")

	clamped := false
	for _, call := range h.client.usage() {
		if call.Preset == "custom" && call.From == ymd(first) {
			clamped = true
		}
	}
	if !clamped {
		t.Fatalf("Daily did not request the earlier dates from the first data day %s; requests = %+v",
			ymd(first), h.client.usage())
	}

	lines := h.screen()
	cases := []struct {
		date time.Time
		want string
	}{
		{now.AddDate(0, 0, -2), "+100.0%↑"},
		{now, "-75.0%↓"},
		{now.AddDate(0, 0, -1), "—"},
	}
	for _, tc := range cases {
		if !hasLine(lines, ymd(tc.date), tc.want) {
			t.Errorf("Daily row %s does not show %q:\n%s", ymd(tc.date), tc.want, strings.Join(lines, "\n"))
		}
	}
}

// Protects tui-usage.md "Overview": company segments come from the model
// detail of Anthropic, OpenAI and Google models over the current range only.
func TestOverviewStacksByCompany(t *testing.T) {
	now := today()
	service := newLedger(t, []ledgerRow{
		{date: now.AddDate(0, 0, -2), model: "claude-sonnet-4-5", tokens: 300, cost: 3},
		{date: now.AddDate(0, 0, -1), model: "gpt-5", tokens: 200, cost: 2},
		{date: now.AddDate(0, 0, -1), model: "gemini-2.5-pro", tokens: 150, cost: 1.5},
		{date: now, model: "deepseek-v3", tokens: 100, cost: 1},
	})
	h := startTUI(t, service)

	expected, err := service.Report(report.Params{Preset: "30d", Agents: []string{"claude"}})
	if err != nil {
		t.Fatal(err)
	}
	var want []string
	for _, model := range expected.Models {
		if model.Company == "anthropic" || model.Company == "openai" || model.Company == "google" {
			want = append(want, model.Model)
		}
	}
	sort.Strings(want)

	requested := map[string]bool{}
	for _, call := range h.client.models() {
		if call.from != expected.Range.From || call.to != expected.Range.To {
			t.Errorf("model detail %s requested over %s..%s, want the range %s..%s",
				call.model, call.from, call.to, expected.Range.From, expected.Range.To)
		}
		requested[call.model] = true
	}
	var got []string
	for model := range requested {
		got = append(got, model)
	}
	sort.Strings(got)
	if strings.Join(got, ",") != strings.Join(want, ",") {
		t.Fatalf("model details requested for %v, want exactly %v", got, want)
	}

	screen := strings.Join(h.screen(), "\n")
	for _, label := range []string{"● Anthropic", "● OpenAI", "● Google", "● Others"} {
		if !strings.Contains(screen, label) {
			t.Errorf("Overview legend lacks %q:\n%s", label, screen)
		}
	}
}

// Protects tui-usage.md "Activity": the period overview rows sum the days
// each window names.
func TestActivityPeriodRows(t *testing.T) {
	now := today()
	tokens := map[string]int64{}
	add := func(day time.Time, value int64) { tokens[ymd(day)] += value }
	add(now, 3)
	add(now.AddDate(0, 0, -1), 50)
	add(now.AddDate(0, 0, -6), 700)
	add(now.AddDate(0, 0, -7), 9_000)
	add(now.AddDate(0, 0, -29), 20_000)
	add(now.AddDate(0, 0, -30), 400_000)
	add(time.Date(now.Year(), 1, 1, 0, 0, 0, 0, time.Local), 6_000_000)

	var rows []ledgerRow
	for date, value := range tokens {
		day, err := time.ParseInLocation("2006-01-02", date, time.Local)
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, ledgerRow{date: day, model: "claude-sonnet-4-5", tokens: value})
	}
	h := startTUI(t, newLedger(t, rows))
	h.key("tab", "tab", "tab")

	monday := now.AddDate(0, 0, -((int(now.Weekday()) + 6) % 7))
	sum := func(include func(day time.Time) bool) int64 {
		var total int64
		for date, value := range tokens {
			day, _ := time.ParseInLocation("2006-01-02", date, time.Local)
			if !day.After(now) && include(day) {
				total += value
			}
		}
		return total
	}
	want := map[string]int64{
		"Today": sum(func(d time.Time) bool { return d.Equal(now) }),
		"Week":  sum(func(d time.Time) bool { return !d.Before(monday) }),
		"Month": sum(func(d time.Time) bool { return d.Year() == now.Year() && d.Month() == now.Month() }),
		"Year":  sum(func(d time.Time) bool { return d.Year() == now.Year() }),
		"Last 7 days": sum(func(d time.Time) bool {
			return !d.Before(now.AddDate(0, 0, -6))
		}),
		"Last 30 days": sum(func(d time.Time) bool {
			return !d.Before(now.AddDate(0, 0, -29))
		}),
	}

	lines := h.screen()
	for label, value := range want {
		if !hasLine(lines, label, compactTokens(value)) {
			t.Errorf("Activity row %q does not show %s:\n%s", label, compactTokens(value), strings.Join(lines, "\n"))
		}
	}
}

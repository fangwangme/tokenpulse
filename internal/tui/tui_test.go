package tui

import (
	"errors"
	"path/filepath"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/remote"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
	"github.com/fangwangme/tokenpulse/internal/scan"
)

// Protects tui-usage.md "Quota": out-of-range provider percentages cannot
// produce a negative or oversized bar.
func TestGaugeFillClamps(t *testing.T) {
	cases := map[float64]int{-50: 0, 0: 0, 50: 10, 100: 20, 150: 20}
	for percent, want := range cases {
		if got := gaugeFill(percent, 20); got != want {
			t.Errorf("gaugeFill(%v, 20) = %d, want %d", percent, got, want)
		}
	}
}

// Protects tui-usage.md "Model detail": the sparkline writes whole runes and
// maps the largest day to the tallest level.
func TestSparklineUsesWholeRunes(t *testing.T) {
	for _, test := range []struct {
		values []int64
		want   string
	}{{[]int64{0}, "▁"}, {[]int64{10}, "█"}, {[]int64{5, 10}, "▅█"}} {
		got := sparklineText(test.values)
		if got != test.want {
			t.Errorf("sparklineText(%v) = %q, want %q", test.values, got, test.want)
		}
		if utf8.RuneCountInString(got) != len(test.values) {
			t.Errorf("sparklineText(%v) has %d runes, want %d", test.values, utf8.RuneCountInString(got), len(test.values))
		}
	}
}

func TestTruncateHandlesNonPositiveWidth(t *testing.T) {
	if got := truncate("hello", -1); got != "" {
		t.Fatalf("truncate(-1) = %q, want empty", got)
	}
	if got := truncate("hello", 0); got != "" {
		t.Fatalf("truncate(0) = %q, want empty", got)
	}
	if got := truncate("hello", 1); got != "h" {
		t.Fatalf("truncate(1) = %q, want h", got)
	}
}

func TestRenderQuotaIncludesPaceAndReset(t *testing.T) {
	cfg := config.Default()
	resets := time.Now().Add(2*time.Hour + 15*time.Minute)
	periodMs := int64(5 * 3600 * 1000)
	m := &model{
		opts: Options{Config: cfg},
		snapshots: []provider.QuotaSnapshot{{
			Provider: "claude",
			Windows: []provider.RateWindow{{
				ID:          "session",
				Label:       "Session (5h)",
				UsedPercent: 80,
				PeriodMs:    &periodMs,
				ResetsAt:    &resets,
			}},
		}},
	}
	out := m.renderQuota(100)
	if !strings.Contains(out, "pace") || !strings.Contains(out, "reset") {
		t.Fatalf("renderQuota missing pace or reset:\n%s", out)
	}
}

func TestApplyQuotaViewHidesDisabledProviders(t *testing.T) {
	m := &model{}
	m.applyQuotaView(rt.QuotaView{Providers: []rt.QuotaProviderView{
		{Provider: "claude", Enabled: true, Snapshot: &provider.QuotaSnapshot{Provider: "claude"}},
		{Provider: "opencode", Enabled: false, Snapshot: &provider.QuotaSnapshot{Provider: "opencode"}},
	}})
	if len(m.snapshots) != 1 || m.snapshots[0].Provider != "claude" {
		t.Fatalf("snapshots = %+v", m.snapshots)
	}
}

func TestApplyQuotaViewShowsEmptyProvidersWhenEnabled(t *testing.T) {
	cfg := config.Default()
	cfg.Display.ShowEmptyProviders = true
	m := &model{opts: Options{Config: cfg}}
	m.applyQuotaView(rt.QuotaView{Providers: []rt.QuotaProviderView{{Provider: "opencode", Enabled: true}}})
	if len(m.snapshots) != 1 || m.snapshots[0].Provider != "opencode" || len(m.snapshots[0].Windows) != 0 {
		t.Fatalf("snapshots = %+v", m.snapshots)
	}
}

func TestRenderSettingsIncludesWebRemote(t *testing.T) {
	cfg := config.Default()
	m := &model{opts: Options{Config: cfg}}
	out := m.renderSettings(100)
	if !strings.Contains(out, "Web remote") {
		t.Fatalf("renderSettings omitted Web remote row:\n%s", out)
	}
}

func TestInProcessAttachExposesLocalSocketForSecondClient(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("TOKENPULSE_ENV", "test")
	t.Setenv("TOKENPULSE_DATA_DIR", dir)
	t.Setenv("TOKENPULSE_CONFIG_PATH", filepath.Join(dir, "config.toml"))

	manager, err := config.New()
	if err != nil {
		t.Fatal(err)
	}
	cfg, err := manager.Load()
	if err != nil {
		t.Fatal(err)
	}
	scanner, err := scan.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	opts := Options{DataDir: dir, Scanner: scanner, Config: cfg, ConfigManager: manager, NoScan: true}
	first, err := attachClient(opts)
	if err != nil {
		t.Fatalf("first attachClient: %v", err)
	}
	defer first.Close()
	if first.IsRemote() {
		t.Fatal("first attachClient should be in-process")
	}

	// Second TUI attaches over the in-process Unix socket.
	second, err := attachClient(opts)
	if err != nil {
		t.Fatalf("second attachClient: %v", err)
	}
	defer second.Close()
	if !second.IsRemote() {
		t.Fatal("second attachClient should attach as SocketClient")
	}
	if _, err := remote.NewLocalClient(dir).Status(); err != nil {
		t.Fatalf("local socket status: %v", err)
	}

	// Shutdown over the in-process socket must be refused (403) so the second
	// TUI cannot stop the first TUI's runtime.
	if err := remote.NewLocalClient(dir).Shutdown(); err == nil {
		t.Fatal("in-process local socket allowed /api/shutdown")
	}

	// Second TUI's Web remote toggle must not stop the first TUI, and its
	// header/settings must show attached-local state rather than web ON.
	m2 := newModel(opts, second)
	if cmd := m2.toggleWebRemote(); cmd != nil {
		t.Fatal("second TUI toggleWebRemote returned a command for attached standalone host")
	}
	if !strings.Contains(m2.renderHeader(100), "local (attached)") {
		t.Fatalf("header = %q, want local (attached)", m2.renderHeader(100))
	}
	if !strings.Contains(m2.renderSettings(100), "attached to local TUI") {
		t.Fatalf("settings = %q, want attached to local TUI", m2.renderSettings(100))
	}
	// First TUI's runtime must still answer normally.
	if _, err := first.Status(); err != nil {
		t.Fatalf("first runtime broken after second TUI toggle: %v", err)
	}
}

func TestWebToggleResubscribesAndFetchesConfig(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("TOKENPULSE_ENV", "test")
	t.Setenv("TOKENPULSE_DATA_DIR", dir)
	t.Setenv("TOKENPULSE_CONFIG_PATH", filepath.Join(dir, "config.toml"))

	manager, _ := config.New()
	cfg, _ := manager.Load()
	scanner, _ := scan.New(dir)
	lock, err := rt.AcquireLock(dir)
	if err != nil {
		t.Fatal(err)
	}
	client, err := newInProcess(Options{DataDir: dir, Scanner: scanner, Config: cfg, ConfigManager: manager, NoScan: true}, lock, false, false)
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()

	m := newModel(Options{DataDir: dir, Scanner: scanner, Config: cfg, ConfigManager: manager, NoScan: true}, client)
	// Error path must restore client and resubscribe events.
	_, cmd := m.Update(webToggleMsg{client: client, err: errors.New("boom")})
	if cmd == nil || m.eventCh == nil {
		t.Fatal("webToggleMsg error did not resubscribe events")
	}
	// Success path must resubscribe events and schedule config fetch.
	_, cmd = m.Update(webToggleMsg{client: client, toast: "ok"})
	if cmd == nil || m.eventCh == nil {
		t.Fatal("webToggleMsg success did not resubscribe events")
	}
}

// Protects tui-usage.md "Range and filter": Overview, Models and Daily all
// show the shared range presets and resolved dates under the tabs.
func TestRangeBarOnRangePages(t *testing.T) {
	h := startTUI(t, newLedger(t, []ledgerRow{{date: today(), model: "claude-sonnet-4-5", tokens: 100}}))
	for _, page := range []string{"Models", "Daily"} {
		h.key("tab")
		found := false
		for _, line := range h.screen() {
			if strings.Contains(line, "1 today") && strings.Contains(line, "→") {
				found = true
				break
			}
		}
		if !found {
			t.Fatalf("%s page has no range bar", page)
		}
	}
}

// Protects tui-usage.md "Overview": a long Y-axis label stays inside the
// chart, so it cannot overwrite the block's left border.
func TestOverviewChartKeepsLeftBorder(t *testing.T) {
	h := startTUI(t, newLedger(t, []ledgerRow{{
		date: today().AddDate(0, 0, -1), model: "claude-sonnet-4-5", tokens: 910_000_000,
	}}))
	lines := h.screen()
	for index, line := range lines {
		if strings.Contains(line, "Token Usage") && strings.HasPrefix(line, "┌") {
			if index+1 >= len(lines) {
				t.Fatal("chart has no content row")
			}
			if !strings.HasPrefix(lines[index+1], "│") {
				t.Fatalf("chart row overwrote the left border: %q", lines[index+1])
			}
			return
		}
	}
	t.Fatal("chart block not found")
}

// Protects tui-usage.md "Keeper": the card's model and last run come from the
// newest execution record of that agent, not the oldest.
func TestKeeperUsesNewestRecord(t *testing.T) {
	cfg := config.Default()
	now := time.Now()
	records := []keeper.ExecutionRecord{
		{Agent: "codex", Model: "older", Timestamp: now.Add(-2 * time.Hour), Success: true, DurationMs: 100},
		{Agent: "codex", Model: "newer", Timestamp: now.Add(-time.Hour), Success: true, DurationMs: 200},
	}
	m := &model{opts: Options{Config: cfg}, cfg: cfg, themeOverride: -1, width: 180, height: 60}
	next, _ := m.Update(keeperLogMsg{records: records})
	out := next.(*model).renderKeeper(180)
	if !strings.Contains(out, "Model: newer") {
		t.Fatalf("Keeper card did not pick the newest record:\n%s", out)
	}
}

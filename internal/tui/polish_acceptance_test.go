package tui

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/report"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
)

func stripANSI(s string) string { return ansi.ReplaceAllString(s, "") }

// Protects tui-theme.md "Auto Theme Detection And Refresh".
func TestAutoThemeDetectsEnvironmentVariables(t *testing.T) {
	cfg := config.Default()
	cfg.Display.Theme = config.ThemeAuto

	t.Run("TOKENPULSE_THEME light", func(t *testing.T) {
		t.Setenv("TOKENPULSE_THEME", "light")
		t.Setenv("COLORFGBG", "")
		t.Setenv("TERM_BACKGROUND", "")
		if got := resolveTheme(cfg, -1).mode; got != 1 {
			t.Fatalf("mode = %d, want 1 (light)", got)
		}
	})

	t.Run("COLORFGBG light and dark", func(t *testing.T) {
		t.Setenv("TOKENPULSE_THEME", "")
		t.Setenv("TERM_BACKGROUND", "")
		t.Setenv("COLORFGBG", "0;15")
		if got := resolveTheme(cfg, -1).mode; got != 1 {
			t.Fatalf("COLORFGBG=0;15 mode = %d, want 1 (light)", got)
		}
		t.Setenv("COLORFGBG", "15;0")
		if got := resolveTheme(cfg, -1).mode; got != 0 {
			t.Fatalf("COLORFGBG=15;0 mode = %d, want 0 (dark)", got)
		}
	})

	t.Run("TERM_BACKGROUND light", func(t *testing.T) {
		t.Setenv("TOKENPULSE_THEME", "")
		t.Setenv("COLORFGBG", "")
		t.Setenv("TERM_BACKGROUND", "light")
		if got := resolveTheme(cfg, -1).mode; got != 1 {
			t.Fatalf("TERM_BACKGROUND=light mode = %d, want 1 (light)", got)
		}
	})
}

// Protects tui-usage.md "Overview": stacked company chart renders without
// N+1 UsageModel calls and KPI omits pricing disclaimer lines while keeping vs.
func TestOverviewRendersCompanyBarsWithoutUsageModelFanoutAndOmitsDisclaimers(t *testing.T) {
	today := startOfToday()
	service := newLedger(t, []ledgerRow{
		{date: today.AddDate(0, 0, -65), model: "claude-opus-4-6", tokens: 5000, cost: 1.0},
		{date: today.AddDate(0, 0, -2), model: "claude-opus-4-6", tokens: 6000, cost: 2.0},
		{date: today.AddDate(0, 0, -1), model: "gpt-5", tokens: 7000, cost: 3.0},
		{date: today, model: "gemini-2.5-pro", tokens: 8000, cost: 4.0},
	})
	h := startTUI(t, service)

	if calls := h.client.models(); len(calls) != 0 {
		t.Fatalf("expected 0 UsageModel calls for Overview chart, got %d: %+v", len(calls), calls)
	}

	plain := stripANSI(h.m.View())
	if strings.Contains(plain, "Est. API pricing") {
		t.Fatalf("Overview KPI must omit API pricing estimate notice:\n%s", plain)
	}
	if strings.Contains(plain, "Some models have no pricing data available.") {
		t.Fatalf("Overview KPI must omit unpriced-model notice:\n%s", plain)
	}
	if !strings.Contains(plain, "vs ") {
		t.Fatalf("Overview period KPI must keep comparison range line:\n%s", plain)
	}
}

// Protects tui-usage.md "Global keys", "Activity", and "Keeper": mouse clicks.
func TestMouseClicksSwitchRangePresetsAndSelectActivityAndKeeper(t *testing.T) {
	today := startOfToday()
	service := newLedger(t, []ledgerRow{
		{date: today.AddDate(0, 0, -5), model: "claude-opus-4-6", tokens: 5000, cost: 1.0},
		{date: today, model: "gpt-5", tokens: 8000, cost: 2.0},
	})
	h := startTUI(t, service)
	m := h.m.(*model)

	// Range bar sits at y = 7 (header 4 + tabs 3). Find "2 7d" column from rendered frame.
	lines := strings.Split(stripANSI(h.m.View()), "\n")
	if len(lines) <= 7 {
		t.Fatalf("frame too short: %d lines", len(lines))
	}
	rangeLine := lines[7]
	idx7d := strings.Index(rangeLine, "2 7d")
	if idx7d < 0 {
		t.Fatalf("range bar missing '2 7d': %q", rangeLine)
	}
	h.send(tea.MouseMsg{X: idx7d + 1, Y: 7, Action: tea.MouseActionPress, Button: tea.MouseButtonLeft})
	if m.preset != "7d" {
		t.Fatalf("clicking '2 7d' set preset = %q, want 7d", m.preset)
	}

	idxCustom := strings.Index(strings.Split(stripANSI(h.m.View()), "\n")[7], "6 custom")
	if idxCustom < 0 {
		t.Fatalf("range bar missing '6 custom'")
	}
	h.send(tea.MouseMsg{X: idxCustom + 1, Y: 7, Action: tea.MouseActionPress, Button: tea.MouseButtonLeft})
	if !m.customOpen {
		t.Fatalf("clicking '6 custom' did not open custom range entry")
	}
	h.send(tea.KeyMsg{Type: tea.KeyEsc})

	// Switch to Keeper and click the second card.
	h.send(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("r")})
	m.switchTab(tabKeeper - m.tab)
	h.send(tea.MouseMsg{X: 80, Y: 12, Action: tea.MouseActionPress, Button: tea.MouseButtonLeft})
	if m.keeperCursor != 1 {
		t.Fatalf("clicking second Keeper card set keeperCursor = %d, want 1", m.keeperCursor)
	}
}

// Protects tui-usage.md "Activity" and "Data loading": Activity preloads after
// initial report, survives preset changes, refreshes incrementally on
// usage_updated, and renders Total/Average/Max plus two-line model rows.
func TestActivityPreloadsSurvivesPresetChangeAndRendersTwoLineModels(t *testing.T) {
	today := startOfToday()
	service := newLedger(t, []ledgerRow{
		{date: today.AddDate(0, 0, -10), model: "claude-opus-4-6", tokens: 5000, cost: 1.0},
		{date: today, model: "deepseek-v4-1-flash", tokens: 12000, cost: 2.5},
	})
	h := startTUI(t, service)
	m := h.m.(*model)

	if !m.activityLoaded {
		t.Fatalf("expected Activity to preload in the background after initial report")
	}
	beforePreset := len(h.client.usage())
	h.send(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("2")}) // switch Overview to 7d
	if !m.activityLoaded {
		t.Fatalf("switching Overview preset invalidated cached Activity data")
	}
	afterPreset := h.client.usage()[beforePreset:]
	for _, call := range afterPreset {
		if call.Preset == "custom" {
			t.Fatalf("preset change triggered unexpected Activity custom-range reload: %+v", call)
		}
	}

	// Simulate usage_updated on the same date: Activity must stay loaded without
	// re-fetching the 365-day custom range.
	beforeEvent := len(h.client.usage())
	m.switchTab(tabActivity - m.tab)
	h.run(m.currentTabCmd())
	h.send(eventMsg{event: rt.Event{Type: "usage_updated"}})
	for _, call := range h.client.usage()[beforeEvent:] {
		if call.Preset == "custom" {
			t.Fatalf("same-day usage_updated reloaded full 365-day Activity instead of refreshing today: %+v", call)
		}
	}

	plain := stripANSI(h.m.View())
	for _, label := range []string{"Total", "Average", "Max"} {
		if !strings.Contains(plain, label) {
			t.Fatalf("Activity Range Overview missing %q:\n%s", label, plain)
		}
	}
	// Two-line model layout: model name line must not share the same line with "CR ".
	for _, line := range strings.Split(plain, "\n") {
		if strings.Contains(line, "deepseek-v4-1-flash") && strings.Contains(line, "CR ") {
			t.Fatalf("Activity Selected Day still puts model name and token metrics on one line: %q", line)
		}
	}
}

// Protects tui-usage.md "Formats": integer counts below 1,000 omit ".00",
// while >= 1,000 keep two-decimal suffixes and costs keep decimals.
func TestCompactCountOmitsDecimalsBelowThousand(t *testing.T) {
	if got := strings.TrimSpace(formatCompact(0)); got != "0" {
		t.Errorf("formatCompact(0) = %q, want 0", got)
	}
	if got := strings.TrimSpace(formatCompact(983)); got != "983" {
		t.Errorf("formatCompact(983) = %q, want 983", got)
	}
	if got := formatCompact(1500); got != "1.50 K" {
		t.Errorf("formatCompact(1500) = %q, want 1.50 K", got)
	}
	if got := formatCost(0); got != "$0.00" {
		t.Errorf("formatCost(0) = %q, want $0.00", got)
	}
}

// Keep report import used even if test helpers shift.
var _ = report.Params{}

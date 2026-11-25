package quota

import (
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

// Protects quota.md "Level and pace": level thresholds.
func TestLevel(t *testing.T) {
	cases := map[float64]string{0: "normal", 79.9: "normal", 80: "warn", 99.9: "warn", 100: "exhausted"}
	for used, want := range cases {
		if got := Level(used); got != want {
			t.Errorf("Level(%v) = %s, want %s", used, got, want)
		}
	}
}

// Protects quota.md "Level and pace": deficit direction and ETA.
func TestPace(t *testing.T) {
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.UTC)
	reset := now.Add(3 * 24 * time.Hour)
	period := int64(7 * 24 * 60 * 60 * 1000)
	window := func(used float64) provider.RateWindow {
		return provider.RateWindow{UsedPercent: used, PeriodMs: &period, ResetsAt: &reset}
	}
	if pace := ComputePace(window(80), now); pace == nil || pace.Kind != "ahead" || pace.EtaMs == nil {
		t.Fatalf("ahead pace = %+v", pace)
	}
	if pace := ComputePace(window(40), now); pace == nil || pace.Kind != "behind" {
		t.Fatalf("behind pace = %+v", pace)
	}
	if pace := ComputePace(window(57), now); pace == nil || pace.Kind != "on_track" {
		t.Fatalf("on-track pace = %+v", pace)
	}
	if pace := ComputePace(window(0), now); pace != nil {
		t.Fatalf("0%% must have no pace: %+v", pace)
	}
	if pace := ComputePace(window(100), now); pace != nil {
		t.Fatalf("100%% must have no pace: %+v", pace)
	}
	missing := provider.RateWindow{UsedPercent: 50}
	if pace := ComputePace(missing, now); pace != nil {
		t.Fatalf("missing period/reset must have no pace: %+v", pace)
	}
}

// Protects quota.md "Order": family first-named, then period ascending.
func TestWindowOrdering(t *testing.T) {
	five := int64(5 * 60 * 60 * 1000)
	week := int64(7 * 24 * 60 * 60 * 1000)
	windows := []provider.RateWindow{
		{Label: "B weekly", Family: "B", PeriodMs: &week},
		{Label: "A weekly", Family: "A", PeriodMs: &week},
		{Label: "none weekly", PeriodMs: &week},
		{Label: "B 5h", Family: "B", PeriodMs: &five},
		{Label: "A unknown", Family: "A"},
	}
	ordered := OrderWindows(windows)
	labels := make([]string, 0, len(ordered))
	for _, window := range ordered {
		labels = append(labels, window.Label)
	}
	want := []string{"B 5h", "B weekly", "A weekly", "A unknown", "none weekly"}
	for i, label := range want {
		if labels[i] != label {
			t.Fatalf("order = %v, want %v", labels, want)
		}
	}
}

// Protects quota.md "Snapshot": bucket ids are kept, else a stable slug.
func TestWindowIDs(t *testing.T) {
	windows := []provider.RateWindow{
		{ID: "gemini-5h", Label: "Gemini (5h)", Family: "Gemini"},
		{Label: "Weekly (7d)", Family: "Opus"},
		{Label: "Session (5h)"},
	}
	AssignWindowIDs(windows)
	if windows[0].ID != "gemini-5h" {
		t.Errorf("bucket id changed: %s", windows[0].ID)
	}
	if windows[1].ID != "opus-weekly-7d" {
		t.Errorf("family slug = %s", windows[1].ID)
	}
	if windows[2].ID != "session-5h" {
		t.Errorf("label slug = %s", windows[2].ID)
	}
}

// Protects quota.md "Snapshot": a window with no family is its own group.
func TestOrderingWithoutFamily(t *testing.T) {
	windows := []provider.RateWindow{{Label: "Session (5h)"}, {Label: "Weekly (7d)"}}
	ordered := OrderWindows(windows)
	if len(ordered) != 2 || ordered[0].Label != "Session (5h)" {
		t.Fatalf("order = %+v", ordered)
	}
}

package tui

import (
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/provider"
)

// Protects tui-usage.md "Quota": out-of-range provider percentages cannot
// produce a negative or oversized bar.
func TestBarCellsClamps(t *testing.T) {
	cases := map[float64]int{-50: 0, 0: 0, 50: 10, 100: 20, 150: 20}
	for percent, want := range cases {
		if got := barCells(percent); got != want {
			t.Errorf("barCells(%v) = %d, want %d", percent, got, want)
		}
	}
}

// Protects tui-usage.md "Model detail": the sparkline writes whole runes and
// maps the largest day to the tallest level.
func TestSparkRuneUsesWholeRunes(t *testing.T) {
	for _, test := range []struct {
		tokens, max int64
		want        string
	}{{0, 10, " "}, {10, 10, "▇"}, {5, 10, "▄"}} {
		got := sparkRune(test.tokens, test.max)
		if got != test.want {
			t.Errorf("sparkRune(%d, %d) = %q, want %q", test.tokens, test.max, got, test.want)
		}
		if utf8.RuneCountInString(got) != 1 {
			t.Errorf("sparkRune(%d, %d) is not one rune", test.tokens, test.max)
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

func TestKeeperCheckMsgDoesNotReschedule(t *testing.T) {
	cfg := config.Default()
	cfg.Keeper.Enabled = true
	m := &model{opts: Options{Config: cfg}}
	_, cmd := m.Update(keeperCheckMsg{})
	if cmd != nil {
		t.Fatal("keeperCheckMsg returned a command; must not reschedule keeper loop")
	}
}

func TestRenderQuotaIncludesPace(t *testing.T) {
	cfg := config.Default()
	resets := time.Now().Add(2 * time.Hour)
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
	if !strings.Contains(out, "ahead") {
		t.Fatalf("renderQuota omitted pace text:\n%s", out)
	}
}

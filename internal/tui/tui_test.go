package tui

import (
	"testing"
	"unicode/utf8"
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

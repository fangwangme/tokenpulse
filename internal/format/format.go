// Package format holds the one display format for money shared by the CLI and
// TUI.
package format

import "fmt"

// Cost prints an amount compactly: two decimals below $1,000, then three
// decimals with a K or M suffix so a growing total does not widen every
// column that shows it.
func Cost(value float64) string {
	switch {
	case value >= 1_000_000:
		return fmt.Sprintf("$%.3fM", value/1_000_000)
	case value >= 1_000:
		return fmt.Sprintf("$%.3fK", value/1_000)
	default:
		return fmt.Sprintf("$%.2f", value)
	}
}

// Package quota fetches and caches provider quota snapshots.
package quota

import (
	"sort"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

// ProviderIDs in default display order.
var ProviderIDs = []string{"claude", "codex", "opencode", "antigravity"}

func IsProvider(id string) bool {
	for _, provider := range ProviderIDs {
		if provider == id {
			return true
		}
	}
	return false
}

func DisplayName(id string) string {
	switch id {
	case "claude":
		return "CLAUDE CODE"
	case "codex":
		return "CODEX"
	case "opencode":
		return "OPENCODE GO"
	case "antigravity":
		return "ANTIGRAVITY"
	default:
		return "UNKNOWN"
	}
}

// Fetcher is one provider's quota source. A fetcher never writes credentials.
type Fetcher interface {
	ProviderID() string
	Fetch() (*provider.QuotaSnapshot, error)
}

// Level is quota.md's level rule.
func Level(usedPercent float64) string {
	if usedPercent >= 100 {
		return "exhausted"
	}
	if usedPercent >= 80 {
		return "warn"
	}
	return "normal"
}

// Pace compares the used share with the elapsed share of a window.
type Pace struct {
	Kind            string
	ExpectedPercent float64
	Deficit         float64
	EtaMs           *int64
}

func ComputePace(window provider.RateWindow, now time.Time) *Pace {
	used := window.UsedPercent
	if used <= 0 || used >= 100 || window.PeriodMs == nil || window.ResetsAt == nil {
		return nil
	}
	periodMs := *window.PeriodMs
	remaining := window.ResetsAt.Sub(now).Milliseconds()
	elapsed := periodMs - remaining
	if elapsed <= 0 || remaining <= 0 || periodMs <= 0 {
		return nil
	}
	expected := float64(elapsed) / float64(periodMs) * 100
	deficit := used - expected
	if deficit > -5 && deficit < 5 {
		return &Pace{Kind: "on_track", ExpectedPercent: expected, Deficit: deficit}
	}
	if deficit > 0 {
		eta := (100 - used) / (used / float64(elapsed))
		etaMs := int64(eta)
		return &Pace{Kind: "ahead", ExpectedPercent: expected, Deficit: deficit, EtaMs: &etaMs}
	}
	return &Pace{Kind: "behind", ExpectedPercent: expected, Deficit: deficit}
}

// OrderWindows groups by family in first-named order, then sorts each group by
// period ascending, unknown last. Windows without a family form one group.
func OrderWindows(windows []provider.RateWindow) []provider.RateWindow {
	groupOrder := []string{}
	groups := map[string][]provider.RateWindow{}
	for _, window := range windows {
		family := window.Family
		if _, ok := groups[family]; !ok {
			groupOrder = append(groupOrder, family)
		}
		groups[family] = append(groups[family], window)
	}
	var out []provider.RateWindow
	for _, family := range groupOrder {
		group := groups[family]
		sort.SliceStable(group, func(i, j int) bool {
			left, right := group[i].PeriodMs, group[j].PeriodMs
			if left == nil && right == nil {
				return false
			}
			if left == nil {
				return false
			}
			if right == nil {
				return true
			}
			return *left < *right
		})
		out = append(out, group...)
	}
	return out
}

func slug(value string) string {
	var b strings.Builder
	lastDash := false
	for _, r := range strings.ToLower(value) {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9':
			b.WriteRune(r)
			lastDash = false
		default:
			if !lastDash && b.Len() > 0 {
				b.WriteByte('-')
				lastDash = true
			}
		}
	}
	return strings.Trim(b.String(), "-")
}

// AssignWindowIDs fills the API id for windows whose provider response did not
// carry a bucket id.
func AssignWindowIDs(windows []provider.RateWindow) {
	for i := range windows {
		if windows[i].ID != "" {
			continue
		}
		if windows[i].Family != "" {
			windows[i].ID = slug(windows[i].Family + " " + windows[i].Label)
		} else {
			windows[i].ID = slug(windows[i].Label)
		}
	}
}

// Normalize applies the shared id and ordering rules to a snapshot.
func Normalize(snapshot *provider.QuotaSnapshot) {
	AssignWindowIDs(snapshot.Windows)
	snapshot.Windows = OrderWindows(snapshot.Windows)
}

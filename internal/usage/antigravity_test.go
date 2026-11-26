package usage

import (
	"testing"
	"time"
)

// Protects usage-sources.md "Antigravity": incremental parsing starts at local
// midnight, not UTC midnight.
func TestMidnightMillisUsesLocation(t *testing.T) {
	zone := time.FixedZone("UTC+8", 8*60*60)
	since := time.Date(2026, 11, 20, 0, 30, 0, 0, zone)
	want := time.Date(2026, 11, 20, 0, 0, 0, 0, zone).UnixMilli()
	if got := midnightMillis(since, zone); got != want {
		t.Fatalf("midnight = %d, want %d", got, want)
	}
	utcMidnight := time.Date(2026, 11, 20, 0, 0, 0, 0, time.UTC).UnixMilli()
	if want == utcMidnight {
		t.Fatal("test zone must differ from UTC")
	}
}

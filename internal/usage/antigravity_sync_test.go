package usage

import "testing"

// Protects usage-sources.md "Antigravity": a bare language-server address is
// HTTP, and Unix-second timestamps are converted to milliseconds.
func TestParseLSAddressDefaultsToHTTP(t *testing.T) {
	port, scheme := parseLSAddress("127.0.0.1:51234")
	if port != 51234 || scheme != "http" {
		t.Fatalf("bare address = %d %q", port, scheme)
	}
	port, scheme = parseLSAddress("https://localhost:51235")
	if port != 51235 || scheme != "https" {
		t.Fatalf("https address = %d %q", port, scheme)
	}
}

func TestParseTimestampValueConvertsSeconds(t *testing.T) {
	got := parseTimestampValue(float64(1764000000))
	if got == nil || *got != 1764000000000 {
		t.Fatalf("seconds = %v", got)
	}
	got = parseTimestampValue("1764000000123")
	if got == nil || *got != 1764000000123 {
		t.Fatalf("milliseconds = %v", got)
	}
}

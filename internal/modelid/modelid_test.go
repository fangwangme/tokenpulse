package modelid

import "testing"

// Protects model-ids.md step 6: thinking-level variant suffixes including
// -xhigh fold into the base canonical model ID.
func TestCanonicalFoldsXHighThinkingLevel(t *testing.T) {
	cases := []struct {
		in   string
		want string
	}{
		{"gemini-4-hydrogen-xhigh", "gemini-4-hydrogen"},
		{"gemini-4-neon-xhigh", "gemini-4-neon"},
		{"claude-opus-4-6-xhigh", "claude-opus-4-6"},
	}
	for _, tc := range cases {
		if got := Canonical(tc.in); got != tc.want {
			t.Errorf("Canonical(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

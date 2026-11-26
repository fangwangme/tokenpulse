package usage

import (
	"testing"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

// Protects usage-report.md "By model": legacy Antigravity sources collapse to
// one agent and a message key duplicated across clients counts once.
func TestLoadModelSummariesDeduplicatesAntigravity(t *testing.T) {
	store := NewStore(t.TempDir() + "/usage.db")
	stamp := int64(1764000000000)
	build := func(source string, input int64) provider.UnifiedMessage {
		return provider.NewMessage(source, "gemini-3-pro", "google", "session", "same-key", stamp,
			provider.TokenBreakdown{Input: input})
	}
	cost := func(string, string, string, provider.TokenBreakdown) (float64, bool) { return 0, false }
	if _, err := store.IngestMessages([]provider.UnifiedMessage{
		build("antigravity", 10), build("antigravity-cli", 5),
	}, cost); err != nil {
		t.Fatal(err)
	}
	rows, err := store.LoadModelSummaries(nil, nil, []string{"antigravity"})
	if err != nil {
		t.Fatal(err)
	}
	if len(rows) != 1 {
		t.Fatalf("rows = %+v", rows)
	}
	row := rows[0]
	if row.Tokens != 10 || row.MessageCount != 1 || len(row.Sessions) != 1 || !row.Sources["antigravity"] {
		t.Fatalf("row = %+v", row)
	}
}

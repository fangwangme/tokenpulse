package scan

import (
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/pricing"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

type emptyParser struct{}

func (emptyParser) Name() string                           { return "fake" }
func (emptyParser) ParserVersion() string                  { return "fake-v1" }
func (emptyParser) SessionPaths() []string                 { return nil }
func (emptyParser) IncrementalMode() usage.IncrementalMode { return usage.UpsertMessages }
func (emptyParser) Parse(*time.Time) ([]provider.UnifiedMessage, error) {
	return nil, nil
}

func testScanner(t *testing.T) *Scanner {
	t.Helper()
	dir := t.TempDir()
	scanner, err := New(dir)
	if err != nil {
		t.Fatal(err)
	}
	scanner.Pricing.Catalog = map[string]pricing.Price{"dummy": {Input: 1}}
	scanner.Pricing.FetchedAt = time.Now()
	scanner.Pricing.Loaded = true
	scanner.testParsers = []usage.Parser{emptyParser{}}
	return scanner
}

func insertFake(t *testing.T, scanner *Scanner, date string) {
	t.Helper()
	stamp, _ := time.ParseInLocation("2006-01-02", date, time.Local)
	message := provider.NewMessage("fake", "model", "provider", "session", "key-"+date, stamp.UnixMilli(), provider.TokenBreakdown{Input: 10})
	message.Date = date
	if _, err := scanner.Store.IngestMessages([]provider.UnifiedMessage{message}, func(string, string, string, provider.TokenBreakdown) (float64, bool) {
		return 0, false
	}); err != nil {
		t.Fatal(err)
	}
}

func fakeRows(t *testing.T, scanner *Scanner) int {
	t.Helper()
	db, err := scanner.Store.Open()
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	var count int
	if err := db.QueryRow("SELECT COUNT(*) FROM usage_messages WHERE source = 'fake'").Scan(&count); err != nil {
		t.Fatal(err)
	}
	return count
}

// Protects go-rewrite.md "Scanning": an explicit rebuild that parses no rows
// removes the source's stale ledger rows instead of keeping them.
func TestRebuildAllEmptyParseDeletesOldRows(t *testing.T) {
	scanner := testScanner(t)
	insertFake(t, scanner, "2026-11-20")
	if err := scanner.Run(Options{RebuildAll: true}); err != nil {
		t.Fatal(err)
	}
	if rows := fakeRows(t, scanner); rows != 0 {
		t.Fatalf("rows after empty rebuild = %d, want 0", rows)
	}
}

// Protects go-rewrite.md "Scanning": a refresh range with no end deletes only
// rows at or after its start and does not dereference a nil RefreshTo.
func TestRefreshOpenEndedRangeDeletesOnlyMatchingRows(t *testing.T) {
	scanner := testScanner(t)
	insertFake(t, scanner, "2026-11-20")
	insertFake(t, scanner, "2026-11-22")
	from := time.Date(2026, 11, 21, 0, 0, 0, 0, time.Local)
	if err := scanner.Run(Options{RefreshFrom: &from}); err != nil {
		t.Fatal(err)
	}
	db, err := scanner.Store.Open()
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	var dates []string
	rows, err := db.Query("SELECT date FROM usage_messages WHERE source = 'fake' ORDER BY date")
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	for rows.Next() {
		var date string
		if err := rows.Scan(&date); err != nil {
			t.Fatal(err)
		}
		dates = append(dates, date)
	}
	if len(dates) != 1 || dates[0] != "2026-11-20" {
		t.Fatalf("remaining dates = %v", dates)
	}
}

func TestFilterRangeAllowsMissingBounds(t *testing.T) {
	message := func(date string) provider.UnifiedMessage {
		return provider.UnifiedMessage{Date: date}
	}
	all := []provider.UnifiedMessage{message("2026-11-20"), message("2026-11-22")}
	from := time.Date(2026, 11, 21, 0, 0, 0, 0, time.Local)
	if got := filterRange(all, &from, nil); len(got) != 1 || got[0].Date != "2026-11-22" {
		t.Fatalf("from-only = %+v", got)
	}
	to := time.Date(2026, 11, 21, 0, 0, 0, 0, time.Local)
	if got := filterRange(all, nil, &to); len(got) != 1 || got[0].Date != "2026-11-20" {
		t.Fatalf("to-only = %+v", got)
	}
	if got := filterRange(all, nil, nil); len(got) != 2 {
		t.Fatalf("no bounds = %+v", got)
	}
}

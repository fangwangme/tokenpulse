package parity

import (
	"database/sql"
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

// ledgerRow is the parity identity of one stored message. Costs are excluded
// on purpose: pricing.md changes them.
type ledgerRow struct {
	Source           string `json:"source"`
	Client           string `json:"client"`
	Session          string `json:"session"`
	MessageKey       string `json:"messageKey"`
	Date             string `json:"date"`
	TimestampMs      int64  `json:"timestampMs"`
	ModelID          string `json:"modelId"`
	CanonicalModelID string `json:"canonicalModelId"`
	ServedModel      string `json:"servedModel"`
	ParserVersion    string `json:"parserVersion"`
	Input            int64  `json:"input"`
	Output           int64  `json:"output"`
	CacheRead        int64  `json:"cacheRead"`
	CacheWrite       int64  `json:"cacheWrite"`
	Reasoning        int64  `json:"reasoning"`
	Total            int64  `json:"total"`
}

type golden struct {
	RustRows          []ledgerRow                  `json:"rustRows"`
	ExpectedOverrides map[string]map[string]string `json:"expectedOverrides"`
	OpencodeOverride  struct {
		Env      map[string]string `json:"env"`
		RustRows []ledgerRow       `json:"rustRows"`
	} `json:"opencodeOverride"`
}

// Protects go-rewrite.md "Parity": every usage source's fixture rows match the
// 0.6.16 golden for identity, dedup, dates, parser versions and tokens. The
// only allowed differences are the unversioned-family fixtures this release
// resolves on purpose; their expected ids are listed in the golden.
func TestParityFixtures(t *testing.T) {
	oldLocal := time.Local
	time.Local = time.UTC
	t.Cleanup(func() { time.Local = oldLocal })
	root := filepath.Join("..", "..", "testdata", "parity")
	var want golden
	data, err := os.ReadFile(filepath.Join(root, "golden.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(data, &want); err != nil {
		t.Fatal(err)
	}

	home := copyFixtureHome(t, root)
	t.Setenv("HOME", home)
	t.Setenv("XDG_DATA_HOME", filepath.Join(home, ".local", "share"))
	t.Setenv("CODEX_HOME", filepath.Join(home, ".codex-home"))
	t.Setenv("OPENCODE_DB", "")

	dataDir := filepath.Join(home, ".local", "share", "tokenpulse")
	releases := usage.ReleaseMap{
		"gemini-2-5-flash": "2025-05-01",
		"gemini-3-5-flash": "2026-05-01",
		"gemini-4-carbon":  "2026-09-01",
	}
	parsers := []usage.Parser{
		usage.NewClaudeParser(),
		usage.NewCodexParser(),
		usage.NewCopilotParser(),
		usage.NewOpenCodeParser(),
		usage.NewGeminiParser(),
		usage.NewPiParser(),
		func() usage.Parser {
			parser := usage.NewAntigravityParser(filepath.Join(dataDir, "antigravity-cache"), releases)
			parser.SkipLanguageServers = true
			return parser
		}(),
	}
	store := usage.NewStore(filepath.Join(dataDir, "usage.db"))
	db := ingest(t, store, parsers)

	got := readRows(t, db)
	wantRows := applyOverrides(want.RustRows, want.ExpectedOverrides)
	assertRows(t, got, wantRows)
	assertTotals(t, db, wantRows)

	// OPENCODE_DB replaces database discovery.
	t.Setenv("OPENCODE_DB", want.OpencodeOverride.Env["OPENCODE_DB"])
	overrideStore := usage.NewStore(filepath.Join(dataDir, "override.db"))
	overrideDB := ingest(t, overrideStore, []usage.Parser{usage.NewOpenCodeParser()})
	assertRows(t, readRows(t, overrideDB), want.OpencodeOverride.RustRows)
}

func copyFixtureHome(t *testing.T, root string) string {
	t.Helper()
	source := filepath.Join(root, "home")
	target := t.TempDir()
	err := filepath.Walk(source, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		relative, err := filepath.Rel(source, path)
		if err != nil {
			return err
		}
		destination := filepath.Join(target, relative)
		if info.IsDir() {
			return os.MkdirAll(destination, 0o755)
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		return os.WriteFile(destination, data, 0o644)
	})
	if err != nil {
		t.Fatal(err)
	}
	return target
}

func ingest(t *testing.T, store *usage.Store, parsers []usage.Parser) *sql.DB {
	t.Helper()
	cost := func(string, string, string, provider.TokenBreakdown) (float64, bool) { return 0, false }
	for _, parser := range parsers {
		messages, err := parser.Parse(nil)
		if err != nil {
			t.Fatalf("%s: %v", parser.Name(), err)
		}
		var err2 error
		if parser.IncrementalMode() == usage.ReplaceChangedSessions {
			_, err2 = store.ReplaceSessionsMessages(messages, cost)
		} else {
			_, err2 = store.IngestMessages(messages, cost)
		}
		if err2 != nil {
			t.Fatalf("%s ingest: %v", parser.Name(), err2)
		}
	}
	db, err := store.Open()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	return db
}

func readRows(t *testing.T, db *sql.DB) []ledgerRow {
	t.Helper()
	rows, err := db.Query(`SELECT source, client, session_id, message_key, date, timestamp_ms,
        model_id, canonical_model_id, COALESCE(served_model, ''), parser_version,
        input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
        reasoning_tokens, total_tokens FROM usage_messages`)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var out []ledgerRow
	for rows.Next() {
		var row ledgerRow
		if err := rows.Scan(&row.Source, &row.Client, &row.Session, &row.MessageKey, &row.Date,
			&row.TimestampMs, &row.ModelID, &row.CanonicalModelID, &row.ServedModel, &row.ParserVersion,
			&row.Input, &row.Output, &row.CacheRead, &row.CacheWrite, &row.Reasoning, &row.Total); err != nil {
			t.Fatal(err)
		}
		out = append(out, row)
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Source != out[j].Source {
			return out[i].Source < out[j].Source
		}
		return out[i].MessageKey < out[j].MessageKey
	})
	return out
}

func applyOverrides(rows []ledgerRow, overrides map[string]map[string]string) []ledgerRow {
	out := make([]ledgerRow, len(rows))
	copy(out, rows)
	for i := range out {
		override, ok := overrides[out[i].MessageKey]
		if !ok {
			continue
		}
		if value, ok := override["modelId"]; ok {
			out[i].ModelID = value
		}
		if value, ok := override["canonicalModelId"]; ok {
			out[i].CanonicalModelID = value
		}
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Source != out[j].Source {
			return out[i].Source < out[j].Source
		}
		return out[i].MessageKey < out[j].MessageKey
	})
	return out
}

func assertRows(t *testing.T, got, want []ledgerRow) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("row count = %d, want %d", len(got), len(want))
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("row %d\n got %+v\nwant %+v", i, got[i], want[i])
		}
	}
}

type aggregate struct {
	tokens, messages, sessions int64
}

func assertTotals(t *testing.T, db *sql.DB, rows []ledgerRow) {
	t.Helper()
	wantDay := map[string]*aggregate{}
	wantModel := map[string]*aggregate{}
	wantDaySessions := map[string]map[string]bool{}
	wantModelSessions := map[string]map[string]bool{}
	seen := map[string]bool{}
	for _, row := range rows {
		dayKey := row.Date
		modelKey := row.Source + "\x00" + row.CanonicalModelID
		if wantDay[dayKey] == nil {
			wantDay[dayKey] = &aggregate{}
			wantDaySessions[dayKey] = map[string]bool{}
		}
		if wantModel[modelKey] == nil {
			wantModel[modelKey] = &aggregate{}
			wantModelSessions[modelKey] = map[string]bool{}
		}
		wantDay[dayKey].tokens += row.Total
		wantDay[dayKey].messages++
		wantDaySessions[dayKey][row.Session] = true
		if !seen[row.Source+"\x00"+row.Client+"\x00"+row.Session] {
			seen[row.Source+"\x00"+row.Client+"\x00"+row.Session] = true
			wantDay[dayKey].sessions++
		}
		wantModel[modelKey].tokens += row.Total
		wantModel[modelKey].messages++
		wantModelSessions[modelKey][row.Session] = true
	}
	for key, sessions := range wantDaySessions {
		wantDay[key].sessions = int64(len(sessions))
	}
	for key, sessions := range wantModelSessions {
		wantModel[key].sessions = int64(len(sessions))
	}

	days := map[string]*aggregate{}
	rows2, err := db.Query("SELECT date, SUM(total_tokens), SUM(message_count), SUM(session_count) FROM daily_model_usage GROUP BY date")
	if err != nil {
		t.Fatal(err)
	}
	defer rows2.Close()
	for rows2.Next() {
		var date string
		var sum aggregate
		if err := rows2.Scan(&date, &sum.tokens, &sum.messages, &sum.sessions); err != nil {
			t.Fatal(err)
		}
		days[date] = &sum
	}
	for date, want := range wantDay {
		got := days[date]
		if got == nil || got.tokens != want.tokens || got.messages != want.messages {
			t.Errorf("day %s = %+v, want %+v", date, got, want)
		}
	}

	models := map[string]*aggregate{}
	rows3, err := db.Query("SELECT source, model_id, SUM(total_tokens), SUM(message_count), SUM(session_count) FROM daily_model_usage GROUP BY source, model_id")
	if err != nil {
		t.Fatal(err)
	}
	defer rows3.Close()
	for rows3.Next() {
		var source, model string
		var sum aggregate
		if err := rows3.Scan(&source, &model, &sum.tokens, &sum.messages, &sum.sessions); err != nil {
			t.Fatal(err)
		}
		models[source+"\x00"+model] = &sum
	}
	for key, want := range wantModel {
		got := models[key]
		if got == nil || got.tokens != want.tokens || got.messages != want.messages {
			t.Errorf("model %s = %+v, want %+v", key, got, want)
		}
	}
}

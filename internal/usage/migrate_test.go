package usage

import (
	"database/sql"
	"os"
	"path/filepath"
	"testing"
)

func oldUsageDB(t *testing.T, path string) *sql.DB {
	t.Helper()
	db, err := sql.Open("sqlite", "file:"+path)
	if err != nil {
		t.Fatal(err)
	}
	_, err = db.Exec(`
CREATE TABLE usage_messages (
    source TEXT NOT NULL,
    client TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    canonical_model_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    message_key TEXT NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    date TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL,
    cache_write_tokens INTEGER NOT NULL,
    reasoning_tokens INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    cost_usd REAL NOT NULL,
    pricing_day TEXT NOT NULL,
    parser_version TEXT NOT NULL,
    PRIMARY KEY (source, client, message_key)
);
CREATE TABLE daily_model_usage (
    date TEXT NOT NULL,
    source TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL,
    cache_write_tokens INTEGER NOT NULL,
    reasoning_tokens INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    cost_usd REAL NOT NULL,
    message_count INTEGER NOT NULL,
    session_count INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (date, source, provider_id, model_id)
);
CREATE TABLE source_refresh_state (
    source TEXT PRIMARY KEY,
    last_refresh_started_ms INTEGER NOT NULL
);
PRAGMA user_version = 6;
INSERT INTO usage_messages VALUES
 ('claude','claude','anthropic','claude-opus-4-6','claude-opus-4-6','s1','msg-1',1764000000000,'2026-11-20',10,5,0,0,0,15,0,'2026-11-20','claude-v4');
`)
	if err != nil {
		t.Fatal(err)
	}
	db.Close()
	return nil
}

// Protects go-rewrite.md "Existing data": the first open backs up a 0.6.x
// ledger, keeps every row and cost, adds unpriced, and leaves an existing cost
// above zero untouched.
func TestMigrationBackupAndRows(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "usage.db")
	oldUsageDB(t, path)
	schemaMu.Lock()
	delete(schemaDone, path)
	schemaMu.Unlock()

	store := NewStore(path)
	db, err := store.Open()
	if err != nil {
		t.Fatal(err)
	}
	var source, model, parser string
	var input, output, total int64
	var cost float64
	var unpriced int
	if err := db.QueryRow(`SELECT source, model_id, parser_version, input_tokens, output_tokens,
        total_tokens, cost_usd, unpriced FROM usage_messages`).
		Scan(&source, &model, &parser, &input, &output, &total, &cost, &unpriced); err != nil {
		t.Fatal(err)
	}
	if source != "claude" || model != "claude-opus-4-6" || parser != "claude-v4" ||
		input != 10 || output != 5 || total != 15 || cost != 0 || unpriced != 1 {
		t.Fatalf("migrated row = %s %s %s %d %d %d %v %d", source, model, parser, input, output, total, cost, unpriced)
	}
	var version int
	_ = db.QueryRow("PRAGMA user_version").Scan(&version)
	if version != 7 {
		t.Fatalf("user_version = %d", version)
	}
	db.Close()

	backups, _ := filepath.Glob(filepath.Join(dir, "usage.db.pre-0.7.*.bak"))
	if len(backups) != 1 {
		t.Fatalf("backups = %v", backups)
	}
	info, err := os.Stat(backups[0])
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("backup mode = %v, %v", info.Mode(), err)
	}
	backup, err := sql.Open("sqlite", "file:"+backups[0]+"?mode=ro")
	if err != nil {
		t.Fatal(err)
	}
	defer backup.Close()
	var count int
	if err := backup.QueryRow("SELECT COUNT(*) FROM usage_messages").Scan(&count); err != nil || count != 1 {
		t.Fatalf("backup rows = %d, %v", count, err)
	}
}

// Protects go-rewrite.md "Existing data": a second run writes no backup and
// changes no ledger row.
func TestSecondOpenDoesNotBackupAgain(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "usage.db")
	oldUsageDB(t, path)
	schemaMu.Lock()
	delete(schemaDone, path)
	schemaMu.Unlock()
	first, err := NewStore(path).Open()
	if err != nil {
		t.Fatal(err)
	}
	first.Close()
	schemaMu.Lock()
	delete(schemaDone, path)
	schemaMu.Unlock()
	second, err := NewStore(path).Open()
	if err != nil {
		t.Fatal(err)
	}
	defer second.Close()
	backups, _ := filepath.Glob(filepath.Join(dir, "usage.db.pre-0.7.*.bak"))
	if len(backups) != 1 {
		t.Fatalf("second open wrote another backup: %v", backups)
	}
	var rows, costRows int
	_ = second.QueryRow("SELECT COUNT(*) FROM usage_messages").Scan(&rows)
	_ = second.QueryRow("SELECT COUNT(*) FROM usage_messages WHERE cost_usd = 0").Scan(&costRows)
	if rows != 1 || costRows != 1 {
		t.Fatalf("rows=%d zero-cost=%d", rows, costRows)
	}
}

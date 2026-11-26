package keeper

import (
	"database/sql"
	"os"
	"path/filepath"

	_ "modernc.org/sqlite"
)

func openHistory(path string) (*sql.DB, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, err
	}
	db, err := sql.Open("sqlite", "file:"+path+"?_pragma=busy_timeout(5000)&_pragma=journal_mode(WAL)")
	if err != nil {
		return nil, err
	}
	_, err = db.Exec(`CREATE TABLE IF NOT EXISTS keeper_executions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        agent TEXT NOT NULL,
        trigger_type TEXT NOT NULL,
        executed_at TEXT NOT NULL,
        model TEXT NOT NULL,
        prompt TEXT NOT NULL,
        command TEXT NOT NULL,
        duration_ms INTEGER NOT NULL,
        success INTEGER NOT NULL,
        exit_code INTEGER,
        output_snippet TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_keeper_executions_time ON keeper_executions(executed_at);
    CREATE INDEX IF NOT EXISTS idx_keeper_executions_agent_time ON keeper_executions(agent, executed_at);`)
	if err != nil {
		db.Close()
		return nil, err
	}
	return db, nil
}

package quota

import (
	"database/sql"
	"os"
	"path/filepath"
	"time"

	_ "modernc.org/sqlite"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

const cacheTTL = 5 * time.Minute

// Cache stores one snapshot per provider and appends observation history.
type Cache struct {
	Path string
}

func NewCache(dataDir string) *Cache {
	return &Cache{Path: filepath.Join(dataDir, "tokenpulse.db")}
}

func (c *Cache) open() (*sql.DB, error) {
	if err := os.MkdirAll(filepath.Dir(c.Path), 0o755); err != nil {
		return nil, err
	}
	db, err := sql.Open("sqlite", "file:"+c.Path+"?_pragma=busy_timeout(5000)&_pragma=journal_mode(WAL)")
	if err != nil {
		return nil, err
	}
	if err := ensureQuotaSchema(db); err != nil {
		db.Close()
		return nil, err
	}
	return db, nil
}

type Cached struct {
	Provider   string
	ObservedAt time.Time
	ExpiresAt  time.Time
	Snapshot   provider.QuotaSnapshot
}

// LoadValid returns a snapshot only while its cache entry is fresh.
func (c *Cache) LoadValid(providerID string, now time.Time) (*Cached, error) {
	cached, err := c.LoadAny(providerID)
	if err != nil || cached == nil {
		return nil, err
	}
	if !cached.ExpiresAt.After(now) {
		return nil, nil
	}
	return cached, nil
}

// LoadAny returns the last snapshot even when expired; a failed fetch shows it
// next to the error.
func (c *Cache) LoadAny(providerID string) (*Cached, error) {
	db, err := c.open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	var observed, expires, fetched, raw string
	err = db.QueryRow(`SELECT observed_at, expires_at, fetched_at, snapshot_json FROM quota_cache WHERE provider = ?1`, providerID).
		Scan(&observed, &expires, &fetched, &raw)
	if err == sql.ErrNoRows {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var snapshot provider.QuotaSnapshot
	if err := snapshot.UnmarshalCacheJSON([]byte(raw)); err != nil {
		return nil, err
	}
	observedAt, err := time.Parse(time.RFC3339Nano, observed)
	if err != nil {
		return nil, err
	}
	expiresAt, err := time.Parse(time.RFC3339Nano, expires)
	if err != nil {
		return nil, err
	}
	return &Cached{Provider: providerID, ObservedAt: observedAt, ExpiresAt: expiresAt, Snapshot: snapshot}, nil
}

// Save overwrites the provider's cache entry and appends history.
func (c *Cache) Save(providerID string, observedAt time.Time, snapshot *provider.QuotaSnapshot) error {
	db, err := c.open()
	if err != nil {
		return err
	}
	defer db.Close()
	expires := observedAt.Add(cacheTTL)
	raw, err := snapshot.MarshalCacheJSON()
	if err != nil {
		return err
	}
	if _, err := db.Exec(`INSERT INTO quota_cache (provider, observed_at, expires_at, fetched_at, snapshot_json)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(provider) DO UPDATE SET observed_at = excluded.observed_at,
            expires_at = excluded.expires_at, fetched_at = excluded.fetched_at,
            snapshot_json = excluded.snapshot_json`,
		providerID, observedAt.Format(time.RFC3339Nano), expires.Format(time.RFC3339Nano),
		snapshot.FetchedAt.Format(time.RFC3339Nano), string(raw)); err != nil {
		return err
	}
	return c.RecordSnapshot(providerID, observedAt, snapshot)
}

// RecordSnapshot appends one observation row per window plus credits.
func (c *Cache) RecordSnapshot(providerID string, observedAt time.Time, snapshot *provider.QuotaSnapshot) error {
	db, err := c.open()
	if err != nil {
		return err
	}
	defer db.Close()
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	stmt, err := tx.Prepare(`INSERT INTO quota_observations (
        provider, observed_at, fetched_at, plan, account, window_label, model_family,
        used_percent, resets_at, period_duration_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`)
	if err != nil {
		return err
	}
	for _, window := range snapshot.Windows {
		var resets any
		if window.ResetsAt != nil {
			resets = window.ResetsAt.Format(time.RFC3339Nano)
		}
		if _, err := stmt.Exec(providerID, observedAt.Format(time.RFC3339Nano),
			snapshot.FetchedAt.Format(time.RFC3339Nano), nullable(snapshot.Plan), nullable(snapshot.Account),
			window.Label, nullable(window.Family), window.UsedPercent, resets, window.PeriodMs); err != nil {
			stmt.Close()
			return err
		}
	}
	stmt.Close()
	if snapshot.Credits != nil {
		if _, err := tx.Exec(`INSERT INTO quota_credit_observations (provider, observed_at, used, credit_limit, currency)
            VALUES (?, ?, ?, ?, ?)`, providerID, observedAt.Format(time.RFC3339Nano),
			snapshot.Credits.Used, snapshot.Credits.Limit, snapshot.Credits.Currency); err != nil {
			return err
		}
	}
	return tx.Commit()
}

// RecordFailure appends one failed poll.
func (c *Cache) RecordFailure(providerID string, observedAt time.Time, message string) error {
	db, err := c.open()
	if err != nil {
		return err
	}
	defer db.Close()
	_, err = db.Exec("INSERT INTO quota_fetch_failures (provider, observed_at, error) VALUES (?, ?, ?)",
		providerID, observedAt.Format(time.RFC3339Nano), message)
	return err
}

func nullable(value string) any {
	if value == "" {
		return nil
	}
	return value
}

func ensureQuotaSchema(db *sql.DB) error {
	if _, err := db.Exec(`
CREATE TABLE IF NOT EXISTS quota_cache (
    provider TEXT PRIMARY KEY,
    observed_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    snapshot_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS quota_observations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    plan TEXT,
    account TEXT,
    window_label TEXT NOT NULL,
    model_family TEXT,
    used_percent REAL NOT NULL,
    resets_at TEXT,
    period_duration_ms INTEGER
);
CREATE TABLE IF NOT EXISTS quota_credit_observations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    used REAL NOT NULL,
    credit_limit REAL,
    currency TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS quota_fetch_failures (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    error TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS keeper_executions (
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
CREATE INDEX IF NOT EXISTS idx_quota_observations_provider_time ON quota_observations(provider, observed_at);
CREATE INDEX IF NOT EXISTS idx_quota_observations_window_time ON quota_observations(provider, window_label, observed_at);
CREATE INDEX IF NOT EXISTS idx_quota_observations_family_time ON quota_observations(provider, model_family, observed_at);
CREATE INDEX IF NOT EXISTS idx_quota_credit_observations_provider_time ON quota_credit_observations(provider, observed_at);
CREATE INDEX IF NOT EXISTS idx_quota_fetch_failures_provider_time ON quota_fetch_failures(provider, observed_at);
CREATE INDEX IF NOT EXISTS idx_keeper_executions_time ON keeper_executions(executed_at);
CREATE INDEX IF NOT EXISTS idx_keeper_executions_agent_time ON keeper_executions(agent, executed_at);
`); err != nil {
		return err
	}
	var version int
	_ = db.QueryRow("PRAGMA user_version").Scan(&version)
	if version < 2 {
		var count int
		_ = db.QueryRow("SELECT COUNT(*) FROM pragma_table_info('quota_observations') WHERE name = 'model_family'").Scan(&count)
		if count == 0 {
			_, _ = db.Exec("ALTER TABLE quota_observations ADD COLUMN model_family TEXT;")
		}
		if _, err := db.Exec("PRAGMA user_version = 2;"); err != nil {
			return err
		}
	}
	return nil
}

// FetchResult is one provider's poll outcome: a snapshot, an error, or both.
type FetchResult struct {
	Provider  string
	Snapshot  *provider.QuotaSnapshot
	Err       error
	FetchedAt time.Time
}

// FetchAll polls every enabled provider in parallel, saves successes and
// keeps the previous snapshot of a failed provider.
func FetchAll(dataDir string, fetchers []Fetcher) []FetchResult {
	cache := NewCache(dataDir)
	type outcome struct {
		index    int
		snapshot *provider.QuotaSnapshot
		err      error
	}
	results := make(chan outcome, len(fetchers))
	for i, fetcher := range fetchers {
		go func(index int, f Fetcher) {
			snapshot, err := f.Fetch()
			results <- outcome{index: index, snapshot: snapshot, err: err}
		}(i, fetcher)
	}
	out := make([]FetchResult, len(fetchers))
	now := time.Now().UTC()
	for range fetchers {
		result := <-results
		fetcher := fetchers[result.index]
		entry := FetchResult{Provider: fetcher.ProviderID(), Err: result.err, FetchedAt: now}
		if result.snapshot != nil {
			Normalize(result.snapshot)
			entry.Snapshot = result.snapshot
			_ = cache.Save(fetcher.ProviderID(), now, result.snapshot)
		} else {
			_ = cache.RecordFailure(fetcher.ProviderID(), now, errorText(result.err))
			if cached, _ := cache.LoadAny(fetcher.ProviderID()); cached != nil {
				entry.Snapshot = &cached.Snapshot
			}
		}
		out[result.index] = entry
	}
	return out
}

func errorText(err error) string {
	if err == nil {
		return "unknown error"
	}
	return err.Error()
}

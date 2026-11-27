package usage

import (
	"database/sql"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	_ "modernc.org/sqlite"

	"github.com/fangwangme/tokenpulse/internal/modelid"
	"github.com/fangwangme/tokenpulse/internal/provider"
)

const canonicalSourceSQL = "CASE WHEN source IN ('antigravity-cli', 'antigravity-desktop', 'antigravity-ide') THEN 'antigravity' ELSE source END"

const refreshSlackDays = 1
const fallbackLookbackDays = 2

// CostFunc returns a message's price under the current catalog. priced=false
// means no period covers the model and date, so the message is unpriced.
type CostFunc func(canonicalModel, providerID, date string, tokens provider.TokenBreakdown) (cost float64, priced bool)

// Store is the usage ledger.
type Store struct {
	Path     string
	readOnly bool
}

func NewStore(path string) *Store { return &Store{Path: path} }

// NewReadOnlyStore reads an existing ledger without migrations or journal
// changes, so a report never waits behind a running scan.
func NewReadOnlyStore(path string) *Store { return &Store{Path: path, readOnly: true} }

var schemaMu sync.Mutex
var schemaDone = map[string]bool{}

func (s *Store) Open() (*sql.DB, error) {
	if s.readOnly {
		db, err := sql.Open("sqlite", "file:"+s.Path+"?mode=ro&_pragma=busy_timeout(5000)")
		if err != nil {
			return nil, err
		}
		db.SetMaxOpenConns(1)
		return db, nil
	}
	if err := os.MkdirAll(filepath.Dir(s.Path), 0o755); err != nil {
		return nil, err
	}
	db, err := sql.Open("sqlite", "file:"+s.Path+"?_pragma=busy_timeout(5000)&_pragma=journal_mode(WAL)")
	if err != nil {
		return nil, err
	}
	db.SetMaxOpenConns(1)
	schemaMu.Lock()
	defer schemaMu.Unlock()
	if !schemaDone[s.Path] {
		if err := ensureUsageSchema(db, s.Path); err != nil {
			db.Close()
			return nil, err
		}
		schemaDone[s.Path] = true
	}
	return db, nil
}

func (s *Store) LatestMessageDate(source string) (*time.Time, error) {
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	var value sql.NullString
	if source == "antigravity" {
		err = db.QueryRow("SELECT MAX(date) FROM usage_messages WHERE source IN ('antigravity', 'antigravity-cli', 'antigravity-desktop', 'antigravity-ide')").Scan(&value)
	} else {
		err = db.QueryRow("SELECT MAX(date) FROM usage_messages WHERE source = ?1", source).Scan(&value)
	}
	if errors.Is(err, sql.ErrNoRows) || !value.Valid {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return parseDate(value.String)
}

func parseDate(value string) (*time.Time, error) {
	t, err := time.ParseInLocation("2006-01-02", value, time.Local)
	if err != nil {
		return nil, err
	}
	return &t, nil
}

// CheckStaleParserVersions returns sources whose stored rows were written by a
// different parser version than the one that would parse them now.
func (s *Store) CheckStaleParserVersions(versions map[string]string) (map[string]bool, error) {
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	rows, err := db.Query("SELECT DISTINCT source, parser_version FROM usage_messages")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	stale := map[string]bool{}
	for rows.Next() {
		var source, version string
		if err := rows.Scan(&source, &version); err != nil {
			return nil, err
		}
		canonical := source
		if source == "antigravity" || source == "antigravity-cli" || source == "antigravity-desktop" || source == "antigravity-ide" {
			canonical = "antigravity"
		}
		if expected, ok := versions[canonical]; ok && version != expected {
			stale[canonical] = true
		}
	}
	return stale, rows.Err()
}

func (s *Store) RecordRefreshStarted(source string, startedMs int64) error {
	db, err := s.Open()
	if err != nil {
		return err
	}
	defer db.Close()
	_, err = db.Exec(`INSERT INTO source_refresh_state (source, last_refresh_started_ms) VALUES (?1, ?2)
ON CONFLICT(source) DO UPDATE SET last_refresh_started_ms = MAX(last_refresh_started_ms, excluded.last_refresh_started_ms)`, source, startedMs)
	return err
}

// LastRefreshStartedMs returns the exact wall-clock start time (ms) of the
// previous successful refresh for source, or nil when none is recorded.
func (s *Store) LastRefreshStartedMs(source string) (*int64, error) {
	return s.lastRefreshStartedMs(source)
}

func (s *Store) lastRefreshStartedMs(source string) (*int64, error) {
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	var value int64
	err = db.QueryRow("SELECT last_refresh_started_ms FROM source_refresh_state WHERE source = ?1", source).Scan(&value)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &value, nil
}

// DefaultSince is where an incremental refresh starts: just before the
// previous refresh began, one day of slack, or two days before the newest
// stored message when no refresh was recorded.
func (s *Store) DefaultSince(source string, requested *time.Time) (*time.Time, error) {
	latest, err := s.LatestMessageDate(source)
	if err != nil || latest == nil {
		return requested, err
	}
	var inferred *time.Time
	startedMs, err := s.lastRefreshStartedMs(source)
	if err != nil {
		return nil, err
	}
	if startedMs != nil {
		started := time.UnixMilli(*startedMs).Local()
		d := time.Date(started.Year(), started.Month(), started.Day(), 0, 0, 0, 0, time.Local).AddDate(0, 0, -refreshSlackDays)
		inferred = &d
	} else {
		d := latest.AddDate(0, 0, -fallbackLookbackDays)
		inferred = &d
	}
	if requested == nil {
		return inferred, nil
	}
	if inferred != nil && requested.Before(*inferred) {
		return inferred, nil
	}
	return requested, nil
}

// IngestMessages upserts messages, derives their cost and rebuilds the daily
// rollups of every affected date in one transaction.
func (s *Store) IngestMessages(messages []provider.UnifiedMessage, cost CostFunc) (map[string]bool, error) {
	if len(messages) == 0 {
		return map[string]bool{}, nil
	}
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	tx, err := db.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	affected := map[string]bool{}
	now := time.Now().UnixMilli()
	for _, message := range messages {
		changed, err := upsertMessage(tx, message, cost)
		if err != nil {
			return nil, err
		}
		if changed {
			affected[message.Date] = true
		}
	}
	sources := map[string]bool{}
	for _, message := range messages {
		sources[message.Client] = true
	}
	if err := collapseCrossClientDuplicates(tx, sources, affected); err != nil {
		return nil, err
	}
	for date := range affected {
		if err := rebuildDaily(tx, date, now); err != nil {
			return nil, err
		}
	}
	return affected, tx.Commit()
}

// ReplaceSessionsMessages drops rows of the same sessions that an older parser
// wrote, then upserts the fresh parse.
func (s *Store) ReplaceSessionsMessages(messages []provider.UnifiedMessage, cost CostFunc) (map[string]bool, error) {
	if len(messages) == 0 {
		return map[string]bool{}, nil
	}
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	tx, err := db.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()

	parserVersions := map[string]string{}
	for _, message := range messages {
		parserVersions[message.Client] = message.ParserVersion
	}
	sessionKeys := map[[3]string]bool{}
	for _, message := range messages {
		client := message.ClientDetail
		if client == "" {
			client = message.Client
		}
		sessionKeys[[3]string{message.Client, client, message.SessionID}] = true
	}
	affected := map[string]bool{}
	for key := range sessionKeys {
		version := parserVersions[key[0]]
		rows, err := tx.Query("SELECT DISTINCT date FROM usage_messages WHERE source = ?1 AND client = ?2 AND session_id = ?3 AND parser_version <> ?4",
			key[0], key[1], key[2], version)
		if err != nil {
			return nil, err
		}
		for rows.Next() {
			var date string
			if err := rows.Scan(&date); err != nil {
				rows.Close()
				return nil, err
			}
			affected[date] = true
		}
		rows.Close()
		if _, err := tx.Exec("DELETE FROM usage_messages WHERE source = ?1 AND client = ?2 AND session_id = ?3 AND parser_version <> ?4",
			key[0], key[1], key[2], version); err != nil {
			return nil, err
		}
	}
	now := time.Now().UnixMilli()
	for _, message := range messages {
		changed, err := upsertMessage(tx, message, cost)
		if err != nil {
			return nil, err
		}
		if changed {
			affected[message.Date] = true
		}
	}
	sources := map[string]bool{}
	for _, message := range messages {
		sources[message.Client] = true
	}
	if err := collapseCrossClientDuplicates(tx, sources, affected); err != nil {
		return nil, err
	}
	for date := range affected {
		if err := rebuildDaily(tx, date, now); err != nil {
			return nil, err
		}
	}
	return affected, tx.Commit()
}

// ReplaceParsedSessions replaces every ledger row of the sessions present in
// messages, deleting rows the fresh parse no longer produces. Sessions with no
// readable source file are not in messages and keep their rows. Costs of rows
// that survive are preserved by upsertMessage; only removed rows disappear.
func (s *Store) ReplaceParsedSessions(messages []provider.UnifiedMessage, cost CostFunc) (map[string]bool, error) {
	if len(messages) == 0 {
		return map[string]bool{}, nil
	}
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	tx, err := db.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()

	parsed := map[[3]string]map[string]bool{}
	for _, message := range messages {
		client := message.ClientDetail
		if client == "" {
			client = message.Client
		}
		key := [3]string{message.Client, client, message.SessionID}
		if parsed[key] == nil {
			parsed[key] = map[string]bool{}
		}
		parsed[key][message.MessageKey] = true
	}

	affected := map[string]bool{}
	for key, keep := range parsed {
		rows, err := tx.Query("SELECT rowid, message_key, date FROM usage_messages WHERE source = ?1 AND client = ?2 AND session_id = ?3",
			key[0], key[1], key[2])
		if err != nil {
			return nil, err
		}
		var deleteIDs []int64
		for rows.Next() {
			var rowID int64
			var messageKey, date string
			if err := rows.Scan(&rowID, &messageKey, &date); err != nil {
				rows.Close()
				return nil, err
			}
			if !keep[messageKey] {
				deleteIDs = append(deleteIDs, rowID)
				affected[date] = true
			}
		}
		if err := rows.Err(); err != nil {
			rows.Close()
			return nil, err
		}
		rows.Close()
		if err := deleteUsageRows(tx, deleteIDs); err != nil {
			return nil, err
		}
	}

	for _, message := range messages {
		changed, err := upsertMessage(tx, message, cost)
		if err != nil {
			return nil, err
		}
		if changed {
			affected[message.Date] = true
		}
	}
	sources := map[string]bool{}
	for _, message := range messages {
		sources[message.Client] = true
	}
	if err := collapseCrossClientDuplicates(tx, sources, affected); err != nil {
		return nil, err
	}
	now := time.Now().UnixMilli()
	for date := range affected {
		if err := rebuildDaily(tx, date, now); err != nil {
			return nil, err
		}
	}
	return affected, tx.Commit()
}

func deleteUsageRows(tx *sql.Tx, rowIDs []int64) error {
	for start := 0; start < len(rowIDs); start += 500 {
		end := start + 500
		if end > len(rowIDs) {
			end = len(rowIDs)
		}
		placeholders := make([]string, end-start)
		args := make([]any, end-start)
		for i, rowID := range rowIDs[start:end] {
			placeholders[i] = "?"
			args[i] = rowID
		}
		if _, err := tx.Exec("DELETE FROM usage_messages WHERE rowid IN ("+strings.Join(placeholders, ",")+")", args...); err != nil {
			return err
		}
	}
	return nil
}

// MigrationDone reports whether a named one-time data repair has run.
func (s *Store) MigrationDone(name string) (bool, error) {
	db, err := s.Open()
	if err != nil {
		return false, err
	}
	defer db.Close()
	var value string
	err = db.QueryRow("SELECT value FROM app_state WHERE key = ?1", name).Scan(&value)
	if errors.Is(err, sql.ErrNoRows) {
		return false, nil
	}
	return err == nil, err
}

// MarkMigration records a completed one-time data repair.
func (s *Store) MarkMigration(name string) error {
	db, err := s.Open()
	if err != nil {
		return err
	}
	defer db.Close()
	_, err = db.Exec(`INSERT INTO app_state (key, value) VALUES (?1, ?2)
ON CONFLICT(key) DO UPDATE SET value = excluded.value`, name, time.Now().UTC().Format(time.RFC3339Nano))
	return err
}

// ReplaceSourceMessages replaces one source's ledger rows, in range or all.
func (s *Store) ReplaceSourceMessages(source string, from, to *time.Time, messages []provider.UnifiedMessage, cost CostFunc) (map[string]bool, error) {
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	tx, err := db.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()

	affected := map[string]bool{}
	rows, err := tx.Query("SELECT DISTINCT date FROM usage_messages WHERE "+sourceFilterSQL(source), sourceArgs(source)...)
	if err != nil {
		return nil, err
	}
	for rows.Next() {
		var date string
		if err := rows.Scan(&date); err != nil {
			rows.Close()
			return nil, err
		}
		day, err := parseDate(date)
		if err != nil {
			continue
		}
		if (from == nil || !day.Before(*from)) && (to == nil || !day.After(*to)) {
			affected[date] = true
		}
	}
	rows.Close()

	deleteSQL := "DELETE FROM usage_messages WHERE " + sourceFilterSQL(source)
	args := sourceArgs(source)
	if from != nil {
		deleteSQL += " AND date >= ?"
		args = append(args, from.Format("2006-01-02"))
	}
	if to != nil {
		deleteSQL += " AND date <= ?"
		args = append(args, to.Format("2006-01-02"))
	}
	if _, err := tx.Exec(deleteSQL, args...); err != nil {
		return nil, err
	}
	dailySQL := "DELETE FROM daily_model_usage WHERE " + sourceFilterSQL(source)
	dailyArgs := sourceArgs(source)
	if from != nil {
		dailySQL += " AND date >= ?"
		dailyArgs = append(dailyArgs, from.Format("2006-01-02"))
	}
	if to != nil {
		dailySQL += " AND date <= ?"
		dailyArgs = append(dailyArgs, to.Format("2006-01-02"))
	}
	if _, err := tx.Exec(dailySQL, dailyArgs...); err != nil {
		return nil, err
	}

	now := time.Now().UnixMilli()
	for _, message := range messages {
		changed, err := upsertMessage(tx, message, cost)
		if err != nil {
			return nil, err
		}
		if changed {
			affected[message.Date] = true
		}
	}
	sources := map[string]bool{source: true}
	if err := collapseCrossClientDuplicates(tx, sources, affected); err != nil {
		return nil, err
	}
	for date := range affected {
		if err := rebuildDaily(tx, date, now); err != nil {
			return nil, err
		}
	}
	return affected, tx.Commit()
}

func sourceFilterSQL(source string) string {
	if source == "antigravity" {
		return "source IN ('antigravity', 'antigravity-cli', 'antigravity-desktop', 'antigravity-ide')"
	}
	return "source = ?"
}

func sourceArgs(source string) []any {
	if source == "antigravity" {
		return nil
	}
	return []any{source}
}

func upsertMessage(tx *sql.Tx, message provider.UnifiedMessage, cost CostFunc) (bool, error) {
	client := message.ClientDetail
	if client == "" {
		client = message.Client
	}
	canonical := modelid.Canonical(message.ModelID)
	price, priced := 0.0, false
	if cost != nil {
		price, priced = cost(canonical, message.ProviderID, message.Date, message.Tokens)
	}
	unpriced := 1
	if priced {
		unpriced = 0
	} else {
		price = 0
	}
	// A stored cost above 0 is a fact: the first sync and every later ingests
	// keep it byte for byte unless the message's model itself changed.
	var oldCost float64
	var oldUnpriced int
	var oldCanonical, oldModel, oldProvider, oldSession, oldDate, oldPricingDay, oldParser string
	var oldTimestamp, oldInput, oldOutput, oldCacheRead, oldCacheWrite, oldReasoning, oldTotal int64
	var oldServed sql.NullString
	err := tx.QueryRow(`SELECT cost_usd, unpriced, canonical_model_id, model_id, provider_id, session_id,
        timestamp_ms, date, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
        reasoning_tokens, total_tokens, pricing_day, parser_version, served_model
        FROM usage_messages WHERE source = ?1 AND client = ?2 AND message_key = ?3`,
		message.Client, client, message.MessageKey).Scan(&oldCost, &oldUnpriced, &oldCanonical,
		&oldModel, &oldProvider, &oldSession, &oldTimestamp, &oldDate, &oldInput, &oldOutput,
		&oldCacheRead, &oldCacheWrite, &oldReasoning, &oldTotal, &oldPricingDay, &oldParser, &oldServed)
	exists := err == nil
	if exists {
		// A stored price is a fact until reprice --apply, even when the model
		// identity was corrected (an unversioned Antigravity family merging
		// into its dated model).
		if oldUnpriced == 0 && oldCost > 0 {
			price = oldCost
			unpriced = 0
		}
		served := ""
		if oldServed.Valid {
			served = oldServed.String
		}
		if oldModel == message.ModelID && oldProvider == message.ProviderID && oldSession == message.SessionID &&
			oldTimestamp == message.Timestamp && oldDate == message.Date && oldInput == message.Tokens.Input &&
			oldOutput == message.Tokens.Output && oldCacheRead == message.Tokens.CacheRead &&
			oldCacheWrite == message.Tokens.CacheWrite && oldReasoning == message.Tokens.Reasoning &&
			oldTotal == message.TotalTokens() && oldPricingDay == message.PricingDay &&
			oldParser == message.ParserVersion && served == message.ServedModel &&
			oldCanonical == canonical && oldCost == price && oldUnpriced == unpriced {
			return false, nil
		}
	}
	_, err = tx.Exec(`INSERT INTO usage_messages (
        source, client, provider_id, model_id, canonical_model_id, session_id, message_key,
        timestamp_ms, date, input_tokens, output_tokens,
        cache_read_tokens, cache_write_tokens, reasoning_tokens,
        total_tokens, cost_usd, pricing_day, parser_version, served_model, unpriced
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
    ON CONFLICT(source, client, message_key) DO UPDATE SET
        provider_id = excluded.provider_id,
        model_id = excluded.model_id,
        canonical_model_id = excluded.canonical_model_id,
        session_id = excluded.session_id,
        timestamp_ms = excluded.timestamp_ms,
        date = excluded.date,
        input_tokens = excluded.input_tokens,
        output_tokens = excluded.output_tokens,
        cache_read_tokens = excluded.cache_read_tokens,
        cache_write_tokens = excluded.cache_write_tokens,
        reasoning_tokens = excluded.reasoning_tokens,
        total_tokens = excluded.total_tokens,
        cost_usd = excluded.cost_usd,
        pricing_day = excluded.pricing_day,
        parser_version = excluded.parser_version,
        served_model = excluded.served_model,
        unpriced = excluded.unpriced`,
		message.Client, client, message.ProviderID, message.ModelID, canonical, message.SessionID, message.MessageKey,
		message.Timestamp, message.Date, message.Tokens.Input, message.Tokens.Output,
		message.Tokens.CacheRead, message.Tokens.CacheWrite, message.Tokens.Reasoning,
		message.TotalTokens(), price, message.PricingDay, message.ParserVersion, nullableString(message.ServedModel), unpriced)
	if err != nil {
		return false, err
	}
	return true, nil
}

func nullableString(value string) any {
	if value == "" {
		return nil
	}
	return value
}

// collapseCrossClientDuplicates keeps the widest observation of a message that
// Antigravity recorded under several runtimes.
func collapseCrossClientDuplicates(tx *sql.Tx, sources map[string]bool, affected map[string]bool) error {
	const survivors = `SELECT rowid FROM (
        SELECT rowid, ROW_NUMBER() OVER (
            PARTITION BY message_key
            ORDER BY total_tokens DESC, client ASC, rowid ASC
        ) AS rn
        FROM usage_messages WHERE source = ?1
    ) WHERE rn = 1`
	for source := range sources {
		if source != "antigravity" {
			continue
		}
		rows, err := tx.Query("SELECT DISTINCT date FROM usage_messages WHERE source = ?1 AND rowid NOT IN ("+survivors+")", source)
		if err != nil {
			return err
		}
		var dates []string
		for rows.Next() {
			var date string
			if err := rows.Scan(&date); err != nil {
				rows.Close()
				return err
			}
			dates = append(dates, date)
		}
		rows.Close()
		if len(dates) == 0 {
			continue
		}
		if _, err := tx.Exec("DELETE FROM usage_messages WHERE source = ?1 AND rowid NOT IN ("+survivors+")", source); err != nil {
			return err
		}
		for _, date := range dates {
			affected[date] = true
		}
	}
	return nil
}

func rebuildDaily(tx *sql.Tx, date string, now int64) error {
	if _, err := tx.Exec("DELETE FROM daily_model_usage WHERE date = ?1", date); err != nil {
		return err
	}
	_, err := tx.Exec(fmt.Sprintf(`INSERT INTO daily_model_usage (
        date, source, provider_id, model_id,
        input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
        reasoning_tokens, total_tokens, cost_usd, message_count, session_count, updated_at
    )
    SELECT date, source, provider_id, canonical_model_id,
        SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), SUM(cache_write_tokens),
        SUM(reasoning_tokens), SUM(total_tokens), SUM(cost_usd), COUNT(*),
        COUNT(DISTINCT session_id), ?2
    FROM (
        SELECT date, %s AS source, session_id, message_key,
            MAX(provider_id) AS provider_id, MAX(canonical_model_id) AS canonical_model_id,
            MAX(input_tokens) AS input_tokens, MAX(output_tokens) AS output_tokens,
            MAX(cache_read_tokens) AS cache_read_tokens, MAX(cache_write_tokens) AS cache_write_tokens,
            MAX(reasoning_tokens) AS reasoning_tokens, MAX(total_tokens) AS total_tokens,
            MAX(cost_usd) AS cost_usd
        FROM usage_messages WHERE date = ?1
        GROUP BY date, %s, session_id, message_key
    )
    GROUP BY date, source, provider_id, canonical_model_id`, canonicalSourceSQL, canonicalSourceSQL), date, now)
	return err
}

func ensureUsageSchema(db *sql.DB, path string) error {
	var tableCount int
	_ = db.QueryRow("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='usage_messages'").Scan(&tableCount)
	var userVersion int
	_ = db.QueryRow("PRAGMA user_version").Scan(&userVersion)

	if tableCount > 0 && userVersion < 7 {
		if err := backupDatabase(db, path, ".pre-0.7."); err != nil {
			return err
		}
	}
	if tableCount > 0 && userVersion < 1 {
		_, _ = db.Exec("DROP TABLE IF EXISTS usage_messages;")
		_, _ = db.Exec("DROP TABLE IF EXISTS daily_model_usage;")
		_, _ = db.Exec("DROP TABLE IF EXISTS daily_pricing_snapshots;")
		_, _ = db.Exec("PRAGMA user_version = 1;")
	}
	if _, err := db.Exec(usageSchemaSQL); err != nil {
		return err
	}
	if !columnPresent(db, "usage_messages", "served_model") {
		_, _ = db.Exec("ALTER TABLE usage_messages ADD COLUMN served_model TEXT;")
	}
	if !columnPresent(db, "usage_messages", "unpriced") {
		if _, err := db.Exec("ALTER TABLE usage_messages ADD COLUMN unpriced INTEGER NOT NULL DEFAULT 0;"); err != nil {
			return err
		}
		if _, err := db.Exec("UPDATE usage_messages SET unpriced = 1 WHERE cost_usd <= 0 AND total_tokens > 0;"); err != nil {
			return err
		}
	}
	if _, err := db.Exec("PRAGMA user_version = 7;"); err != nil {
		return err
	}
	if err := backfillServedModels(db, path); err != nil {
		return err
	}
	return refreshCanonicalModelIDs(db)
}

func columnPresent(db *sql.DB, table, column string) bool {
	var count int
	_ = db.QueryRow("SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2", table, column).Scan(&count)
	return count > 0
}

// backupDatabase writes a WAL-safe copy next to the database with mode 0600.
// An existing backup of the same instant is never overwritten.
func backupDatabase(db *sql.DB, path, marker string) error {
	stamp := time.Now().Format("20060102-150405")
	base := fmt.Sprintf("%s%s%s.bak", path, marker, stamp)
	target := base
	for i := 1; ; i++ {
		if _, err := os.Stat(target); errors.Is(err, os.ErrNotExist) {
			break
		}
		target = fmt.Sprintf("%s.%d", base, i)
	}
	quoted := strings.ReplaceAll(target, "'", "''")
	if _, err := db.Exec("VACUUM INTO '" + quoted + "'"); err != nil {
		return err
	}
	return os.Chmod(target, 0o600)
}

func backfillServedModels(db *sql.DB, usagePath string) error {
	cachePath := filepath.Join(filepath.Dir(usagePath), "antigravity-cache", "cache.db")
	if _, err := os.Stat(cachePath); err != nil {
		return nil
	}
	var pending int
	_ = db.QueryRow(`SELECT COUNT(*) FROM usage_messages WHERE source IN ('antigravity', 'antigravity-cli', 'antigravity-desktop', 'antigravity-ide') AND (served_model IS NULL OR served_model = '') LIMIT 1`).Scan(&pending)
	if pending == 0 {
		return nil
	}
	escaped := strings.ReplaceAll(cachePath, "'", "''")
	if _, err := db.Exec("ATTACH DATABASE '" + escaped + "' AS ag_cache;"); err != nil {
		return nil
	}
	defer db.Exec("DETACH DATABASE ag_cache;")
	_, err := db.Exec(`UPDATE usage_messages
SET served_model = (
    SELECT su.served_model FROM ag_cache.session_usage su
    WHERE su.session_id = usage_messages.session_id AND su.timestamp = usage_messages.timestamp_ms LIMIT 1
)
WHERE source IN ('antigravity', 'antigravity-cli', 'antigravity-desktop', 'antigravity-ide')
  AND (served_model IS NULL OR served_model = '')`)
	return err
}

// refreshCanonicalModelIDs re-derives the stored canonical id once per open.
func refreshCanonicalModelIDs(db *sql.DB) error {
	rows, err := db.Query("SELECT DISTINCT model_id, canonical_model_id FROM usage_messages")
	if err != nil {
		return err
	}
	type stale struct{ model, canonical string }
	var changes []stale
	for rows.Next() {
		var model, stored string
		if err := rows.Scan(&model, &stored); err != nil {
			continue
		}
		if canonical := modelid.Canonical(model); canonical != stored {
			changes = append(changes, stale{model, canonical})
		}
	}
	rows.Close()
	if len(changes) == 0 {
		return nil
	}
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	affected := map[string]bool{}
	for _, change := range changes {
		if _, err := tx.Exec("UPDATE usage_messages SET canonical_model_id = ?1 WHERE model_id = ?2", change.canonical, change.model); err != nil {
			return err
		}
		dates, err := tx.Query("SELECT DISTINCT date FROM usage_messages WHERE model_id = ?1", change.model)
		if err != nil {
			return err
		}
		for dates.Next() {
			var date string
			if dates.Scan(&date) == nil {
				affected[date] = true
			}
		}
		dates.Close()
	}
	now := time.Now().UnixMilli()
	for date := range affected {
		if err := rebuildDaily(tx, date, now); err != nil {
			return err
		}
	}
	return tx.Commit()
}

const usageSchemaSQL = `
CREATE TABLE IF NOT EXISTS usage_messages (
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
    served_model TEXT,
    unpriced INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (source, client, message_key)
);
CREATE INDEX IF NOT EXISTS idx_usage_messages_date ON usage_messages(date);
CREATE INDEX IF NOT EXISTS idx_usage_messages_source_date ON usage_messages(source, date);
CREATE INDEX IF NOT EXISTS idx_usage_messages_source_date_canonical ON usage_messages(source, date, canonical_model_id);
CREATE INDEX IF NOT EXISTS idx_usage_messages_source_parser_version ON usage_messages(source, parser_version);
CREATE INDEX IF NOT EXISTS idx_usage_messages_session ON usage_messages(source, client, session_id);
CREATE INDEX IF NOT EXISTS idx_usage_messages_zero_cost ON usage_messages(date, source) WHERE cost_usd <= 0 AND total_tokens > 0;

CREATE TABLE IF NOT EXISTS daily_model_usage (
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
CREATE INDEX IF NOT EXISTS idx_daily_model_usage_date ON daily_model_usage(date);

CREATE TABLE IF NOT EXISTS source_refresh_state (
    source TEXT PRIMARY KEY,
    last_refresh_started_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS daily_pricing_snapshots (
    date TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    input_cost_per_token REAL NOT NULL,
    output_cost_per_token REAL NOT NULL,
    cache_read_input_token_cost REAL,
    cache_creation_input_token_cost REAL,
    captured_at INTEGER NOT NULL,
    pricing_source TEXT NOT NULL,
    pricing_version TEXT NOT NULL,
    PRIMARY KEY (date, provider_id, model_id)
);

CREATE TABLE IF NOT EXISTS model_price_periods (
    model TEXT NOT NULL,
    effective_from TEXT NOT NULL,
    effective_to TEXT,
    input_rate REAL NOT NULL,
    output_rate REAL NOT NULL,
    cache_read_rate REAL NOT NULL,
    cache_write_rate REAL NOT NULL,
    tier TEXT,
    source TEXT,
    PRIMARY KEY (model, effective_from)
);
CREATE INDEX IF NOT EXISTS idx_model_price_periods_model ON model_price_periods(model);

CREATE TABLE IF NOT EXISTS model_releases (
    model TEXT PRIMARY KEY,
    release_date TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS app_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
`

// DailyUsageRow is one daily rollup row (CSV output).
type DailyUsageRow struct {
	Date, Source, ProviderID, ModelID                                             string
	InputTokens, OutputTokens, CacheReadTokens, CacheWriteTokens, ReasoningTokens int64
	TotalTokens, MessageCount, SessionCount                                       int64
	CostUSD                                                                       float64
}

func (s *Store) LoadDailyRows(from *time.Time, to *time.Time, sources []string) ([]DailyUsageRow, error) {
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	sqlText := "SELECT date, " + canonicalSourceSQL + " AS source, provider_id, model_id, SUM(input_tokens), SUM(output_tokens), " +
		"SUM(cache_read_tokens), SUM(cache_write_tokens), SUM(reasoning_tokens), SUM(total_tokens), SUM(cost_usd), " +
		"SUM(message_count), SUM(session_count) FROM daily_model_usage WHERE 1=1"
	args := []any{}
	sqlText, args = appendRangeAndSources(sqlText, args, from, to, sources)
	sqlText += " GROUP BY date, " + canonicalSourceSQL + ", provider_id, model_id ORDER BY date ASC, SUM(cost_usd) DESC"
	rows, err := db.Query(sqlText, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []DailyUsageRow
	for rows.Next() {
		var row DailyUsageRow
		if err := rows.Scan(&row.Date, &row.Source, &row.ProviderID, &row.ModelID, &row.InputTokens, &row.OutputTokens,
			&row.CacheReadTokens, &row.CacheWriteTokens, &row.ReasoningTokens, &row.TotalTokens, &row.CostUSD,
			&row.MessageCount, &row.SessionCount); err != nil {
			return nil, err
		}
		out = append(out, row)
	}
	return out, rows.Err()
}

func appendSources(sqlText string, args []any, sources []string) (string, []any) {
	if len(sources) == 0 {
		return sqlText, args
	}
	sqlText += " AND source IN ("
	var placeholders []string
	for _, source := range sources {
		if source == "antigravity" {
			placeholders = append(placeholders, "?", "?", "?", "?")
			args = append(args, "antigravity", "antigravity-cli", "antigravity-desktop", "antigravity-ide")
		} else {
			placeholders = append(placeholders, "?")
			args = append(args, source)
		}
	}
	sqlText += strings.Join(placeholders, ", ") + ")"
	return sqlText, args
}

func appendRangeAndSources(sqlText string, args []any, from, to *time.Time, sources []string) (string, []any) {
	if from != nil {
		sqlText += " AND date >= ?"
		args = append(args, from.Format("2006-01-02"))
	}
	if to != nil {
		sqlText += " AND date <= ?"
		args = append(args, to.Format("2006-01-02"))
	}
	return appendSources(sqlText, args, sources)
}

// RawModelRow is one raw model variant on a date.
type RawModelRow struct {
	Source, RawModelID, TierModelID, CanonicalModelID                                          string
	InputTokens, OutputTokens, CacheReadTokens, CacheWriteTokens, ReasoningTokens, TotalTokens int64
	CostUSD                                                                                    float64
	MessageCount, SessionCount                                                                 int64
}

func (s *Store) LoadRawModelsForDate(date string) ([]RawModelRow, error) {
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	rows, err := db.Query(`SELECT `+canonicalSourceSQL+` AS source,
        COALESCE(NULLIF(served_model, ''), model_id) AS raw_model_id, model_id AS tier_model_id,
        MAX(canonical_model_id), SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
        SUM(cache_write_tokens), SUM(reasoning_tokens), SUM(total_tokens), SUM(cost_usd), COUNT(*), COUNT(DISTINCT session_id)
        FROM usage_messages WHERE date = ?1
        GROUP BY `+canonicalSourceSQL+`, COALESCE(NULLIF(served_model, ''), model_id), model_id
        ORDER BY total_tokens DESC, cost_usd DESC`, date)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []RawModelRow
	for rows.Next() {
		var row RawModelRow
		if err := rows.Scan(&row.Source, &row.RawModelID, &row.TierModelID, &row.CanonicalModelID,
			&row.InputTokens, &row.OutputTokens, &row.CacheReadTokens, &row.CacheWriteTokens,
			&row.ReasoningTokens, &row.TotalTokens, &row.CostUSD, &row.MessageCount, &row.SessionCount); err != nil {
			return nil, err
		}
		out = append(out, row)
	}
	return out, rows.Err()
}

// Rows with a usage rollup for the range, newest first.
func (s *Store) LoadDailyRowsRaw() ([]string, error) {
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	rows, err := db.Query("SELECT DISTINCT date FROM usage_messages ORDER BY date ASC")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var dates []string
	for rows.Next() {
		var date string
		if rows.Scan(&date) == nil {
			dates = append(dates, date)
		}
	}
	return dates, rows.Err()
}

// LoadSummaryCounts counts de-duplicated messages and sessions.
func (s *Store) LoadSummaryCounts(from, to *time.Time, sources []string) (int, int, error) {
	db, err := s.Open()
	if err != nil {
		return 0, 0, err
	}
	defer db.Close()
	query := "SELECT COUNT(*), COUNT(DISTINCT source || '::' || session_id) FROM (" +
		"SELECT date, " + canonicalSourceSQL + " AS source, session_id, message_key FROM usage_messages WHERE 1=1"
	args := []any{}
	query, args = appendRangeAndSources(query, args, from, to, sources)
	query += " GROUP BY date, " + canonicalSourceSQL + ", session_id, message_key)"
	var messages, sessions int
	err = db.QueryRow(query, args...).Scan(&messages, &sessions)
	return messages, sessions, err
}

// LoadDashboardDays returns one row per active date.
func (s *Store) LoadDashboardDays(from, to *time.Time, sources []string) ([]DailyUsageRow, error) {
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	dailyQuery := `SELECT date, SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
        SUM(cache_write_tokens), SUM(reasoning_tokens), SUM(total_tokens), SUM(cost_usd), SUM(message_count)
        FROM daily_model_usage WHERE 1=1`
	dailyArgs := []any{}
	dailyQuery, dailyArgs = appendRangeAndSources(dailyQuery, dailyArgs, from, to, sources)
	dailyQuery += " GROUP BY date ORDER BY date ASC"
	messageQuery := `SELECT date, COUNT(DISTINCT ` + canonicalSourceSQL + ` || '::' || session_id)
        FROM usage_messages WHERE 1=1`
	messageArgs := []any{}
	messageQuery, messageArgs = appendRangeAndSources(messageQuery, messageArgs, from, to, sources)
	messageQuery += " GROUP BY date"
	sessionCounts := map[string]int64{}
	rows, err := db.Query(messageQuery, messageArgs...)
	if err != nil {
		return nil, err
	}
	for rows.Next() {
		var date string
		var count int64
		if rows.Scan(&date, &count) == nil {
			sessionCounts[date] = count
		}
	}
	rows.Close()
	rows, err = db.Query(dailyQuery, dailyArgs...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []DailyUsageRow
	for rows.Next() {
		var row DailyUsageRow
		if err := rows.Scan(&row.Date, &row.InputTokens, &row.OutputTokens, &row.CacheReadTokens,
			&row.CacheWriteTokens, &row.ReasoningTokens, &row.TotalTokens, &row.CostUSD, &row.MessageCount); err != nil {
			return nil, err
		}
		row.SessionCount = sessionCounts[row.Date]
		out = append(out, row)
	}
	return out, rows.Err()
}

// LoadProviderSummaries sums one row per canonical source.
func (s *Store) LoadProviderSummaries(from, to *time.Time, sources []string) ([]struct {
	Provider     string
	Cost         float64
	Tokens       int64
	MessageCount int64
	SessionCount int64
}, error) {
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	dailyQuery := `SELECT source, SUM(cost_usd), SUM(total_tokens), SUM(message_count)
        FROM daily_model_usage WHERE 1=1`
	dailyArgs := []any{}
	dailyQuery, dailyArgs = appendRangeAndSources(dailyQuery, dailyArgs, from, to, sources)
	dailyQuery += " GROUP BY source"
	messageQuery := `SELECT ` + canonicalSourceSQL + ` AS source, COUNT(DISTINCT session_id)
        FROM usage_messages WHERE 1=1`
	messageArgs := []any{}
	messageQuery, messageArgs = appendRangeAndSources(messageQuery, messageArgs, from, to, sources)
	messageQuery += " GROUP BY 1"
	sessions := map[string]int64{}
	rows, err := db.Query(messageQuery, messageArgs...)
	if err != nil {
		return nil, err
	}
	for rows.Next() {
		var source string
		var count int64
		if rows.Scan(&source, &count) == nil {
			sessions[source] = count
		}
	}
	rows.Close()
	rows, err = db.Query(dailyQuery, dailyArgs...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []struct {
		Provider     string
		Cost         float64
		Tokens       int64
		MessageCount int64
		SessionCount int64
	}
	for rows.Next() {
		var row struct {
			Provider     string
			Cost         float64
			Tokens       int64
			MessageCount int64
			SessionCount int64
		}
		if err := rows.Scan(&row.Provider, &row.Cost, &row.Tokens, &row.MessageCount); err != nil {
			return nil, err
		}
		row.SessionCount = sessions[row.Provider]
		out = append(out, row)
	}
	return out, rows.Err()
}

// LoadModelSummaries groups by canonical model, aggregating providers, sources
// and sessions across the range.
func (s *Store) LoadModelSummaries(from, to *time.Time, sources []string) ([]struct {
	Model                                        string
	Providers, Sources                           map[string]bool
	Sessions                                     map[string]bool
	Cost                                         float64
	Tokens, Input, Output, CacheRead, CacheWrite int64
	MessageCount                                 int
}, error) {
	db, err := s.Open()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	// Antigravity runtimes can record the same message key more than once.
	// Rank within the canonical source and keep one row, like the ingest-time
	// cross-client collapse does; other sources keep every client row.
	query := `WITH filtered AS (
        SELECT rowid, canonical_model_id, provider_id, session_id, cost_usd, total_tokens,
            input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, client, message_key,
            ` + canonicalSourceSQL + ` AS canonical_source
        FROM usage_messages WHERE 1=1`
	args := []any{}
	query, args = appendRangeAndSources(query, args, from, to, sources)
	query += `), ranked AS (
        SELECT filtered.*, ROW_NUMBER() OVER (
            PARTITION BY canonical_source, message_key
            ORDER BY total_tokens DESC, client ASC, rowid ASC
        ) AS rn
        FROM filtered
    ), deduped AS (
        SELECT * FROM ranked WHERE canonical_source <> 'antigravity' OR rn = 1
    )
    SELECT canonical_model_id, provider_id, canonical_source, session_id,
        SUM(cost_usd), SUM(total_tokens), COUNT(*), SUM(input_tokens), SUM(output_tokens),
        SUM(cache_read_tokens), SUM(cache_write_tokens)
    FROM deduped GROUP BY canonical_model_id, provider_id, canonical_source, session_id`
	rows, err := db.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	type aggregate struct {
		Model                                        string
		Providers, Sources                           map[string]bool
		Sessions                                     map[string]bool
		Cost                                         float64
		Tokens, Input, Output, CacheRead, CacheWrite int64
		MessageCount                                 int
	}
	grouped := map[string]*aggregate{}
	for rows.Next() {
		var model, providerID, source, sessionID string
		var cost float64
		var tokens, messages, input, output, cacheRead, cacheWrite int64
		if err := rows.Scan(&model, &providerID, &source, &sessionID, &cost, &tokens, &messages,
			&input, &output, &cacheRead, &cacheWrite); err != nil {
			return nil, err
		}
		entry, ok := grouped[model]
		if !ok {
			entry = &aggregate{Model: model, Providers: map[string]bool{}, Sources: map[string]bool{}, Sessions: map[string]bool{}}
			grouped[model] = entry
		}
		entry.Providers[providerID] = true
		entry.Sources[source] = true
		entry.Sessions[sessionID] = true
		entry.Cost += cost
		entry.Tokens += tokens
		entry.MessageCount += int(messages)
		entry.Input += input
		entry.Output += output
		entry.CacheRead += cacheRead
		entry.CacheWrite += cacheWrite
	}
	keys := make([]string, 0, len(grouped))
	for key := range grouped {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	out := make([]struct {
		Model                                        string
		Providers, Sources                           map[string]bool
		Sessions                                     map[string]bool
		Cost                                         float64
		Tokens, Input, Output, CacheRead, CacheWrite int64
		MessageCount                                 int
	}, 0, len(keys))
	for _, key := range keys {
		entry := grouped[key]
		if entry.Tokens <= 0 {
			continue
		}
		out = append(out, *entry)
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Tokens > out[j].Tokens })
	return out, nil
}

// CountDistinctSessionsForDate counts distinct sessions on one date.
func (s *Store) CountDistinctSessionsForDate(date string) (int64, error) {
	db, err := s.Open()
	if err != nil {
		return 0, err
	}
	defer db.Close()
	var count int64
	err = db.QueryRow("SELECT COUNT(DISTINCT "+canonicalSourceSQL+" || '::' || session_id) FROM usage_messages WHERE date = ?1", date).Scan(&count)
	return count, err
}

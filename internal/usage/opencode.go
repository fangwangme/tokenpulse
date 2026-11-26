package usage

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	_ "modernc.org/sqlite"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

const opencodeParserVersion = "opencode-v3"

var opencodeTokenFields = []string{"input", "output", "reasoning", "cache.read", "cache.write"}

// OpenCodeParser reads OpenCode's SQLite databases, 1.x and 2.x layouts.
type OpenCodeParser struct{}

func NewOpenCodeParser() *OpenCodeParser { return &OpenCodeParser{} }

func (p *OpenCodeParser) Name() string                     { return "opencode" }
func (p *OpenCodeParser) ParserVersion() string            { return opencodeParserVersion }
func (p *OpenCodeParser) IncrementalMode() IncrementalMode { return UpsertMessages }

func (p *OpenCodeParser) SessionPaths() []string {
	dataDir := opencodeDataDir()
	return discoverOpenCodeDatabases(dataDir, os.Getenv("OPENCODE_DB"))
}

func opencodeDataDir() string {
	if xdg := os.Getenv("XDG_DATA_HOME"); xdg != "" {
		return filepath.Join(xdg, "opencode")
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".local", "share", "opencode")
}

func discoverOpenCodeDatabases(dataDir, override string) []string {
	if override != "" {
		path := override
		if !filepath.IsAbs(path) {
			path = filepath.Join(dataDir, override)
		}
		if info, err := os.Stat(path); err == nil && !info.IsDir() {
			return []string{path}
		}
		return nil
	}
	entries, _ := os.ReadDir(dataDir)
	var channels []string
	for _, entry := range entries {
		name := entry.Name()
		if entry.IsDir() || !strings.HasPrefix(name, "opencode-") || !strings.HasSuffix(name, ".db") {
			continue
		}
		channels = append(channels, filepath.Join(dataDir, name))
	}
	sort.Strings(channels)
	var paths []string
	def := filepath.Join(dataDir, "opencode.db")
	if info, err := os.Stat(def); err == nil && !info.IsDir() {
		paths = append(paths, def)
	}
	return append(paths, channels...)
}

func (p *OpenCodeParser) Parse(since *time.Time) ([]provider.UnifiedMessage, error) {
	paths := p.SessionPaths()
	var sinceMs *int64
	if since != nil {
		start := time.Date(since.Year(), since.Month(), since.Day(), 0, 0, 0, 0, time.Local)
		ms := start.UnixMilli()
		sinceMs = &ms
	}
	seen := map[string]bool{}
	var messages []provider.UnifiedMessage
	for _, path := range paths {
		rows, err := loadOpenCodeRows(path, sinceMs)
		if err != nil {
			return nil, fmt.Errorf("failed to read OpenCode database %s: %w", path, err)
		}
		for _, row := range rows {
			if seen[row.id] {
				continue
			}
			seen[row.id] = true
			messages = append(messages, row.message())
		}
	}
	if since != nil {
		filtered := messages[:0]
		for _, message := range messages {
			if message.Date >= since.Format("2006-01-02") {
				filtered = append(filtered, message)
			}
		}
		messages = filtered
	}
	sort.SliceStable(messages, func(i, j int) bool { return messages[i].Timestamp < messages[j].Timestamp })
	return messages, nil
}

type openCodeRow struct {
	id              string
	sessionID       string
	timeCreated     *int64
	modelID         string
	providerID      string
	tokens          provider.TokenBreakdown
	dataTimeCreated *float64
	seq             int64
}

func (r openCodeRow) message() provider.UnifiedMessage {
	var timestamp int64
	switch {
	case r.timeCreated != nil:
		timestamp = *r.timeCreated
	case r.dataTimeCreated != nil:
		timestamp = epochMillis(*r.dataTimeCreated)
	default:
		timestamp = time.Now().UnixMilli()
	}
	modelID := r.modelID
	if modelID == "" {
		modelID = "unknown"
	}
	providerID := r.providerID
	if providerID == "" {
		providerID = "unknown"
	}
	msg := provider.NewMessage("opencode", modelID, providerID, r.sessionID, r.id, timestamp, r.tokens)
	msg.ParserVersion = opencodeParserVersion
	return msg
}

func epochMillis(timestamp float64) int64 {
	if timestamp > 10_000_000_000.0 {
		return int64(timestamp)
	}
	return int64(timestamp * 1000.0)
}

func loadOpenCodeRows(path string, sinceMs *int64) ([]openCodeRow, error) {
	dsn := "file:" + path + "?mode=ro&_pragma=busy_timeout(5000)"
	conn, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	if err := conn.Ping(); err != nil {
		return nil, err
	}
	tx, err := conn.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()

	hasV2, err := tableExists(tx, "session_message")
	if err != nil {
		return nil, err
	}
	var rows []openCodeRow
	if hasV2 {
		v2, err := loadV2Rows(tx, sinceMs)
		if err != nil {
			return nil, err
		}
		rows = append(rows, v2...)
	}
	hasV1, err := tableExists(tx, "message")
	if err != nil {
		return nil, err
	}
	if hasV1 {
		v1, err := loadV1Rows(tx, sinceMs, hasV2)
		if err != nil {
			return nil, err
		}
		rows = append(rows, v1...)
	}
	return rows, nil
}

func loadV2Rows(tx *sql.Tx, sinceMs *int64) ([]openCodeRow, error) {
	hasSessions, err := tableExists(tx, "session_v2")
	if err != nil {
		return nil, err
	}
	cutoffs := map[string]int64{}
	if hasSessions {
		cutoffs, err = forkCopyCutoffs(tx)
		if err != nil {
			return nil, err
		}
	}
	query := v2Query(sinceMs != nil, hasSessions)
	var args []any
	if sinceMs != nil {
		args = append(args, *sinceMs)
	}
	sqlRows, err := tx.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer sqlRows.Close()
	var rows []openCodeRow
	for sqlRows.Next() {
		row, err := scanOpenCodeRow(sqlRows, true)
		if err != nil {
			return nil, err
		}
		if cutoff, ok := cutoffs[row.sessionID]; ok && row.seq <= cutoff {
			continue
		}
		rows = append(rows, row)
	}
	return rows, sqlRows.Err()
}

func v2Query(windowed, hasSessions bool) string {
	sessionModel, sessionProvider, join := "NULL", "NULL", ""
	if hasSessions {
		sessionModel = "CASE WHEN json_valid(s.model) THEN " + textAt("s.model", "$.id") + " END"
		sessionProvider = "CASE WHEN json_valid(s.model) THEN " + textAt("s.model", "$.providerID") + " END"
		join = "\nLEFT JOIN session_v2 s ON s.id = sm.session_id"
	}
	window := ""
	if windowed {
		window = "\n  AND sm.time_created >= ?1"
	}
	return fmt.Sprintf(
		`SELECT sm.id, sm.session_id, sm.time_created,
       COALESCE(%s, %s),
       COALESCE(%s, %s),
       %s,
       %s,
       sm.seq
FROM session_message sm%s
WHERE sm.type IN ('assistant', 'compaction')
  AND json_valid(sm.data)
  AND json_type(sm.data, '$.tokens') = 'object'%s`,
		textAt("sm.data", "$.model.id"), sessionModel,
		textAt("sm.data", "$.model.providerID"), sessionProvider,
		tokenColumns("sm.data"), dataTimeColumn("sm.data"), join, window)
}

func loadV1Rows(tx *sql.Tx, sinceMs *int64, hasV2 bool) ([]openCodeRow, error) {
	hasTime, err := columnExists(tx, "message", "time_created")
	if err != nil {
		return nil, err
	}
	windowed := sinceMs != nil && hasTime
	query := v1Query(windowed, hasTime, hasV2)
	var args []any
	if windowed {
		args = append(args, *sinceMs)
	}
	sqlRows, err := tx.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer sqlRows.Close()
	var rows []openCodeRow
	for sqlRows.Next() {
		row, err := scanOpenCodeRow(sqlRows, false)
		if err != nil {
			return nil, err
		}
		rows = append(rows, row)
	}
	return rows, sqlRows.Err()
}

func v1Query(windowed, hasTime, hasV2 bool) string {
	timeCreated := "NULL"
	if hasTime {
		timeCreated = "m.time_created"
	}
	window := ""
	if windowed {
		window = "m.rowid IN (SELECT rowid FROM message WHERE time_created >= ?1)\n  AND "
	}
	dedup := ""
	if hasV2 {
		dedup = "\n  AND NOT EXISTS (SELECT 1 FROM session_message s WHERE s.id = m.id)"
	}
	return fmt.Sprintf(
		`SELECT m.id, m.session_id, %s,
       %s,
       %s,
       %s,
       %s
FROM message m
WHERE %sjson_valid(m.data)
  AND json_extract(m.data, '$.role') = 'assistant'
  AND json_type(m.data, '$.tokens') = 'object'%s`,
		timeCreated,
		textAt("m.data", "$.modelID", "$.model"),
		textAt("m.data", "$.providerID", "$.provider"),
		tokenColumns("m.data"), dataTimeColumn("m.data"), window, dedup)
}

func textAt(data string, paths ...string) string {
	var candidates []string
	for _, path := range paths {
		candidates = append(candidates, fmt.Sprintf("CASE json_type(%s, '%s') WHEN 'text' THEN json_extract(%s, '%s') END", data, path, data, path))
	}
	if len(candidates) == 1 {
		return candidates[0]
	}
	return "COALESCE(" + strings.Join(candidates, ", ") + ")"
}

func tokenColumns(data string) string {
	parts := make([]string, 0, len(opencodeTokenFields))
	for _, field := range opencodeTokenFields {
		parts = append(parts, fmt.Sprintf("CAST(json_extract(%s, '$.tokens.%s') AS INTEGER)", data, field))
	}
	return strings.Join(parts, ",\n       ")
}

func dataTimeColumn(data string) string {
	return fmt.Sprintf("COALESCE(json_extract(%s, '$.time.created'), json_extract(%s, '$.time.completed'))", data, data)
}

func scanOpenCodeRow(rows *sql.Rows, hasSeq bool) (openCodeRow, error) {
	var row openCodeRow
	var timeCreated, dataTime sql.NullFloat64
	var modelID, providerID sql.NullString
	var input, output, reasoning, cacheRead, cacheWrite sql.NullInt64
	var seq sql.NullInt64
	dest := []any{&row.id, &row.sessionID, &timeCreated, &modelID, &providerID,
		&input, &output, &reasoning, &cacheRead, &cacheWrite, &dataTime}
	if hasSeq {
		dest = append(dest, &seq)
	}
	if err := rows.Scan(dest...); err != nil {
		return row, nil
	}
	if timeCreated.Valid {
		v := int64(timeCreated.Float64)
		row.timeCreated = &v
	}
	if dataTime.Valid {
		v := dataTime.Float64
		row.dataTimeCreated = &v
	}
	row.modelID = modelID.String
	row.providerID = providerID.String
	row.tokens = provider.TokenBreakdown{
		Input: max64(input.Int64, 0), Output: max64(output.Int64, 0), Reasoning: max64(reasoning.Int64, 0),
		CacheRead: max64(cacheRead.Int64, 0), CacheWrite: max64(cacheWrite.Int64, 0),
	}
	row.seq = seq.Int64
	return row, nil
}

func tableExists(tx *sql.Tx, table string) (bool, error) {
	var one int
	err := tx.QueryRow("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", table).Scan(&one)
	if err == sql.ErrNoRows {
		return false, nil
	}
	return err == nil, err
}

func columnExists(tx *sql.Tx, table, column string) (bool, error) {
	rows, err := tx.Query("SELECT name FROM pragma_table_info(?1)", table)
	if err != nil {
		return false, err
	}
	defer rows.Close()
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			return false, err
		}
		if name == column {
			return true, nil
		}
	}
	return false, rows.Err()
}

type forkBoundary struct {
	Type      string `json:"type"`
	MessageID string `json:"messageID"`
}

func forkCopyCutoffs(tx *sql.Tx) (map[string]int64, error) {
	hasParent, err := columnExists(tx, "session_v2", "fork_session_id")
	if err != nil || !hasParent {
		return map[string]int64{}, err
	}
	hasBoundary, err := columnExists(tx, "session_v2", "fork_boundary")
	if err != nil || !hasBoundary {
		return map[string]int64{}, err
	}
	rows, err := tx.Query("SELECT id, fork_session_id, fork_boundary FROM session_v2 WHERE fork_session_id IS NOT NULL AND fork_boundary IS NOT NULL")
	if err != nil {
		return nil, err
	}
	type fork struct{ id, parent, boundary string }
	var forks []fork
	for rows.Next() {
		var f fork
		if err := rows.Scan(&f.id, &f.parent, &f.boundary); err != nil {
			rows.Close()
			return nil, err
		}
		forks = append(forks, f)
	}
	rows.Close()
	cutoffs := map[string]int64{}
	for _, f := range forks {
		var boundary forkBoundary
		if err := json.Unmarshal([]byte(f.boundary), &boundary); err != nil {
			continue
		}
		var seq int64
		if err := tx.QueryRow("SELECT seq FROM session_message WHERE id = ?1 AND session_id = ?2", boundary.MessageID, f.parent).Scan(&seq); err != nil {
			continue
		}
		var cutoff sql.NullInt64
		switch boundary.Type {
		case "through":
			cutoff = sql.NullInt64{Int64: seq, Valid: true}
		case "before":
			_ = tx.QueryRow("SELECT MAX(seq) FROM session_message WHERE session_id = ?1 AND seq < ?2", f.parent, seq).Scan(&cutoff)
		}
		if cutoff.Valid {
			cutoffs[f.id] = cutoff.Int64
		}
	}
	return cutoffs, nil
}

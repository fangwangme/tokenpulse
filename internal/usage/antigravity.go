package usage

import (
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/modelid"
	"github.com/fangwangme/tokenpulse/internal/provider"
)

const antigravityParserVersion = "antigravity-v9"

// AntigravityParser reads TokenPulse's Antigravity cache, syncing it from the
// local conversation databases and (when running) the language servers.
type AntigravityParser struct {
	CacheRoot    string
	Releases     ReleaseMap
	RebuildCache bool
	SkipSync     bool
	// SkipLanguageServers keeps fixtures and tests away from this machine's
	// real Antigravity processes; local conversation databases still sync.
	SkipLanguageServers bool
	SyncCursorMs        *int64
	DebugLogger         *log.Logger
}

func NewAntigravityParser(cacheRoot string, releases ReleaseMap) *AntigravityParser {
	return &AntigravityParser{CacheRoot: cacheRoot, Releases: releases}
}

func (p *AntigravityParser) Name() string                     { return "antigravity" }
func (p *AntigravityParser) ParserVersion() string            { return antigravityParserVersion }
func (p *AntigravityParser) IncrementalMode() IncrementalMode { return ReplaceChangedSessions }

func (p *AntigravityParser) SessionPaths() []string { return []string{p.CacheRoot} }

type antigravityRow struct {
	client      string
	modelID     string
	providerID  string
	sessionID   string
	messageKey  string
	timestamp   int64
	tokens      provider.TokenBreakdown
	pricingDay  string
	servedModel string
	modelEnum   string
	displayName string
}

func (p *AntigravityParser) SetSyncCursor(ms *int64)      { p.SyncCursorMs = ms }
func (p *AntigravityParser) SetDebugLogger(l *log.Logger) { p.DebugLogger = l }

func (p *AntigravityParser) Parse(since *time.Time) ([]provider.UnifiedMessage, error) {
	if !p.SkipSync {
		if err := p.sync(since); err != nil {
			_ = err
		}
	}
	aliases := loadAliasHistory(p.CacheRoot)
	if err := p.normalizeCache(aliases, since); err != nil {
		_ = err
	}

	conn, err := openAntigravityCache(p.CacheRoot)
	if err != nil {
		return nil, err
	}
	defer conn.Close()

	const cols = `client, model_id, provider_id, session_id, COALESCE(response_id, id), timestamp,
       input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
       pricing_day, COALESCE(served_model, ''), COALESCE(model_enum, ''), COALESCE(display_name, '')`
	query := "SELECT " + cols + " FROM session_usage ORDER BY timestamp ASC"
	var args []any
	if since != nil {
		sinceMs := midnightMillis(*since, time.Local)
		syncedCursor := sinceMs
		if p.SyncCursorMs != nil {
			syncedCursor = *p.SyncCursorMs
		}
		query = `SELECT ` + cols + ` FROM session_usage WHERE timestamp >= ?1
UNION
SELECT ` + cols + ` FROM session_usage WHERE session_id IN (SELECT session_id FROM sessions WHERE synced_at >= ?2)
ORDER BY timestamp ASC`
		args = append(args, sinceMs, syncedCursor)
	}
	rows, err := conn.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	unique := map[string]provider.UnifiedMessage{}
	for rows.Next() {
		var row antigravityRow
		if err := rows.Scan(&row.client, &row.modelID, &row.providerID, &row.sessionID, &row.messageKey,
			&row.timestamp, &row.tokens.Input, &row.tokens.Output, &row.tokens.CacheRead,
			&row.tokens.CacheWrite, &row.tokens.Reasoning, &row.pricingDay, &row.servedModel, &row.modelEnum,
			&row.displayName); err != nil {
			continue
		}
		msg := provider.NewMessage("antigravity", row.modelID, row.providerID, row.sessionID, row.messageKey, row.timestamp, row.tokens)
		msg.ClientDetail = row.client
		msg.PricingDay = row.pricingDay
		msg.ServedModel = strings.TrimSpace(row.servedModel)
		msg.ParserVersion = antigravityParserVersion
		key := row.sessionID + "\x00" + row.messageKey
		if existing, ok := unique[key]; ok {
			if shouldReplaceAntigravity(existing, msg) {
				unique[key] = msg
			}
		} else {
			unique[key] = msg
		}
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	messages := make([]provider.UnifiedMessage, 0, len(unique))
	for _, message := range unique {
		messages = append(messages, message)
	}
	sort.SliceStable(messages, func(i, j int) bool {
		if messages[i].Timestamp != messages[j].Timestamp {
			return messages[i].Timestamp < messages[j].Timestamp
		}
		if messages[i].SessionID != messages[j].SessionID {
			return messages[i].SessionID < messages[j].SessionID
		}
		return messages[i].MessageKey < messages[j].MessageKey
	})
	return messages, nil
}

func antigravityClientPriority(client string) int {
	switch client {
	case "antigravity-desktop":
		return 1
	case "antigravity-ide":
		return 2
	case "antigravity-cli":
		return 3
	default:
		return 4
	}
}

func shouldReplaceAntigravity(existing, candidate provider.UnifiedMessage) bool {
	if candidate.TotalTokens() != existing.TotalTokens() {
		return candidate.TotalTokens() > existing.TotalTokens()
	}
	ep, cp := antigravityClientPriority(existing.ClientDetail), antigravityClientPriority(candidate.ClientDetail)
	if cp != ep {
		return cp < ep
	}
	return candidate.Timestamp > existing.Timestamp
}

func openAntigravityCache(cacheRoot string) (*sql.DB, error) {
	if err := os.MkdirAll(cacheRoot, 0o755); err != nil {
		return nil, err
	}
	db, err := sql.Open("sqlite", "file:"+filepath.Join(cacheRoot, "cache.db")+"?_pragma=busy_timeout(5000)&_pragma=foreign_keys(ON)")
	if err != nil {
		return nil, err
	}
	if err := createAntigravitySchema(db); err != nil {
		db.Close()
		return nil, err
	}
	return db, nil
}

func createAntigravitySchema(db *sql.DB) error {
	var pkCount int
	_ = db.QueryRow("SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE pk > 0").Scan(&pkCount)
	if pkCount > 0 && pkCount != 2 {
		_, _ = db.Exec("DROP TABLE IF EXISTS session_usage;")
		_, _ = db.Exec("DROP TABLE IF EXISTS sessions;")
	}
	_, err := db.Exec(`
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT NOT NULL,
    trajectory_id TEXT,
    client TEXT NOT NULL,
    title TEXT,
    model_id TEXT NOT NULL,
    status TEXT,
    step_count INTEGER,
    created_time_ms INTEGER,
    last_modified_ms INTEGER,
    last_user_input_time_ms INTEGER,
    project_id TEXT,
    workspace_path TEXT,
    git_root TEXT,
    repository TEXT,
    git_origin_url TEXT,
    branch_name TEXT,
    parent_conversation_id TEXT,
    mendel_experiment_ids TEXT,
    synced_at INTEGER NOT NULL,
    parser_version TEXT,
    PRIMARY KEY (session_id, client)
);
CREATE TABLE IF NOT EXISTS session_usage (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    client TEXT NOT NULL,
    model_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    step_index INTEGER NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL,
    cache_write_tokens INTEGER NOT NULL,
    reasoning_tokens INTEGER NOT NULL,
    response_id TEXT,
    pricing_day TEXT NOT NULL,
    parser_version TEXT NOT NULL,
    model_enum TEXT,
    served_model TEXT,
    display_name TEXT,
    FOREIGN KEY(session_id, client) REFERENCES sessions(session_id, client) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_session_usage_session_id ON session_usage(session_id);
CREATE INDEX IF NOT EXISTS idx_session_usage_pricing_day ON session_usage(pricing_day);
CREATE INDEX IF NOT EXISTS idx_session_usage_timestamp ON session_usage(timestamp);
CREATE INDEX IF NOT EXISTS idx_sessions_synced_at ON sessions(synced_at);
CREATE TABLE IF NOT EXISTS cache_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
`)
	if err != nil {
		return err
	}
	for _, column := range []string{"model_enum", "served_model", "display_name"} {
		var count int
		_ = db.QueryRow("SELECT COUNT(*) FROM pragma_table_info('session_usage') WHERE name = ?1", column).Scan(&count)
		if count == 0 {
			if _, err := db.Exec("ALTER TABLE session_usage ADD COLUMN " + column + " TEXT;"); err != nil {
				return err
			}
		}
	}
	var sessionPV int
	_ = db.QueryRow("SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'parser_version'").Scan(&sessionPV)
	if sessionPV == 0 {
		if _, err := db.Exec("ALTER TABLE sessions ADD COLUMN parser_version TEXT;"); err != nil {
			return err
		}
	}
	return nil
}

func normalizeFingerprint(aliases map[string]ModelAlias, releases ReleaseMap) string {
	h := sha256.New()
	enc := json.NewEncoder(h)
	_ = enc.Encode(aliases)
	_ = enc.Encode(releases)
	return hex.EncodeToString(h.Sum(nil))
}

// normalizeCache re-resolves cached rows from the raw identities they keep,
// so an alias or release map learned later corrects history. Full-table
// normalization runs only when RebuildCache is set or the (aliases, releases)
// fingerprint changes; steady-state incremental runs scope to recently dated
// or synced rows.
func (p *AntigravityParser) normalizeCache(aliases map[string]ModelAlias, since *time.Time) error {
	db, err := openAntigravityCache(p.CacheRoot)
	if err != nil {
		return err
	}
	defer db.Close()

	fp := normalizeFingerprint(aliases, p.Releases)
	var storedFP string
	_ = db.QueryRow("SELECT value FROM cache_meta WHERE key = 'normalize_fingerprint'").Scan(&storedFP)
	fullPass := p.RebuildCache || since == nil || storedFP != fp

	const normCols = "id, client, session_id, model_id, COALESCE(model_enum, ''), COALESCE(served_model, ''), COALESCE(display_name, ''), timestamp"
	query := "SELECT " + normCols + " FROM session_usage"
	var args []any
	if !fullPass {
		sinceMs := midnightMillis(*since, time.Local)
		syncedCursor := sinceMs
		if p.SyncCursorMs != nil {
			syncedCursor = *p.SyncCursorMs
		}
		query = "SELECT " + normCols + " FROM session_usage WHERE timestamp >= ?1 UNION SELECT " + normCols + " FROM session_usage WHERE session_id IN (SELECT session_id FROM sessions WHERE synced_at >= ?2)"
		args = append(args, sinceMs, syncedCursor)
	}
	rows, err := db.Query(query, args...)
	if err != nil {
		return err
	}
	type cachedRow struct {
		id, client, sessionID, modelID, enum, served, display string
		timestamp                                             int64
	}
	var cached []cachedRow
	for rows.Next() {
		var row cachedRow
		if err := rows.Scan(&row.id, &row.client, &row.sessionID, &row.modelID, &row.enum, &row.served, &row.display, &row.timestamp); err != nil {
			continue
		}
		cached = append(cached, row)
	}
	rows.Close()

	hintsBySession := map[string]map[string]AntigravityEnumHint{}
	for _, row := range cached {
		key := row.client + "\x00" + row.sessionID
		hints := hintsBySession[key]
		if hints == nil {
			hints = map[string]AntigravityEnumHint{}
			hintsBySession[key] = hints
		}
		recordAntigravityEnumHint(hints, row.enum, row.served, row.display)
	}

	type change struct {
		id, from, to, served, display string
	}
	var changes []change
	for _, row := range cached {
		hints := hintsBySession[row.client+"\x00"+row.sessionID]
		served, display := applyAntigravityEnumHint(hints, row.enum, row.served, row.display)
		target := ResolveAntigravityModel(row.enum, served, display, aliases, p.Releases, provider.LocalDateString(row.timestamp))
		if target == row.modelID && served == row.served && display == row.display {
			continue
		}
		if modelid.IsPseudo(target) && !modelid.IsPseudo(row.modelID) {
			continue
		}
		changes = append(changes, change{id: row.id, from: row.modelID, to: target, served: served, display: display})
	}
	if p.DebugLogger != nil {
		mode := "scoped"
		if fullPass {
			mode = "full"
		}
		p.DebugLogger.Printf("antigravity normalize mode=%s scanned=%d changed=%d", mode, len(cached), len(changes))
	}
	if len(changes) == 0 {
		if fullPass {
			_, _ = db.Exec(`INSERT INTO cache_meta (key, value) VALUES ('normalize_fingerprint', ?1)
ON CONFLICT(key) DO UPDATE SET value = excluded.value`, fp)
		}
		return nil
	}
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	now := time.Now().UnixMilli()
	for _, c := range changes {
		if _, err := tx.Exec("UPDATE session_usage SET model_id = ?1, provider_id = ?2, served_model = ?3, display_name = ?4 WHERE id = ?5",
			c.to, modelid.DetectProviderFromModel(c.to), c.served, c.display, c.id); err != nil {
			return err
		}
		if _, err := tx.Exec(`UPDATE sessions SET model_id = ?1, synced_at = ?2
WHERE model_id = ?3 AND (session_id, client) IN (SELECT session_id, client FROM session_usage WHERE id = ?4)`,
			c.to, now, c.from, c.id); err != nil {
			return err
		}
	}
	if fullPass {
		if _, err := tx.Exec(`INSERT INTO cache_meta (key, value) VALUES ('normalize_fingerprint', ?1)
ON CONFLICT(key) DO UPDATE SET value = excluded.value`, fp); err != nil {
			return err
		}
	}
	return tx.Commit()
}

type protoValue struct {
	varint   uint64
	bytes    []byte
	isVarint bool
}

func readVarint(buf []byte, pos *int) (uint64, bool) {
	var value uint64
	var shift uint
	for {
		if *pos >= len(buf) {
			return 0, false
		}
		b := buf[*pos]
		*pos++
		value |= uint64(b&0x7f) << shift
		if b&0x80 == 0 {
			return value, true
		}
		shift += 7
		if shift > 63 {
			return 0, false
		}
	}
}

func readProtoField(buf []byte, pos *int) (uint32, protoValue, bool) {
	key, ok := readVarint(buf, pos)
	if !ok {
		return 0, protoValue{}, false
	}
	number := uint32(key >> 3)
	take := func(n int) ([]byte, bool) {
		end := *pos + n
		if end < *pos || end > len(buf) {
			return nil, false
		}
		slice := buf[*pos:end]
		*pos = end
		return slice, true
	}
	switch key & 7 {
	case 0:
		v, ok := readVarint(buf, pos)
		return number, protoValue{varint: v, isVarint: true}, ok
	case 1:
		b, ok := take(8)
		return number, protoValue{bytes: b}, ok
	case 2:
		length, ok := readVarint(buf, pos)
		if !ok {
			return 0, protoValue{}, false
		}
		b, ok := take(int(length))
		return number, protoValue{bytes: b}, ok
	case 5:
		b, ok := take(4)
		return number, protoValue{bytes: b}, ok
	default:
		return 0, protoValue{}, false
	}
}

func forEachProtoField(buf []byte, visit func(uint32, protoValue)) {
	pos := 0
	for pos < len(buf) {
		number, value, ok := readProtoField(buf, &pos)
		if !ok {
			return
		}
		visit(number, value)
	}
}

func protoMessage(buf []byte, field uint32) []byte {
	var found []byte
	forEachProtoField(buf, func(number uint32, value protoValue) {
		if found == nil && number == field && !value.isVarint {
			found = value.bytes
		}
	})
	return found
}

func protoVarint(buf []byte, field uint32) (uint64, bool) {
	var found uint64
	var ok bool
	forEachProtoField(buf, func(number uint32, value protoValue) {
		if !ok && number == field && value.isVarint {
			found, ok = value.varint, true
		}
	})
	return found, ok
}

func protoString(buf []byte, field uint32) string {
	raw := protoMessage(buf, field)
	if raw == nil {
		return ""
	}
	return strings.TrimSpace(string(raw))
}

func protoTokenCount(buf []byte, field uint32) int64 {
	v, ok := protoVarint(buf, field)
	if !ok || v > 1<<62 {
		return 0
	}
	return int64(v)
}

func protoCustomMetadata(payload []byte, key string) string {
	var found string
	forEachProtoField(payload, func(number uint32, value protoValue) {
		if found == "" && number == 20 && !value.isVarint {
			if protoString(value.bytes, 1) == key {
				found = protoString(value.bytes, 2)
			}
		}
	})
	return found
}

type antigravityLocalUsage struct {
	requestUUID string
	responseID  string
	servedModel string
	modelEnum   string
	displayName string
	tokens      provider.TokenBreakdown
}

func parseGenMetadataUsage(blob []byte) (antigravityLocalUsage, bool) {
	payload := protoMessage(blob, 1)
	usage := protoMessage(payload, 4)
	if payload == nil || usage == nil {
		return antigravityLocalUsage{}, false
	}
	totalOutput := protoTokenCount(usage, 3)
	tokens, ok := normalizeAntigravityTokens(
		protoTokenCount(usage, 2), totalOutput, protoTokenCount(usage, 10),
		protoTokenCount(usage, 5), 0, protoTokenCount(usage, 9), true)
	if !ok || tokens.IsEmpty() {
		return antigravityLocalUsage{}, false
	}
	return antigravityLocalUsage{
		requestUUID: protoString(blob, 4),
		responseID:  protoString(usage, 11),
		servedModel: protoString(payload, 19),
		modelEnum:   protoCustomMetadata(payload, "model_enum"),
		displayName: protoString(payload, 21),
		tokens:      tokens,
	}, true
}

// normalizeAntigravityTokens keeps output and thinking disjoint.
func normalizeAntigravityTokens(input, totalOutput, responseOutput, cacheRead, cacheWrite, reasoning int64, hasResponse bool) (provider.TokenBreakdown, bool) {
	output := totalOutput - reasoning
	if output < 0 {
		output = 0
	}
	if hasResponse {
		if reasoning+responseOutput != totalOutput {
			return provider.TokenBreakdown{}, false
		}
		output = responseOutput
	}
	return provider.TokenBreakdown{
		Input: max64(input, 0), Output: max64(output, 0), CacheRead: max64(cacheRead, 0),
		CacheWrite: max64(cacheWrite, 0), Reasoning: max64(reasoning, 0),
	}, true
}

func localStepTimestamps(db *sql.DB) map[string]int64 {
	timestamps := map[string]int64{}
	rows, err := db.Query("SELECT metadata FROM steps")
	if err != nil {
		return timestamps
	}
	defer rows.Close()
	for rows.Next() {
		var blob []byte
		if err := rows.Scan(&blob); err != nil || blob == nil {
			continue
		}
		uuid := protoString(blob, 12)
		stamp := protoMessage(blob, 1)
		if uuid == "" || stamp == nil {
			continue
		}
		seconds, _ := protoVarint(stamp, 1)
		nanos, _ := protoVarint(stamp, 2)
		if nanos >= 1_000_000_000 || seconds > 1<<53 {
			continue
		}
		timestamps[uuid] = int64(seconds)*1000 + int64(nanos)/1_000_000
	}
	return timestamps
}

func readLocalConversationUsage(db *sql.DB) []struct {
	usage     antigravityLocalUsage
	timestamp *int64
} {
	timestamps := localStepTimestamps(db)
	rows, err := db.Query("SELECT data FROM gen_metadata ORDER BY idx")
	if err != nil {
		return nil
	}
	defer rows.Close()
	var records []struct {
		usage     antigravityLocalUsage
		timestamp *int64
	}
	for rows.Next() {
		var blob []byte
		if err := rows.Scan(&blob); err != nil || blob == nil {
			continue
		}
		usage, ok := parseGenMetadataUsage(blob)
		if !ok {
			continue
		}
		var timestamp *int64
		if usage.requestUUID != "" {
			if ts, ok := timestamps[usage.requestUUID]; ok {
				timestamp = &ts
			}
		}
		records = append(records, struct {
			usage     antigravityLocalUsage
			timestamp *int64
		}{usage, timestamp})
	}
	return records
}

func antigravityLocalConversationDirs(home string) [][2]any {
	gemini := filepath.Join(home, ".gemini")
	return [][2]any{
		{filepath.Join(gemini, "antigravity-cli", "conversations"), "antigravity-cli"},
		{filepath.Join(gemini, "antigravity", "conversations"), "antigravity-desktop"},
	}
}

func localConversationModifiedMs(path string) (int64, bool) {
	var latest int64
	var found bool
	check := func(p string) {
		info, err := os.Stat(p)
		if err != nil {
			return
		}
		ms := info.ModTime().UnixMilli()
		if !found || ms > latest {
			latest, found = ms, true
		}
	}
	check(path)
	if strings.HasSuffix(path, ".db") {
		check(path + "-wal")
	}
	return latest, found
}

func logicalAntigravityKey(sessionID, raw string) string { return sessionID + ":" + raw }
func storageAntigravityID(client, logical string) string { return client + ":" + logical }

func (p *AntigravityParser) sync(since *time.Time) error {
	db, err := openAntigravityCache(p.CacheRoot)
	if err != nil {
		return err
	}
	defer db.Close()
	if p.RebuildCache {
		_, _ = db.Exec("DELETE FROM session_usage;")
		_, _ = db.Exec("DELETE FROM sessions;")
	}
	home, _ := os.UserHomeDir()
	cached := loadCachedSessions(db)
	aliases := loadAliasHistory(p.CacheRoot)
	if err := syncLocalConversations(db, home, cached, p.RebuildCache, aliases, p.Releases); err != nil {
		_ = err
	}
	// Language-server sync compares against what is stored now, including
	// the conversations just written above.
	cached = loadCachedSessions(db)
	var connections []antigravityConnection
	if !p.SkipLanguageServers {
		connections, _ = detectAntigravityConnections()
	}
	if len(connections) == 0 {
		mergeAndSaveAliasHistory(p.CacheRoot, nil)
		return nil
	}
	dynamic := fetchDynamicModelAliases(connections)
	aliases = mergeAndSaveAliasHistory(p.CacheRoot, dynamic)
	return syncLanguageServers(db, home, connections, cached, p.RebuildCache, aliases, p.Releases)
}

// midnightMillis is the local start of t's day. Usage dates are local, so a
// UTC boundary would drop or add hours at every non-UTC offset.
func midnightMillis(t time.Time, location *time.Location) int64 {
	local := t.In(location)
	return time.Date(local.Year(), local.Month(), local.Day(), 0, 0, 0, 0, location).UnixMilli()
}

type cachedAntigravitySession struct {
	lastModifiedMs *int64
	stepCount      *int64
	parserVersion  string
}

func loadCachedSessions(db *sql.DB) map[[2]string]cachedAntigravitySession {
	out := map[[2]string]cachedAntigravitySession{}
	rows, err := db.Query("SELECT client, session_id, last_modified_ms, step_count, COALESCE(parser_version, '') FROM sessions")
	if err != nil {
		return out
	}
	defer rows.Close()
	for rows.Next() {
		var client, session, pv string
		var modified, steps sql.NullInt64
		if err := rows.Scan(&client, &session, &modified, &steps, &pv); err != nil {
			continue
		}
		entry := cachedAntigravitySession{parserVersion: pv}
		if modified.Valid {
			v := modified.Int64
			entry.lastModifiedMs = &v
		}
		if steps.Valid {
			v := steps.Int64
			entry.stepCount = &v
		}
		out[[2]string{client, session}] = entry
	}
	return out
}

func sessionsWithCurrentUsage(db *sql.DB) map[[2]string]bool {
	out := map[[2]string]bool{}
	rows, err := db.Query("SELECT DISTINCT client, session_id FROM session_usage WHERE parser_version = ?1", antigravityParserVersion)
	if err != nil {
		return out
	}
	defer rows.Close()
	for rows.Next() {
		var client, session string
		if rows.Scan(&client, &session) == nil {
			out[[2]string{client, session}] = true
		}
	}
	return out
}

func syncLocalConversations(db *sql.DB, home string, cached map[[2]string]cachedAntigravitySession, rebuild bool, aliases map[string]ModelAlias, releases ReleaseMap) error {
	alreadyParsed := sessionsWithCurrentUsage(db)
	now := time.Now().UnixMilli()
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()

	for _, entry := range antigravityLocalConversationDirs(home) {
		dir := entry[0].(string)
		client := entry[1].(string)
		files, err := os.ReadDir(dir)
		if err != nil {
			continue
		}
		for _, file := range files {
			if file.IsDir() || !strings.HasSuffix(file.Name(), ".db") {
				continue
			}
			sessionID := strings.TrimSuffix(file.Name(), ".db")
			if len(sessionID) < 20 {
				continue
			}
			path := filepath.Join(dir, file.Name())
			modified, hasModified := localConversationModifiedMs(path)
			key := [2]string{client, sessionID}
			if !rebuild {
				if entry, ok := cached[key]; ok {
					parsedCurrent := alreadyParsed[key] || entry.parserVersion == antigravityParserVersion
					if parsedCurrent && entry.lastModifiedMs != nil && hasModified && *entry.lastModifiedMs >= modified {
						continue
					}
				}
			}

			conversation, err := openConversationDB(path)
			if err != nil {
				continue
			}
			trajectoryID := ""
			_ = conversation.QueryRow("SELECT trajectory_id FROM trajectory_meta LIMIT 1").Scan(&trajectoryID)
			stepCount := int64(0)
			_ = conversation.QueryRow("SELECT count(*) FROM steps").Scan(&stepCount)
			usageRecords := readLocalConversationUsage(conversation)
			conversation.Close()

			hints := map[string]AntigravityEnumHint{}
			for _, record := range usageRecords {
				recordAntigravityEnumHint(hints, record.usage.modelEnum, record.usage.servedModel, record.usage.displayName)
			}

			type resolved struct {
				modelID     string
				timestamp   int64
				servedModel string
				displayName string
				usage       antigravityLocalUsage
			}
			var resolvedRecords []resolved
			var undatableIDs []string
			for _, record := range usageRecords {
				if record.timestamp == nil {
					if record.usage.responseID != "" {
						undatableIDs = append(undatableIDs, storageAntigravityID(client,
							logicalAntigravityKey(sessionID, record.usage.responseID)))
					}
					continue
				}
				date := provider.LocalDateString(*record.timestamp)
				served, display := applyAntigravityEnumHint(hints, record.usage.modelEnum, record.usage.servedModel, record.usage.displayName)
				modelID := ResolveAntigravityModel(record.usage.modelEnum, served, display, aliases, releases, date)
				resolvedRecords = append(resolvedRecords, resolved{modelID, *record.timestamp, served, display, record.usage})
			}
			primary := "unknown"
			for _, record := range resolvedRecords {
				if record.modelID != "unknown" {
					primary = record.modelID
					break
				}
			}
			summary := antigravitySyncSummary{}
			if hasModified {
				summary.LastModifiedMs = &modified
			}
			if trajectoryID != "" {
				summary.TrajectoryID = &trajectoryID
			}
			summary.StepCount = &stepCount
			if err := upsertAntigravitySession(tx, sessionID, client, summary, primary, now); err != nil {
				return err
			}
			for index, record := range resolvedRecords {
				raw := record.usage.responseID
				if raw == "" {
					raw = fmt.Sprintf("%d:%s:%d", record.timestamp, record.modelID, index)
				}
				logical := logicalAntigravityKey(sessionID, raw)
				id := storageAntigravityID(client, logical)
				if _, err := tx.Exec(`INSERT OR REPLACE INTO session_usage (
                    id, session_id, client, model_id, provider_id, timestamp, step_index,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
                    response_id, pricing_day, parser_version, model_enum, served_model, display_name
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
					id, sessionID, client, record.modelID, modelid.DetectProviderFromModel(record.modelID),
					record.timestamp, index, record.usage.tokens.Input, record.usage.tokens.Output,
					record.usage.tokens.CacheRead, record.usage.tokens.CacheWrite, record.usage.tokens.Reasoning,
					logical, provider.LocalDateString(record.timestamp), antigravityParserVersion,
					record.usage.modelEnum, record.servedModel, record.displayName); err != nil {
					return err
				}
			}
			for _, id := range undatableIDs {
				_, _ = tx.Exec("DELETE FROM session_usage WHERE id = ?1", id)
			}
		}
	}
	return tx.Commit()
}

func openConversationDB(path string) (*sql.DB, error) {
	db, err := sql.Open("sqlite", "file:"+path+"?mode=ro&_pragma=busy_timeout(5000)")
	if err != nil {
		return nil, err
	}
	if err := db.Ping(); err != nil {
		db.Close()
		return nil, err
	}
	return db, nil
}

type antigravitySyncSummary struct {
	LastModifiedMs       *int64
	TrajectoryID         *string
	Title                *string
	Status               *string
	StepCount            *int64
	CreatedTimeMs        *int64
	LastUserInputTimeMs  *int64
	ProjectID            *string
	WorkspacePath        *string
	GitRoot              *string
	Repository           *string
	GitOriginURL         *string
	BranchName           *string
	ParentConversationID *string
	MendelExperimentIDs  *string
}

// sqlExecer lets the cache writers run inside one transaction instead of
// committing every row separately. A full Antigravity re-import writes tens of
// thousands of rows; autocommit made that take minutes on copy-on-write
// filesystems.
type sqlExecer interface {
	Exec(query string, args ...any) (sql.Result, error)
}

func upsertAntigravitySession(db sqlExecer, sessionID, client string, summary antigravitySyncSummary, modelID string, syncedAt int64) error {
	_, err := db.Exec(`INSERT INTO sessions (
        session_id, trajectory_id, client, title, model_id, status, step_count,
        created_time_ms, last_modified_ms, last_user_input_time_ms, project_id,
        workspace_path, git_root, repository, git_origin_url, branch_name,
        parent_conversation_id, mendel_experiment_ids, synced_at, parser_version
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(session_id, client) DO UPDATE SET
        trajectory_id = COALESCE(excluded.trajectory_id, sessions.trajectory_id),
        title = COALESCE(excluded.title, sessions.title),
        model_id = CASE WHEN excluded.model_id = 'unknown' THEN sessions.model_id ELSE excluded.model_id END,
        status = COALESCE(excluded.status, sessions.status),
        step_count = COALESCE(excluded.step_count, sessions.step_count),
        created_time_ms = COALESCE(excluded.created_time_ms, sessions.created_time_ms),
        last_modified_ms = COALESCE(excluded.last_modified_ms, sessions.last_modified_ms),
        last_user_input_time_ms = COALESCE(excluded.last_user_input_time_ms, sessions.last_user_input_time_ms),
        project_id = COALESCE(excluded.project_id, sessions.project_id),
        workspace_path = COALESCE(excluded.workspace_path, sessions.workspace_path),
        git_root = COALESCE(excluded.git_root, sessions.git_root),
        repository = COALESCE(excluded.repository, sessions.repository),
        git_origin_url = COALESCE(excluded.git_origin_url, sessions.git_origin_url),
        branch_name = COALESCE(excluded.branch_name, sessions.branch_name),
        parent_conversation_id = COALESCE(excluded.parent_conversation_id, sessions.parent_conversation_id),
        mendel_experiment_ids = COALESCE(excluded.mendel_experiment_ids, sessions.mendel_experiment_ids),
        synced_at = excluded.synced_at,
        parser_version = excluded.parser_version`,
		sessionID, summary.TrajectoryID, client, summary.Title, modelID, summary.Status, summary.StepCount,
		summary.CreatedTimeMs, summary.LastModifiedMs, summary.LastUserInputTimeMs, summary.ProjectID,
		summary.WorkspacePath, summary.GitRoot, summary.Repository, summary.GitOriginURL, summary.BranchName,
		summary.ParentConversationID, summary.MendelExperimentIDs, syncedAt, antigravityParserVersion)
	return err
}

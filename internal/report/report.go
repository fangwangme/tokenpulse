// Package report is the one reading of the usage ledger every surface shares.
package report

import (
	"database/sql"
	"errors"
	"fmt"
	"math"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/fangwangme/tokenpulse/internal/modelid"
)

const canonicalSourceSQL = "CASE WHEN source IN ('antigravity-cli', 'antigravity-desktop', 'antigravity-ide') THEN 'antigravity' ELSE source END"

// Agents are TokenPulse's names; the HTTP API maps antigravity to agy.
var AllAgents = []string{"claude", "codex", "copilot", "gemini", "opencode", "pi", "antigravity"}

type Totals struct {
	Cost       float64 `json:"cost"`
	Tokens     int64   `json:"tokens"`
	Input      int64   `json:"input"`
	Output     int64   `json:"output"`
	CacheRead  int64   `json:"cacheRead"`
	CacheWrite int64   `json:"cacheWrite"`
	Messages   int64   `json:"messages"`
	Sessions   int64   `json:"sessions"`
}

type DaySummary struct {
	Totals
	Date string `json:"date"`
}

type ModelSummary struct {
	Totals
	Model    string   `json:"model"`
	Company  string   `json:"company"`
	Agents   []string `json:"agents"`
	LastUsed string   `json:"lastUsed"`
	Unpriced bool     `json:"unpriced"`
}

type Comparison struct {
	From         string `json:"from"`
	To           string `json:"to"`
	UntilMinutes *int   `json:"untilMinutes,omitempty"`
	Totals       Totals `json:"totals"`
}

type Range struct {
	From string `json:"from"`
	To   string `json:"to"`
}

type Report struct {
	Range           Range          `json:"range"`
	FirstDay        *string        `json:"firstDay"`
	ScannedAt       int64          `json:"scannedAt"`
	PricesAvailable bool           `json:"pricesAvailable"`
	Totals          Totals         `json:"totals"`
	Comparison      *Comparison    `json:"comparison"`
	Days            []DaySummary   `json:"days"`
	Models          []ModelSummary `json:"models"`
	Layout          string         `json:"layout"`
}

type AgentTotals struct {
	Agent string `json:"agent"`
	Totals
}

type DayModel struct {
	Model   string `json:"model"`
	Company string `json:"company"`
	Totals
}

type DayAgent struct {
	Agent  string     `json:"agent"`
	Totals Totals     `json:"totals"`
	Models []DayModel `json:"models"`
}

type DayDetail struct {
	Date   string     `json:"date"`
	Totals Totals     `json:"totals"`
	Agents []DayAgent `json:"agents"`
}

type ModelPerDay struct {
	Date   string  `json:"date"`
	Tokens int64   `json:"tokens"`
	Cost   float64 `json:"cost"`
}

type ModelDetail struct {
	Model    string        `json:"model"`
	Company  string        `json:"company"`
	Totals   Totals        `json:"totals"`
	LastUsed string        `json:"lastUsed"`
	Agents   []AgentTotals `json:"agents"`
	PerDay   []ModelPerDay `json:"perDay"`
}

// Params is a report request.
type Params struct {
	Preset string
	From   string
	To     string
	Agents []string
}

// Service answers reports from one ledger.
type Service struct {
	DBPath string
	Now    func() time.Time

	mu    sync.Mutex
	cache map[string]cacheEntry
	order []string
}

type cacheEntry struct {
	value any
}

func NewService(dbPath string) *Service {
	return &Service{DBPath: dbPath, Now: time.Now, cache: map[string]cacheEntry{}}
}

func (s *Service) open() (*sql.DB, error) {
	return sql.Open("sqlite", "file:"+s.DBPath+"?mode=ro&_pragma=busy_timeout(5000)")
}

// Clear drops every cached report; a scan, reprice or rebuild calls it.
func (s *Service) Clear() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.cache = map[string]cacheEntry{}
	s.order = nil
}

func (s *Service) cacheGet(key string) (any, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	entry, ok := s.cache[key]
	return entry.value, ok
}

func (s *Service) cachePut(key string, value any) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.cache[key]; ok {
		s.cache[key] = cacheEntry{value}
		return
	}
	if len(s.order) >= 64 {
		delete(s.cache, s.order[0])
		s.order = s.order[1:]
	}
	s.order = append(s.order, key)
	s.cache[key] = cacheEntry{value}
}

// ScannedAtValue returns the recorded last-scan time.
func (s *Service) ScannedAtValue() (int64, error) {
	db, err := s.open()
	if err != nil {
		return 0, err
	}
	defer db.Close()
	var value string
	if err := db.QueryRow("SELECT value FROM app_state WHERE key = 'scanned_at'").Scan(&value); err != nil {
		return 0, nil
	}
	var ms int64
	_, _ = fmt.Sscan(value, &ms)
	return ms, nil
}

func (s *Service) scannedAt() int64 {
	db, err := s.open()
	if err != nil {
		return 0
	}
	defer db.Close()
	var value string
	if err := db.QueryRow("SELECT value FROM app_state WHERE key = 'scanned_at'").Scan(&value); err != nil {
		return 0
	}
	var ms int64
	_, _ = fmt.Sscan(value, &ms)
	return ms
}

// SetScannedAt records a completed scan.
func (s *Service) SetScannedAt(ms int64) error {
	db, err := sql.Open("sqlite", "file:"+s.DBPath+"?_pragma=busy_timeout(5000)")
	if err != nil {
		return err
	}
	defer db.Close()
	_, err = db.Exec(`INSERT INTO app_state (key, value) VALUES ('scanned_at', ?)
ON CONFLICT(key) DO UPDATE SET value = excluded.value`, fmt.Sprintf("%d", ms))
	return err
}

func normalizeAgents(agents []string) ([]string, error) {
	if len(agents) == 0 {
		return nil, errors.New("no agents selected")
	}
	var out []string
	for _, agent := range agents {
		switch agent {
		case "claude", "codex", "copilot", "gemini", "opencode", "pi", "antigravity":
			out = append(out, agent)
		default:
			return nil, fmt.Errorf("unknown agent %q", agent)
		}
	}
	sort.Strings(out)
	return out, nil
}

func (s *Service) firstDay(db *sql.DB, agents []string) *string {
	query := "SELECT MIN(date) FROM usage_messages WHERE 1=1"
	args := []any{}
	query, args = appendAgentFilter(query, args, agents)
	var value sql.NullString
	if err := db.QueryRow(query, args...).Scan(&value); err != nil || !value.Valid {
		return nil
	}
	return &value.String
}

func sourceFilter(agents []string) (string, []any) {
	if len(agents) == 0 {
		return "", nil
	}
	var parts []string
	var args []any
	for _, agent := range agents {
		if agent == "antigravity" {
			parts = append(parts, "source IN ('antigravity','antigravity-cli','antigravity-desktop','antigravity-ide')")
		} else {
			parts = append(parts, "source = ?")
			args = append(args, agent)
		}
	}
	return " AND (" + strings.Join(parts, " OR ") + ")", args
}

func appendAgentFilter(query string, args []any, agents []string) (string, []any) {
	filter, filterArgs := sourceFilter(agents)
	return query + filter, append(args, filterArgs...)
}

func parseDate(value string) (time.Time, error) {
	return time.ParseInLocation("2006-01-02", value, time.Local)
}

func formatDate(t time.Time) string { return t.Format("2006-01-02") }

func (s *Service) resolveRange(db *sql.DB, params Params, agents []string) (time.Time, time.Time, *string, error) {
	now := s.Now().In(time.Local)
	today := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.Local)
	first := s.firstDay(db, agents)
	var from, to time.Time
	switch params.Preset {
	case "", "today":
		from, to = today, today
	case "7d":
		from, to = today.AddDate(0, 0, -6), today
	case "30d":
		from, to = today.AddDate(0, 0, -29), today
	case "year":
		from, to = time.Date(today.Year(), 1, 1, 0, 0, 0, 0, time.Local), today
	case "all":
		from, to = today, today
		if first != nil {
			if t, err := parseDate(*first); err == nil {
				from = t
			}
		}
	case "custom":
		var err error
		to = today
		from = today
		if params.To != "" {
			if to, err = parseDate(params.To); err != nil {
				return time.Time{}, time.Time{}, nil, fmt.Errorf("invalid date %q", params.To)
			}
		}
		if params.From != "" {
			if from, err = parseDate(params.From); err != nil {
				return time.Time{}, time.Time{}, nil, fmt.Errorf("invalid date %q", params.From)
			}
		}
		if from.After(to) {
			from, to = to, from
		}
		lower := today
		if first != nil {
			if t, err := parseDate(*first); err == nil {
				lower = t
			}
		}
		if from.Before(lower) || from.After(today) {
			return time.Time{}, time.Time{}, nil, fmt.Errorf("from %s is outside %s..%s", formatDate(from), formatDate(lower), formatDate(today))
		}
		if to.Before(lower) || to.After(today) {
			return time.Time{}, time.Time{}, nil, fmt.Errorf("to %s is outside %s..%s", formatDate(to), formatDate(lower), formatDate(today))
		}
	default:
		return time.Time{}, time.Time{}, nil, fmt.Errorf("unknown preset %q", params.Preset)
	}
	return from, to, first, nil
}

// Report answers one report request.
func (s *Service) Report(params Params) (Report, error) {
	agents, err := normalizeAgents(params.Agents)
	if err != nil {
		return Report{}, err
	}
	db, err := s.open()
	if err != nil {
		return Report{}, err
	}
	defer db.Close()
	from, to, first, err := s.resolveRange(db, params, agents)
	if err != nil {
		return Report{}, err
	}
	scannedAt := s.scannedAt()
	key := fmt.Sprintf("%s|%s|%s|%s|%d|%s|%s", params.Preset, params.From, params.To,
		strings.Join(agents, ","), scannedAt, formatDate(from), formatDate(to))
	if formatDate(to) == s.Now().In(time.Local).Format("2006-01-02") {
		key += "|" + s.Now().In(time.Local).Format("15:04")
	}
	if cached, ok := s.cacheGet(key); ok {
		return cached.(Report), nil
	}

	report := Report{
		Range:     Range{From: formatDate(from), To: formatDate(to)},
		FirstDay:  first,
		ScannedAt: scannedAt,
		Layout:    layoutFor(from, to),
	}
	report.Totals, err = totalsFor(db, agents, from, to, nil)
	if err != nil {
		return Report{}, err
	}
	report.Days, err = daysFor(db, agents, from, to)
	if err != nil {
		return Report{}, err
	}
	report.Models, err = modelsFor(db, agents, from, to)
	if err != nil {
		return Report{}, err
	}
	report.PricesAvailable = pricesAvailable(db)
	report.Comparison = comparisonFor(db, agents, from, to, first, s.Now(), params.Preset)
	s.cachePut(key, report)
	return report, nil
}

func pricesAvailable(db *sql.DB) bool {
	var count int
	if err := db.QueryRow("SELECT COUNT(*) FROM model_price_periods").Scan(&count); err == nil && count > 0 {
		return true
	}
	return false
}

func daysInclusive(from, to time.Time) int {
	return int(math.Round(to.Sub(from).Hours()/24)) + 1
}

func layoutFor(from, to time.Time) string {
	days := daysInclusive(from, to)
	if days <= 7 {
		return "strip"
	}
	fromMonday := mondayOf(from)
	toMonday := mondayOf(to)
	weeks := int(math.Round(toMonday.Sub(fromMonday).Hours()/24/7)) + 1
	if weeks <= 6 {
		return "week-rows"
	}
	return "week-columns"
}

func mondayOf(date time.Time) time.Time {
	offset := (int(date.Weekday()) + 6) % 7
	return date.AddDate(0, 0, -offset)
}

func totalsFor(db *sql.DB, agents []string, from, to time.Time, untilMinutes *int) (Totals, error) {
	query := `SELECT COALESCE(SUM(cost_usd),0), COALESCE(SUM(total_tokens),0), COALESCE(SUM(input_tokens),0),
        COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
        COUNT(*), COUNT(DISTINCT ` + canonicalSourceSQL + ` || '::' || session_id)
        FROM usage_messages WHERE date >= ? AND date <= ?`
	args := []any{formatDate(from), formatDate(to)}
	query, args = appendAgentFilter(query, args, agents)
	var totals Totals
	if err := db.QueryRow(query, args...).Scan(&totals.Cost, &totals.Tokens, &totals.Input, &totals.Output,
		&totals.CacheRead, &totals.CacheWrite, &totals.Messages, &totals.Sessions); err != nil {
		return Totals{}, err
	}
	if untilMinutes == nil {
		return totals, nil
	}
	rows, err := db.Query("SELECT timestamp_ms, cost_usd, total_tokens, input_tokens, output_tokens, "+
		"cache_read_tokens, cache_write_tokens, session_id, "+canonicalSourceSQL+" FROM usage_messages WHERE date >= ? AND date <= ?"+func() string {
		filter, _ := sourceFilter(agents)
		return filter
	}(), append(args, []any{}...)...)
	if err != nil {
		return Totals{}, err
	}
	defer rows.Close()
	totals = Totals{}
	sessions := map[string]bool{}
	for rows.Next() {
		var ts, tokens, input, output, cacheRead, cacheWrite int64
		var cost float64
		var session, source string
		if err := rows.Scan(&ts, &cost, &tokens, &input, &output, &cacheRead, &cacheWrite, &session, &source); err != nil {
			return Totals{}, err
		}
		local := time.UnixMilli(ts).Local()
		if local.Hour()*60+local.Minute() > *untilMinutes {
			continue
		}
		totals.Cost += cost
		totals.Tokens += tokens
		totals.Input += input
		totals.Output += output
		totals.CacheRead += cacheRead
		totals.CacheWrite += cacheWrite
		totals.Messages++
		sessions[source+"::"+session] = true
	}
	totals.Sessions = int64(len(sessions))
	return totals, rows.Err()
}

func daysFor(db *sql.DB, agents []string, from, to time.Time) ([]DaySummary, error) {
	query := `SELECT date, COALESCE(SUM(cost_usd),0), COALESCE(SUM(total_tokens),0), COALESCE(SUM(input_tokens),0),
        COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
        COUNT(*), COUNT(DISTINCT ` + canonicalSourceSQL + ` || '::' || session_id)
        FROM usage_messages WHERE date >= ? AND date <= ?`
	args := []any{formatDate(from), formatDate(to)}
	query, args = appendAgentFilter(query, args, agents)
	query += " GROUP BY date ORDER BY date DESC"
	rows, err := db.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []DaySummary
	for rows.Next() {
		var day DaySummary
		if err := rows.Scan(&day.Date, &day.Cost, &day.Tokens, &day.Input, &day.Output, &day.CacheRead,
			&day.CacheWrite, &day.Messages, &day.Sessions); err != nil {
			return nil, err
		}
		out = append(out, day)
	}
	return out, rows.Err()
}

func modelsFor(db *sql.DB, agents []string, from, to time.Time) ([]ModelSummary, error) {
	query := `SELECT canonical_model_id, COALESCE(SUM(cost_usd),0), COALESCE(SUM(total_tokens),0),
        COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read_tokens),0),
        COALESCE(SUM(cache_write_tokens),0), COUNT(*), COUNT(DISTINCT ` + canonicalSourceSQL + ` || '::' || session_id),
        MAX(date), MAX(CASE WHEN unpriced = 1 THEN 1 ELSE 0 END),
        GROUP_CONCAT(DISTINCT ` + canonicalSourceSQL + `)
        FROM usage_messages WHERE date >= ? AND date <= ?`
	args := []any{formatDate(from), formatDate(to)}
	query, args = appendAgentFilter(query, args, agents)
	query += " GROUP BY canonical_model_id ORDER BY SUM(total_tokens) DESC, canonical_model_id ASC"
	rows, err := db.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []ModelSummary
	for rows.Next() {
		var model ModelSummary
		var unpriced int
		var agentList string
		if err := rows.Scan(&model.Model, &model.Cost, &model.Tokens, &model.Input, &model.Output,
			&model.CacheRead, &model.CacheWrite, &model.Messages, &model.Sessions, &model.LastUsed,
			&unpriced, &agentList); err != nil {
			return nil, err
		}
		model.Company = modelid.DetectProviderFromModel(model.Model)
		model.Unpriced = unpriced == 1
		model.Agents = splitAgents(agentList)
		out = append(out, model)
	}
	return out, rows.Err()
}

func splitAgents(value string) []string {
	if value == "" {
		return nil
	}
	seen := map[string]bool{}
	var out []string
	for _, agent := range strings.Split(value, ",") {
		agent = strings.TrimSpace(agent)
		if agent == "" || seen[agent] {
			continue
		}
		seen[agent] = true
		out = append(out, agent)
	}
	sort.Strings(out)
	return out
}

func comparisonFor(db *sql.DB, agents []string, from, to time.Time, first *string, now time.Time, preset string) *Comparison {
	if preset == "year" || preset == "all" {
		return nil
	}
	days := daysInclusive(from, to)
	if days == 1 {
		weekday := from
		previous := weekday.AddDate(0, 0, -7)
		if first != nil {
			if firstDate, err := parseDate(*first); err == nil && previous.Before(firstDate) {
				return nil
			}
		}
		comparison := &Comparison{From: formatDate(previous), To: formatDate(previous)}
		var until *int
		if formatDate(from) == now.In(time.Local).Format("2006-01-02") {
			minutes := now.In(time.Local).Hour()*60 + now.In(time.Local).Minute()
			until = &minutes
			comparison.UntilMinutes = until
		}
		totals, err := totalsFor(db, agents, previous, previous, until)
		if err != nil {
			return nil
		}
		comparison.Totals = totals
		return comparison
	}
	previousTo := from.AddDate(0, 0, -1)
	previousFrom := previousTo.AddDate(0, 0, -(days - 1))
	if first != nil {
		if firstDate, err := parseDate(*first); err == nil && previousFrom.Before(firstDate) {
			return nil
		}
	}
	comparison := &Comparison{From: formatDate(previousFrom), To: formatDate(previousTo)}
	totals, err := totalsFor(db, agents, previousFrom, previousTo, nil)
	if err != nil {
		return nil
	}
	comparison.Totals = totals
	return comparison
}

// Day answers one day's detail.
func (s *Service) Day(date string, agents []string) (DayDetail, error) {
	agents, err := normalizeAgents(agents)
	if err != nil {
		return DayDetail{}, err
	}
	day, err := parseDate(date)
	if err != nil {
		return DayDetail{}, fmt.Errorf("invalid date %q", date)
	}
	db, err := s.open()
	if err != nil {
		return DayDetail{}, err
	}
	defer db.Close()
	detail := DayDetail{Date: date}
	detail.Totals, err = totalsFor(db, agents, day, day, nil)
	if err != nil {
		return DayDetail{}, err
	}
	query := `SELECT ` + canonicalSourceSQL + ` AS agent, canonical_model_id,
        COALESCE(SUM(cost_usd),0), COALESCE(SUM(total_tokens),0), COALESCE(SUM(input_tokens),0),
        COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
        COUNT(*), COUNT(DISTINCT session_id)
        FROM usage_messages WHERE date = ?`
	args := []any{date}
	query, args = appendAgentFilter(query, args, agents)
	query += " GROUP BY agent, canonical_model_id ORDER BY SUM(total_tokens) DESC"
	rows, err := db.Query(query, args...)
	if err != nil {
		return DayDetail{}, err
	}
	defer rows.Close()
	agentIndex := map[string]int{}
	for rows.Next() {
		var agent, model string
		var totals Totals
		if err := rows.Scan(&agent, &model, &totals.Cost, &totals.Tokens, &totals.Input, &totals.Output,
			&totals.CacheRead, &totals.CacheWrite, &totals.Messages, &totals.Sessions); err != nil {
			return DayDetail{}, err
		}
		index, ok := agentIndex[agent]
		if !ok {
			index = len(detail.Agents)
			agentIndex[agent] = index
			detail.Agents = append(detail.Agents, DayAgent{Agent: agent})
		}
		detail.Agents[index].Totals.Cost += totals.Cost
		detail.Agents[index].Totals.Tokens += totals.Tokens
		detail.Agents[index].Totals.Input += totals.Input
		detail.Agents[index].Totals.Output += totals.Output
		detail.Agents[index].Totals.CacheRead += totals.CacheRead
		detail.Agents[index].Totals.CacheWrite += totals.CacheWrite
		detail.Agents[index].Totals.Messages += totals.Messages
		detail.Agents[index].Totals.Sessions += totals.Sessions
		detail.Agents[index].Models = append(detail.Agents[index].Models, DayModel{
			Model: model, Company: modelid.DetectProviderFromModel(model), Totals: totals,
		})
	}
	if err := rows.Err(); err != nil {
		return DayDetail{}, err
	}
	sort.SliceStable(detail.Agents, func(i, j int) bool { return detail.Agents[i].Totals.Tokens > detail.Agents[j].Totals.Tokens })
	return detail, nil
}

// Model answers one model's detail over a range, with per-day zero fill.
func (s *Service) Model(model, from, to string, agents []string) (ModelDetail, error) {
	agents, err := normalizeAgents(agents)
	if err != nil {
		return ModelDetail{}, err
	}
	fromDate, err := parseDate(from)
	if err != nil {
		return ModelDetail{}, fmt.Errorf("invalid date %q", from)
	}
	toDate, err := parseDate(to)
	if err != nil {
		return ModelDetail{}, fmt.Errorf("invalid date %q", to)
	}
	if fromDate.After(toDate) {
		fromDate, toDate = toDate, fromDate
	}
	canonical := modelid.Canonical(model)
	db, err := s.open()
	if err != nil {
		return ModelDetail{}, err
	}
	defer db.Close()
	detail := ModelDetail{Model: canonical, Company: modelid.DetectProviderFromModel(canonical)}
	detail.Totals, err = totalsForModel(db, agents, canonical, fromDate, toDate)
	if err != nil {
		return ModelDetail{}, err
	}
	query := `SELECT MAX(date) FROM usage_messages WHERE canonical_model_id = ? AND date >= ? AND date <= ?`
	args := []any{canonical, from, to}
	query, args = appendAgentFilter(query, args, agents)
	_ = db.QueryRow(query, args...).Scan(&detail.LastUsed)

	agentQuery := `SELECT ` + canonicalSourceSQL + `, COALESCE(SUM(cost_usd),0), COALESCE(SUM(total_tokens),0),
        COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read_tokens),0),
        COALESCE(SUM(cache_write_tokens),0), COUNT(*), COUNT(DISTINCT session_id)
        FROM usage_messages WHERE canonical_model_id = ? AND date >= ? AND date <= ?`
	agentArgs := []any{canonical, from, to}
	agentQuery, agentArgs = appendAgentFilter(agentQuery, agentArgs, agents)
	agentQuery += " GROUP BY 1 ORDER BY SUM(total_tokens) DESC"
	rows, err := db.Query(agentQuery, agentArgs...)
	if err != nil {
		return ModelDetail{}, err
	}
	for rows.Next() {
		var entry AgentTotals
		if err := rows.Scan(&entry.Agent, &entry.Cost, &entry.Tokens, &entry.Input, &entry.Output,
			&entry.CacheRead, &entry.CacheWrite, &entry.Messages, &entry.Sessions); err != nil {
			rows.Close()
			return ModelDetail{}, err
		}
		detail.Agents = append(detail.Agents, entry)
	}
	rows.Close()

	perDay := map[string]ModelPerDay{}
	dayQuery := `SELECT date, COALESCE(SUM(total_tokens),0), COALESCE(SUM(cost_usd),0)
        FROM usage_messages WHERE canonical_model_id = ? AND date >= ? AND date <= ?`
	dayArgs := []any{canonical, from, to}
	dayQuery, dayArgs = appendAgentFilter(dayQuery, dayArgs, agents)
	dayQuery += " GROUP BY date"
	dayRows, err := db.Query(dayQuery, dayArgs...)
	if err != nil {
		return ModelDetail{}, err
	}
	for dayRows.Next() {
		var entry ModelPerDay
		if err := dayRows.Scan(&entry.Date, &entry.Tokens, &entry.Cost); err != nil {
			dayRows.Close()
			return ModelDetail{}, err
		}
		perDay[entry.Date] = entry
	}
	dayRows.Close()
	for date := fromDate; !date.After(toDate); date = date.AddDate(0, 0, 1) {
		key := formatDate(date)
		entry, ok := perDay[key]
		if !ok {
			entry = ModelPerDay{Date: key}
		}
		detail.PerDay = append(detail.PerDay, entry)
	}
	return detail, nil
}

func totalsForModel(db *sql.DB, agents []string, model string, from, to time.Time) (Totals, error) {
	query := `SELECT COALESCE(SUM(cost_usd),0), COALESCE(SUM(total_tokens),0), COALESCE(SUM(input_tokens),0),
        COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
        COUNT(*), COUNT(DISTINCT ` + canonicalSourceSQL + ` || '::' || session_id)
        FROM usage_messages WHERE canonical_model_id = ? AND date >= ? AND date <= ?`
	args := []any{model, formatDate(from), formatDate(to)}
	query, args = appendAgentFilter(query, args, agents)
	var totals Totals
	err := db.QueryRow(query, args...).Scan(&totals.Cost, &totals.Tokens, &totals.Input, &totals.Output,
		&totals.CacheRead, &totals.CacheWrite, &totals.Messages, &totals.Sessions)
	return totals, err
}

// CacheHit is cache read ÷ total tokens × 100, two decimals, 0 for no tokens.
func CacheHit(totals Totals) float64 {
	if totals.Tokens == 0 {
		return 0
	}
	value := float64(totals.CacheRead) / float64(totals.Tokens) * 100
	return float64(int64(value*100+0.5)) / 100
}

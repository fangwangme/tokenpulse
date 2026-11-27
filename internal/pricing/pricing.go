// Package pricing implements the models.dev + LiteLLM catalog, official-first
// selection, price periods and reprice.
package pricing

import (
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/fangwangme/tokenpulse/internal/modelid"
	"github.com/fangwangme/tokenpulse/internal/provider"
)

const (
	modelsDevURL = "https://models.dev/api.json"
	litellmURL   = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"
	cacheSchema  = 1
	dayDuration  = 24 * time.Hour
	retryDelay   = 10 * time.Minute
)

// Price is one model's selected price, rates in USD per million tokens.
type Price struct {
	Input       float64 `json:"input"`
	Output      float64 `json:"output"`
	CacheRead   float64 `json:"cacheRead"`
	CacheWrite  float64 `json:"cacheWrite"`
	Tier        string  `json:"tier"`
	Source      string  `json:"source"`
	ReleaseDate string  `json:"releaseDate,omitempty"`
}

type cacheFile struct {
	SchemaVersion int              `json:"schemaVersion"`
	FetchedAt     int64            `json:"fetchedAt"`
	Entries       map[string]Price `json:"entries"`
}

type owner struct {
	global   []string
	regional []string
	litellm  []string
}

var owners = map[string]owner{
	"anthropic":        {global: []string{"anthropic"}, litellm: []string{"anthropic"}},
	"openai":           {global: []string{"openai"}, litellm: []string{"openai"}},
	"google":           {global: []string{"google", "google-vertex"}, litellm: []string{"gemini", "vertex_ai", "vertex_ai-language-models"}},
	"deepseek":         {global: []string{"deepseek"}},
	"zai":              {global: []string{"zai"}, regional: []string{"zhipuai"}, litellm: []string{"zai"}},
	"minimax":          {global: []string{"minimax"}, regional: []string{"minimax-cn"}, litellm: []string{"minimax"}},
	"moonshotai":       {global: []string{"moonshotai"}, regional: []string{"moonshotai-cn"}, litellm: []string{"moonshot"}},
	"meta":             {global: []string{"meta"}},
	"nvidia":           {global: []string{"nvidia"}},
	"thinkingmachines": {global: []string{"thinkingmachines"}},
}

var litellmOfficial = map[string]bool{
	"anthropic": true, "openai": true, "gemini": true, "vertex_ai": true,
	"vertex_ai-language-models": true, "zai": true, "moonshot": true, "minimax": true,
}

type fallback struct {
	provider string
	tier     string
}

var fallbacks = []fallback{
	{"openrouter", "fallback-openrouter"},
	{"vercel", "fallback-vercel"},
	{"cloudflare-workers-ai", "fallback-cloudflare"},
	{"cloudflare-ai-gateway", "fallback-cloudflare"},
}

func startsFamily(key, family string) bool {
	return key == family || strings.HasPrefix(key, family+"-")
}

func ownerFor(key string) (owner, bool) {
	switch {
	case startsFamily(key, "claude"):
		return owners["anthropic"], true
	case startsFamily(key, "gpt"), startsFamily(key, "codex"), startsFamily(key, "chatgpt"):
		return owners["openai"], true
	case startsFamily(key, "gemini"):
		return owners["google"], true
	case startsFamily(key, "deepseek"):
		return owners["deepseek"], true
	case startsFamily(key, "glm"):
		return owners["zai"], true
	case startsFamily(key, "minimax"):
		return owners["minimax"], true
	case startsFamily(key, "kimi"), startsFamily(key, "moonshot"):
		return owners["moonshotai"], true
	case startsFamily(key, "muse"):
		return owners["meta"], true
	case startsFamily(key, "nemotron"):
		return owners["nvidia"], true
	case startsFamily(key, "inkling"):
		return owners["thinkingmachines"], true
	default:
		return owner{}, false
	}
}

type entry struct {
	id    string
	key   string
	price Price
}

type providerCatalog map[string]map[string]entry

func rawModelID(id string) string {
	if idx := strings.LastIndex(id, "/"); idx >= 0 {
		id = id[idx+1:]
	}
	return spelling(id)
}

// spelling applies the ledger's spelling normalization only.
func spelling(id string) string {
	normalized := strings.ToLower(strings.TrimSpace(id))
	normalized = strings.NewReplacer(".", "-", "_", "-", " ", "-").Replace(normalized)
	for _, prefix := range []string{"antigravity-", "anti-gravity-"} {
		normalized = strings.TrimPrefix(normalized, prefix)
	}
	return strings.Trim(modelid.CollapseRepeatedHyphens(normalized), "-")
}

func sameRates(a, b Price) bool {
	return a.Input == b.Input && a.Output == b.Output && a.CacheRead == b.CacheRead && a.CacheWrite == b.CacheWrite
}

func resolveCollision(provider, key string, entries []entry) (entry, bool) {
	if len(entries) == 1 {
		return entries[0], true
	}
	var exact []entry
	for _, candidate := range entries {
		if rawModelID(candidate.id) == key {
			exact = append(exact, candidate)
		}
	}
	if len(exact) == 1 {
		return exact[0], true
	}
	first := entries[0]
	agree := true
	for _, candidate := range entries[1:] {
		if !sameRates(candidate.price, first.price) {
			agree = false
			break
		}
	}
	if agree {
		return first, true
	}
	return entry{}, false
}

func finalizeCatalog(raw map[string]map[string][]entry) providerCatalog {
	out := providerCatalog{}
	for provider, byKey := range raw {
		resolved := map[string]entry{}
		for key, entries := range byKey {
			if selected, ok := resolveCollision(provider, key, entries); ok {
				resolved[key] = selected
			}
		}
		out[provider] = resolved
	}
	return out
}

// selectableModelsDevProviders keeps only providers selection can read.
func selectableModelsDevProviders() map[string]bool {
	out := map[string]bool{"openrouter": true, "vercel": true, "cloudflare-workers-ai": true, "cloudflare-ai-gateway": true}
	for _, o := range owners {
		for _, p := range o.global {
			out[p] = true
		}
		for _, p := range o.regional {
			out[p] = true
		}
	}
	return out
}

func indexModelsDev(data map[string]any) providerCatalog {
	selectable := selectableModelsDevProviders()
	raw := map[string]map[string][]entry{}
	for provider, providerRaw := range data {
		if !selectable[provider] {
			continue
		}
		providerData, _ := providerRaw.(map[string]any)
		models, _ := providerData["models"].(map[string]any)
		var entries []entry
		for fallbackID, modelRaw := range models {
			model, _ := modelRaw.(map[string]any)
			cost, _ := model["cost"].(map[string]any)
			if cost == nil {
				continue
			}
			input, okIn := number(cost["input"])
			output, okOut := number(cost["output"])
			if !okIn || !okOut || (input == 0 && output == 0) {
				continue
			}
			id := fallbackID
			if s, ok := model["id"].(string); ok && s != "" {
				id = s
			}
			key := modelid.Canonical(id)
			if key == "" {
				continue
			}
			cacheRead, hasRead := number(cost["cache_read"])
			cacheWrite, hasWrite := number(cost["cache_write"])
			price := Price{
				Input: input, Output: output,
				Tier: "official-global", Source: provider,
			}
			if hasRead {
				price.CacheRead = cacheRead
			}
			if hasWrite {
				price.CacheWrite = cacheWrite
			}
			if o, official := ownerFor(key); official {
				isOfficial := false
				for _, p := range o.global {
					if p == provider {
						isOfficial = true
					}
				}
				if isOfficial {
					if release, ok := model["release_date"].(string); ok && len(release) == 10 && release[4] == '-' && release[7] == '-' {
						price.ReleaseDate = release
					}
				}
			}
			entries = append(entries, entry{id: id, key: key, price: price})
		}
		if len(entries) > 0 {
			if raw[provider] == nil {
				raw[provider] = map[string][]entry{}
			}
			for _, e := range entries {
				raw[provider][e.key] = append(raw[provider][e.key], e)
			}
		}
	}
	return finalizeCatalog(raw)
}

func indexLitellm(data map[string]any) providerCatalog {
	raw := map[string]map[string][]entry{}
	for id, modelRaw := range data {
		model, _ := modelRaw.(map[string]any)
		provider, _ := model["litellm_provider"].(string)
		if !litellmOfficial[provider] {
			continue
		}
		input, ok := number(model["input_cost_per_token"])
		if !ok {
			continue
		}
		output, _ := number(model["output_cost_per_token"])
		if input == 0 && output == 0 {
			continue
		}
		key := modelid.Canonical(id)
		if key == "" {
			continue
		}
		price := Price{
			Input: input * 1_000_000, Output: output * 1_000_000,
			Tier: "official-global", Source: provider,
		}
		if read, ok := number(model["cache_read_input_token_cost"]); ok {
			price.CacheRead = read * 1_000_000
		}
		if write, ok := number(model["cache_creation_input_token_cost"]); ok {
			price.CacheWrite = write * 1_000_000
		}
		if raw[provider] == nil {
			raw[provider] = map[string][]entry{}
		}
		raw[provider][key] = append(raw[provider][key], entry{id: id, key: key, price: price})
	}
	return finalizeCatalog(raw)
}

func number(value any) (float64, bool) {
	switch v := value.(type) {
	case float64:
		return v, true
	case int:
		return float64(v), true
	case json.Number:
		f, err := v.Float64()
		return f, err == nil
	}
	return 0, false
}

func selectPrice(key string, modelsDev, litellm providerCatalog) (Price, bool) {
	if o, ok := ownerFor(key); ok {
		for _, provider := range o.global {
			if e, ok := modelsDev[provider][key]; ok {
				price := e.price
				price.Tier, price.Source = "official-global", provider
				return price, true
			}
		}
		for _, provider := range o.litellm {
			if e, ok := litellm[provider][key]; ok {
				price := e.price
				price.Tier, price.Source = "official-global", provider
				return price, true
			}
		}
		for _, provider := range o.regional {
			if e, ok := modelsDev[provider][key]; ok {
				price := e.price
				price.Tier, price.Source = "official-regional", provider
				return price, true
			}
		}
	}
	for _, fb := range fallbacks {
		if e, ok := modelsDev[fb.provider][key]; ok {
			price := e.price
			price.Tier, price.Source = fb.tier, fb.provider
			return price, true
		}
	}
	return Price{}, false
}

func selectCatalog(modelsDev, litellm providerCatalog) map[string]Price {
	keys := map[string]bool{}
	for _, byKey := range modelsDev {
		for key := range byKey {
			keys[key] = true
		}
	}
	for _, byKey := range litellm {
		for key := range byKey {
			keys[key] = true
		}
	}
	selected := map[string]Price{}
	for key := range keys {
		if price, ok := selectPrice(key, modelsDev, litellm); ok {
			selected[key] = price
		}
	}
	return selected
}

// Service owns the catalog and its periods.
type Service struct {
	mu        sync.Mutex
	CachePath string
	UsageDB   string
	Catalog   map[string]Price
	FetchedAt time.Time
	NextRetry time.Time
	Loaded    bool
	client    *http.Client
	Now       func() time.Time
	Fetch     func(url string) ([]byte, error)
	dirty     bool
}

func NewService(dataDir, usageDB string) *Service {
	return &Service{
		CachePath: filepath.Join(dataDir, "pricing-cache.json"),
		UsageDB:   usageDB,
		client:    &http.Client{Timeout: 30 * time.Second},
		Now:       time.Now,
	}
}

func (s *Service) Available() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.Catalog) > 0
}

// Load reads the disk cache and refreshes it when stale. A failed fetch keeps
// the previous catalog. When force is set a fetch is attempted regardless of
// age, and a failure with no catalog is an error.
func (s *Service) Load(force bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.Now()
	if !s.Loaded {
		s.loadCache()
		s.Loaded = true
	}
	if force {
		if err := s.refresh(); err != nil && len(s.Catalog) == 0 {
			return err
		}
		return nil
	}
	if len(s.Catalog) > 0 && now.Sub(s.FetchedAt) <= dayDuration {
		return nil
	}
	if now.Before(s.NextRetry) {
		return nil
	}
	_ = s.refresh()
	return nil
}

// Sync makes the stored periods match the catalog and backfills newly priced
// messages. It is called before a scan prices new messages.
func (s *Service) Sync(db *sql.DB) error {
	if err := s.Load(false); err != nil {
		return err
	}
	s.mu.Lock()
	dirty := s.dirty
	s.dirty = false
	empty := len(s.Catalog) > 0
	s.mu.Unlock()
	if !empty {
		return nil
	}
	var count int
	_ = db.QueryRow("SELECT COUNT(*) FROM model_price_periods").Scan(&count)
	firstSync := count == 0
	if dirty || firstSync {
		changed, err := s.SyncPeriods(db)
		if err != nil {
			return err
		}
		if !firstSync {
			for model := range changed {
				if err := recomputeModelCosts(db, model); err != nil {
					return err
				}
			}
		}
	}
	return BackfillUnpriced(db)
}

func (s *Service) loadCache() {
	content, err := os.ReadFile(s.CachePath)
	if err != nil {
		return
	}
	var cached cacheFile
	if err := json.Unmarshal(content, &cached); err != nil || cached.SchemaVersion != cacheSchema {
		return
	}
	if len(cached.Entries) == 0 {
		return
	}
	s.Catalog = cached.Entries
	s.FetchedAt = time.UnixMilli(cached.FetchedAt)
}

func (s *Service) refresh() error {
	type result struct {
		body []byte
		err  error
	}
	fetch := func(url string) result {
		if s.Fetch != nil {
			body, err := s.Fetch(url)
			return result{body: body, err: err}
		}
		resp, err := s.client.Get(url)
		if err != nil {
			return result{err: err}
		}
		defer resp.Body.Close()
		if resp.StatusCode != http.StatusOK {
			return result{err: fmt.Errorf("HTTP %d", resp.StatusCode)}
		}
		body, err := io.ReadAll(io.LimitReader(resp.Body, 64*1024*1024))
		return result{body: body, err: err}
	}
	var modelsDevResult, litellmResult result
	var wg sync.WaitGroup
	wg.Add(2)
	go func() { defer wg.Done(); modelsDevResult = fetch(modelsDevURL) }()
	go func() { defer wg.Done(); litellmResult = fetch(litellmURL) }()
	wg.Wait()

	if modelsDevResult.err != nil {
		s.NextRetry = s.Now().Add(retryDelay)
		return modelsDevResult.err
	}
	var modelsDevData map[string]any
	if err := json.Unmarshal(modelsDevResult.body, &modelsDevData); err != nil {
		s.NextRetry = s.Now().Add(retryDelay)
		return err
	}
	litellm := providerCatalog{}
	if litellmResult.err == nil {
		var litellmData map[string]any
		if json.Unmarshal(litellmResult.body, &litellmData) == nil {
			litellm = indexLitellm(litellmData)
		}
	}
	selected := selectCatalog(indexModelsDev(modelsDevData), litellm)
	if len(selected) == 0 {
		s.NextRetry = s.Now().Add(retryDelay)
		return errors.New("catalog is empty")
	}
	s.Catalog = selected
	s.dirty = true
	now := s.Now()
	s.FetchedAt = now
	s.NextRetry = time.Time{}
	return s.writeCache(now)
}

func (s *Service) writeCache(now time.Time) error {
	entries := map[string]Price{}
	for key, price := range s.Catalog {
		entries[key] = price
	}
	data, err := json.Marshal(cacheFile{SchemaVersion: cacheSchema, FetchedAt: now.UnixMilli(), Entries: entries})
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(s.CachePath), 0o755); err != nil {
		return err
	}
	return os.WriteFile(s.CachePath, data, 0o644)
}

// CostFunc returns the ledger's cost function, reading the stored periods so
// a message keeps the period its date falls in.
func (s *Service) CostFunc(db *sql.DB) (func(canonical, providerID, date string, tokens provider.TokenBreakdown) (float64, bool), error) {
	periods, err := LoadPeriods(db)
	if err != nil {
		return nil, err
	}
	byModel := map[string][]Period{}
	for _, period := range periods {
		byModel[period.Model] = append(byModel[period.Model], period)
	}
	return func(canonical, providerID, date string, tokens provider.TokenBreakdown) (float64, bool) {
		period, ok := coveringPeriod(byModel[canonical], date)
		if !ok {
			return 0, false
		}
		return period.Cost(tokens), true
	}, nil
}

// Period is one stored price period.
type Period struct {
	Model      string
	From       string
	To         string
	Input      float64
	Output     float64
	CacheRead  float64
	CacheWrite float64
	Tier       string
	Source     string
}

// Periods returns every stored period.
func LoadPeriods(db *sql.DB) ([]Period, error) {
	rows, err := db.Query(`SELECT model, effective_from, COALESCE(effective_to, ''), input_rate, output_rate,
        cache_read_rate, cache_write_rate, COALESCE(tier, ''), COALESCE(source, '')
        FROM model_price_periods ORDER BY model, effective_from`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Period
	for rows.Next() {
		var p Period
		if err := rows.Scan(&p.Model, &p.From, &p.To, &p.Input, &p.Output, &p.CacheRead, &p.CacheWrite, &p.Tier, &p.Source); err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

// Cost applies a period to a token breakdown.
func (p Period) Cost(tokens provider.TokenBreakdown) float64 {
	readRate := p.CacheRead
	if readRate == 0 {
		readRate = p.Input * 0.1
	}
	writeRate := p.CacheWrite
	if writeRate == 0 {
		writeRate = p.Input
	}
	return (float64(tokens.Input)*p.Input + float64(tokens.Output)*p.Output +
		float64(tokens.CacheRead)*readRate + float64(tokens.CacheWrite)*writeRate +
		float64(tokens.Reasoning)*p.Output) / 1_000_000
}

func covers(period Period, date string) bool {
	return period.From <= date && (period.To == "" || date <= period.To)
}

// SyncPeriods inserts/closes periods for the current catalog and returns the
// models whose price vector changed.
func (s *Service) SyncPeriods(db *sql.DB) (map[string]bool, error) {
	s.mu.Lock()
	catalog := make(map[string]Price, len(s.Catalog))
	for key, price := range s.Catalog {
		catalog[key] = price
	}
	s.mu.Unlock()
	if len(catalog) == 0 {
		return nil, nil
	}
	today := s.Now().Local().Format("2006-01-02")
	periods, err := LoadPeriods(db)
	if err != nil {
		return nil, err
	}
	byModel := map[string][]Period{}
	for _, period := range periods {
		byModel[period.Model] = append(byModel[period.Model], period)
	}
	models := map[string]bool{}
	for model := range catalog {
		models[model] = true
	}
	for model := range byModel {
		models[model] = true
	}
	sort.Strings(keysOf(models))

	tx, err := db.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	changed := map[string]bool{}
	for model := range models {
		periods := byModel[model]
		var active *Period
		for i := range periods {
			if periods[i].To == "" {
				active = &periods[i]
			}
		}
		price, selected := catalog[model]
		if !selected {
			if active != nil {
				yesterday := yesterdayOf(today)
				if active.From <= yesterday {
					if _, err := tx.Exec("UPDATE model_price_periods SET effective_to = ?1 WHERE model = ?2 AND effective_from = ?3", yesterday, model, active.From); err != nil {
						return nil, err
					}
				} else {
					// A period that only ever started today cannot cover anything.
					if _, err := tx.Exec("DELETE FROM model_price_periods WHERE model = ?1 AND effective_from = ?2", model, active.From); err != nil {
						return nil, err
					}
				}
				changed[model] = true
			}
			continue
		}
		if active == nil {
			from := "0000-01-01"
			if len(periods) > 0 {
				from = today
			}
			if err := insertPeriod(tx, model, from, price); err != nil {
				return nil, err
			}
			changed[model] = true
			continue
		}
		if active.Input == price.Input && active.Output == price.Output && active.CacheRead == price.CacheRead && active.CacheWrite == price.CacheWrite {
			if active.Tier != price.Tier || active.Source != price.Source {
				if _, err := tx.Exec("UPDATE model_price_periods SET tier = ?1, source = ?2 WHERE model = ?3 AND effective_from = ?4",
					price.Tier, price.Source, model, active.From); err != nil {
					return nil, err
				}
			}
			continue
		}
		if active.From >= today {
			if _, err := tx.Exec(`UPDATE model_price_periods SET input_rate = ?1, output_rate = ?2,
                cache_read_rate = ?3, cache_write_rate = ?4, tier = ?5, source = ?6
                WHERE model = ?7 AND effective_from = ?8`,
				price.Input, price.Output, price.CacheRead, price.CacheWrite, price.Tier, price.Source,
				model, active.From); err != nil {
				return nil, err
			}
		} else {
			yesterday := yesterdayOf(today)
			if _, err := tx.Exec("UPDATE model_price_periods SET effective_to = ?1 WHERE model = ?2 AND effective_from = ?3", yesterday, model, active.From); err != nil {
				return nil, err
			}
			if err := insertPeriod(tx, model, today, price); err != nil {
				return nil, err
			}
		}
		changed[model] = true
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	if err := syncReleases(db, catalog); err != nil {
		return nil, err
	}
	return changed, nil
}

func keysOf[V any](m map[string]V) []string {
	out := make([]string, 0, len(m))
	for key := range m {
		out = append(out, key)
	}
	return out
}

func yesterdayOf(date string) string {
	t, _ := time.ParseInLocation("2006-01-02", date, time.Local)
	return t.AddDate(0, 0, -1).Format("2006-01-02")
}

func insertPeriod(tx *sql.Tx, model, from string, price Price) error {
	_, err := tx.Exec(`INSERT INTO model_price_periods
        (model, effective_from, effective_to, input_rate, output_rate, cache_read_rate, cache_write_rate, tier, source)
        VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8)`,
		model, from, price.Input, price.Output, price.CacheRead, price.CacheWrite, price.Tier, price.Source)
	return err
}

func syncReleases(db *sql.DB, catalog map[string]Price) error {
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	for model, price := range catalog {
		if price.ReleaseDate == "" {
			continue
		}
		if _, err := tx.Exec(`INSERT INTO model_releases (model, release_date) VALUES (?1, ?2)
            ON CONFLICT(model) DO UPDATE SET release_date = excluded.release_date
            WHERE release_date <> excluded.release_date`, model, price.ReleaseDate); err != nil {
			return err
		}
	}
	return tx.Commit()
}

// Releases loads model_releases for Antigravity's unversioned-family
// resolution.
func LoadReleases(db *sql.DB) (map[string]string, error) {
	rows, err := db.Query("SELECT model, release_date FROM model_releases")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := map[string]string{}
	for rows.Next() {
		var model, date string
		if err := rows.Scan(&model, &date); err == nil {
			out[model] = date
		}
	}
	return out, rows.Err()
}

// BackfillUnpriced prices every unpriced message whose model and date a period
// covers. It is called after a catalog sync.
func BackfillUnpriced(db *sql.DB) error {
	periods, err := LoadPeriods(db)
	if err != nil {
		return err
	}
	byModel := map[string][]Period{}
	for _, period := range periods {
		byModel[period.Model] = append(byModel[period.Model], period)
	}
	rows, err := db.Query("SELECT rowid, canonical_model_id, date, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens FROM usage_messages WHERE unpriced = 1")
	if err != nil {
		return err
	}
	type update struct {
		rowid int64
		date  string
		cost  float64
	}
	var updates []update
	for rows.Next() {
		var rowid int64
		var model, date string
		var tokens provider.TokenBreakdown
		if err := rows.Scan(&rowid, &model, &date, &tokens.Input, &tokens.Output, &tokens.CacheRead, &tokens.CacheWrite, &tokens.Reasoning); err != nil {
			continue
		}
		period, ok := coveringPeriod(byModel[model], date)
		if !ok {
			continue
		}
		updates = append(updates, update{rowid, date, period.Cost(tokens)})
	}
	rows.Close()
	if len(updates) == 0 {
		return nil
	}
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	dates := map[string]bool{}
	for _, u := range updates {
		if _, err := tx.Exec("UPDATE usage_messages SET cost_usd = ?1, unpriced = 0 WHERE rowid = ?2", u.cost, u.rowid); err != nil {
			return err
		}
		dates[u.date] = true
	}
	for date := range dates {
		if err := rebuildDaily(tx, date); err != nil {
			return err
		}
	}
	return tx.Commit()
}

func coveringPeriod(periods []Period, date string) (Period, bool) {
	for _, period := range periods {
		if covers(period, date) {
			return period, true
		}
	}
	return Period{}, false
}

func rebuildDaily(tx *sql.Tx, date string) error {
	if _, err := tx.Exec("DELETE FROM daily_model_usage WHERE date = ?1", date); err != nil {
		return err
	}
	_, err := tx.Exec(`INSERT INTO daily_model_usage (
        date, source, provider_id, model_id, input_tokens, output_tokens, cache_read_tokens,
        cache_write_tokens, reasoning_tokens, total_tokens, cost_usd, message_count, session_count, updated_at)
    SELECT date, source, provider_id, canonical_model_id, SUM(input_tokens), SUM(output_tokens),
        SUM(cache_read_tokens), SUM(cache_write_tokens), SUM(reasoning_tokens), SUM(total_tokens),
        SUM(cost_usd), COUNT(*), COUNT(DISTINCT session_id), ?2
    FROM (
        SELECT date, CASE WHEN source IN ('antigravity-cli','antigravity-desktop','antigravity-ide') THEN 'antigravity' ELSE source END AS source,
            session_id, message_key, MAX(provider_id) AS provider_id, MAX(canonical_model_id) AS canonical_model_id,
            MAX(input_tokens) AS input_tokens, MAX(output_tokens) AS output_tokens,
            MAX(cache_read_tokens) AS cache_read_tokens, MAX(cache_write_tokens) AS cache_write_tokens,
            MAX(reasoning_tokens) AS reasoning_tokens, MAX(total_tokens) AS total_tokens, MAX(cost_usd) AS cost_usd
        FROM usage_messages WHERE date = ?1
        GROUP BY date, source, session_id, message_key
    ) GROUP BY date, source, provider_id, canonical_model_id`, date, time.Now().UnixMilli())
	return err
}

// recomputeModelCosts re-derives every message cost of one model from the
// periods now covering it. A cost that is already the same value is left byte
// for byte.
func recomputeModelCosts(db *sql.DB, model string) error {
	periods, err := LoadPeriods(db)
	if err != nil {
		return err
	}
	modelPeriods := make([]Period, 0, len(periods))
	for _, period := range periods {
		if period.Model == model {
			modelPeriods = append(modelPeriods, period)
		}
	}
	rows, err := db.Query(`SELECT rowid, date, input_tokens, output_tokens, cache_read_tokens,
        cache_write_tokens, reasoning_tokens, cost_usd, unpriced FROM usage_messages WHERE canonical_model_id = ?1`, model)
	if err != nil {
		return err
	}
	type row struct {
		id       int64
		date     string
		tokens   provider.TokenBreakdown
		oldCost  float64
		unpriced bool
	}
	var all []row
	for rows.Next() {
		var r row
		var unpriced int
		if err := rows.Scan(&r.id, &r.date, &r.tokens.Input, &r.tokens.Output, &r.tokens.CacheRead,
			&r.tokens.CacheWrite, &r.tokens.Reasoning, &r.oldCost, &unpriced); err != nil {
			continue
		}
		r.unpriced = unpriced == 1
		all = append(all, r)
	}
	rows.Close()
	if len(all) == 0 {
		return nil
	}
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	dates := map[string]bool{}
	for _, r := range all {
		period, ok := coveringPeriod(modelPeriods, r.date)
		cost := 0.0
		unpriced := 1
		if ok {
			cost, unpriced = period.Cost(r.tokens), 0
		}
		if unpriced == 0 && !r.unpriced && closeEnough(cost, r.oldCost) {
			continue
		}
		if _, err := tx.Exec("UPDATE usage_messages SET cost_usd = ?1, unpriced = ?2 WHERE rowid = ?3", cost, unpriced, r.id); err != nil {
			return err
		}
		dates[r.date] = true
	}
	for date := range dates {
		if err := rebuildDaily(tx, date); err != nil {
			return err
		}
	}
	return tx.Commit()
}

// Fingerprint hashes every selected rate vector, tier and source.
func (s *Service) Fingerprint() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	keys := make([]string, 0, len(s.Catalog))
	for key := range s.Catalog {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	h := sha256.New()
	for _, key := range keys {
		price := s.Catalog[key]
		fmt.Fprintf(h, "%s\t%s\t%s\t%s\t%s\t%s\t%s\n", key,
			formatRate(price.Input), formatRate(price.Output), formatRate(price.CacheRead),
			formatRate(price.CacheWrite), price.Tier, price.Source)
	}
	return hex.EncodeToString(h.Sum(nil))
}

func formatRate(value float64) string { return strconv.FormatFloat(value, 'g', -1, 64) }

// closeEnough protects a stored cost from floating-point noise when a recost
// would produce the same value from the same rates.
func closeEnough(a, b float64) bool {
	diff := a - b
	if diff < 0 {
		diff = -diff
	}
	scale := b
	if scale < 0 {
		scale = -scale
	}
	return diff <= 1e-12 || diff <= scale*1e-9
}

// ModelReprice is one model's row in a reprice report.
type ModelReprice struct {
	Model          string  `json:"model"`
	Messages       int64   `json:"messages"`
	Tokens         int64   `json:"tokens"`
	CurrentCost    float64 `json:"currentCost"`
	NewCost        float64 `json:"newCost"`
	Tier           string  `json:"tier,omitempty"`
	Source         string  `json:"source,omitempty"`
	InputRate      float64 `json:"inputRate,omitempty"`
	OutputRate     float64 `json:"outputRate,omitempty"`
	CacheReadRate  float64 `json:"cacheReadRate,omitempty"`
	CacheWriteRate float64 `json:"cacheWriteRate,omitempty"`
	Unpriced       bool    `json:"unpriced,omitempty"`
}

// RepriceReport is the dry-run/apply result.
type RepriceReport struct {
	Models      []ModelReprice `json:"models"`
	CurrentCost float64        `json:"currentCost"`
	NewCost     float64        `json:"newCost"`
	Unpriced    []string       `json:"unpriced"`
	Fingerprint string         `json:"fingerprint"`
	Applied     bool           `json:"applied"`
	Backup      string         `json:"backup,omitempty"`
}

// Reprice recomputes derived costs without touching facts.
func (s *Service) Reprice(apply bool, expect string) (RepriceReport, error) {
	if !s.Available() {
		return RepriceReport{}, errors.New("prices unavailable: models.dev has not been fetched successfully")
	}
	fingerprint := s.Fingerprint()
	if apply {
		if expect == "" {
			return RepriceReport{}, errors.New("--apply requires --expect <fingerprint>")
		}
		if expect != fingerprint {
			return RepriceReport{}, fmt.Errorf("catalog fingerprint changed (expected %s, current %s); run the dry run again", expect, fingerprint)
		}
	}
	report, err := s.computeReprice()
	if err != nil {
		return RepriceReport{}, err
	}
	report.Fingerprint = fingerprint
	if !apply {
		return report, nil
	}
	db, err := sql.Open("sqlite", "file:"+s.UsageDB+"?_pragma=busy_timeout(5000)&_pragma=journal_mode(WAL)")
	if err != nil {
		return RepriceReport{}, err
	}
	defer db.Close()
	backup, err := backupDatabase(db, s.UsageDB)
	if err != nil {
		return RepriceReport{}, err
	}
	report.Backup = backup
	if err := s.applyReprice(db); err != nil {
		return RepriceReport{}, err
	}
	report.Applied = true
	return report, nil
}

func (s *Service) catalogSnapshot() map[string]Price {
	s.mu.Lock()
	defer s.mu.Unlock()
	catalog := make(map[string]Price, len(s.Catalog))
	for model, price := range s.Catalog {
		catalog[model] = price
	}
	return catalog
}

func (s *Service) computeReprice() (RepriceReport, error) {
	catalog := s.catalogSnapshot()
	db, err := sql.Open("sqlite", "file:"+s.UsageDB+"?_pragma=busy_timeout(5000)")
	if err != nil {
		return RepriceReport{}, err
	}
	defer db.Close()
	rows, err := db.Query(`SELECT canonical_model_id, COUNT(*), SUM(input_tokens), SUM(output_tokens),
        SUM(cache_read_tokens), SUM(cache_write_tokens), SUM(reasoning_tokens), SUM(total_tokens),
        SUM(cost_usd) FROM usage_messages GROUP BY canonical_model_id`)
	if err != nil {
		return RepriceReport{}, err
	}
	defer rows.Close()
	var report RepriceReport
	for rows.Next() {
		var model string
		var messages, input, output, cacheRead, cacheWrite, reasoning, tokens int64
		var current float64
		if err := rows.Scan(&model, &messages, &input, &output, &cacheRead, &cacheWrite, &reasoning, &tokens, &current); err != nil {
			return RepriceReport{}, err
		}
		row := ModelReprice{Model: model, Messages: messages, Tokens: tokens, CurrentCost: current}
		if price, ok := catalog[model]; ok {
			period := Period{Input: price.Input, Output: price.Output, CacheRead: price.CacheRead, CacheWrite: price.CacheWrite}
			row.NewCost = period.Cost(provider.TokenBreakdown{Input: input, Output: output, CacheRead: cacheRead, CacheWrite: cacheWrite, Reasoning: reasoning})
			row.Tier, row.Source = price.Tier, price.Source
			row.InputRate, row.OutputRate, row.CacheReadRate, row.CacheWriteRate = price.Input, price.Output, price.CacheRead, price.CacheWrite
		} else {
			row.Unpriced = true
			report.Unpriced = append(report.Unpriced, model)
		}
		report.CurrentCost += current
		report.NewCost += row.NewCost
		report.Models = append(report.Models, row)
	}
	sort.Slice(report.Models, func(i, j int) bool { return report.Models[i].NewCost > report.Models[j].NewCost })
	sort.Strings(report.Unpriced)
	return report, rows.Err()
}

func (s *Service) applyReprice(db *sql.DB) error {
	catalog := s.catalogSnapshot()
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("DELETE FROM model_price_periods"); err != nil {
		return err
	}
	for model, price := range catalog {
		if _, err := tx.Exec(`INSERT INTO model_price_periods
            (model, effective_from, effective_to, input_rate, output_rate, cache_read_rate, cache_write_rate, tier, source)
            VALUES (?1, '0000-01-01', NULL, ?2, ?3, ?4, ?5, ?6, ?7)`,
			model, price.Input, price.Output, price.CacheRead, price.CacheWrite, price.Tier, price.Source); err != nil {
			return err
		}
	}
	rows, err := tx.Query(`SELECT rowid, canonical_model_id, date, input_tokens, output_tokens,
        cache_read_tokens, cache_write_tokens, reasoning_tokens FROM usage_messages`)
	if err != nil {
		return err
	}
	type row struct {
		id                               int64
		model, date                      string
		input, output, cr, cw, reasoning int64
	}
	var all []row
	for rows.Next() {
		var r row
		if err := rows.Scan(&r.id, &r.model, &r.date, &r.input, &r.output, &r.cr, &r.cw, &r.reasoning); err != nil {
			rows.Close()
			return err
		}
		all = append(all, r)
	}
	rows.Close()
	dates := map[string]bool{}
	for _, r := range all {
		price, ok := catalog[r.model]
		cost := 0.0
		unpriced := 1
		if ok {
			period := Period{Input: price.Input, Output: price.Output, CacheRead: price.CacheRead, CacheWrite: price.CacheWrite}
			cost = period.Cost(provider.TokenBreakdown{Input: r.input, Output: r.output, CacheRead: r.cr, CacheWrite: r.cw, Reasoning: r.reasoning})
			unpriced = 0
		}
		if _, err := tx.Exec("UPDATE usage_messages SET cost_usd = ?1, unpriced = ?2 WHERE rowid = ?3", cost, unpriced, r.id); err != nil {
			return err
		}
		dates[r.date] = true
	}
	for date := range dates {
		if err := rebuildDaily(tx, date); err != nil {
			return err
		}
	}
	if _, err := tx.Exec(`INSERT INTO app_state (key, value) VALUES ('reprice_rebuilt_at', ?1)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value`, strconv.FormatInt(time.Now().UnixMilli(), 10)); err != nil {
		return err
	}
	return tx.Commit()
}

func backupDatabase(db *sql.DB, usagePath string) (string, error) {
	stamp := time.Now().Format("20060102-150405")
	base := fmt.Sprintf("%s.pre-reprice.%s.bak", usagePath, stamp)
	target := base
	for i := 1; ; i++ {
		if _, err := os.Stat(target); errors.Is(err, os.ErrNotExist) {
			break
		}
		target = fmt.Sprintf("%s.%d", base, i)
	}
	if _, err := db.Exec("VACUUM INTO '" + strings.ReplaceAll(target, "'", "''") + "'"); err != nil {
		return "", err
	}
	if err := os.Chmod(target, 0o600); err != nil {
		return "", err
	}
	return target, nil
}

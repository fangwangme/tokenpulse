package pricing

import (
	"database/sql"
	"encoding/json"
	"errors"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

func testCatalogDB(t *testing.T) *sql.DB {
	t.Helper()
	path := t.TempDir() + "/usage.db"
	return openTestDB(t, path)
}

func openTestDB(t *testing.T, path string) *sql.DB {
	t.Helper()
	store := usage.NewStore(path)
	db, err := store.Open()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	return db
}

func fixedService(t *testing.T) *Service {
	t.Helper()
	dir := t.TempDir()
	service := NewService(dir, dir+"/usage.db")
	service.Now = func() time.Time { return time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local) }
	return service
}

// Protects pricing.md "Selection": official global, regional, fallback.
func TestSelectionStages(t *testing.T) {
	modelsDev := indexModelsDev(map[string]any{
		"anthropic": map[string]any{"models": map[string]any{
			"claude-opus-4-6": map[string]any{"id": "claude-opus-4-6", "cost": map[string]any{"input": 15.0, "output": 75.0}},
		}},
		"zhipuai": map[string]any{"models": map[string]any{
			"glm-5": map[string]any{"id": "glm-5", "cost": map[string]any{"input": 1.0, "output": 2.0}},
		}},
		"openrouter": map[string]any{"models": map[string]any{
			"deepseek-v4": map[string]any{"id": "deepseek-v4", "cost": map[string]any{"input": 0.5, "output": 1.5}},
			"glm-5":       map[string]any{"id": "glm-5", "cost": map[string]any{"input": 9.0, "output": 9.0}},
			"mystery":     map[string]any{"id": "mystery", "cost": map[string]any{"input": 3.0, "output": 4.0}},
		}},
	})
	catalog := selectCatalog(modelsDev, providerCatalog{})
	if price, ok := catalog["claude-opus-4-6"]; !ok || price.Source != "anthropic" || price.Tier != "official-global" {
		t.Fatalf("Claude selected %+v", price)
	}
	if price, ok := catalog["glm-5"]; !ok || price.Source != "zhipuai" || price.Tier != "official-regional" {
		t.Fatalf("GLM selected %+v", price)
	}
	if price, ok := catalog["deepseek-v4"]; !ok || price.Source != "openrouter" || price.Tier != "fallback-openrouter" {
		t.Fatalf("DeepSeek selected %+v", price)
	}
	if price, ok := catalog["mystery"]; !ok || price.Source != "openrouter" {
		t.Fatalf("ownerless model selected %+v", price)
	}
}

// Protects pricing.md "Identity": exact raw id wins; disagreement skips.
func TestSameProviderCollisions(t *testing.T) {
	modelsDev := indexModelsDev(map[string]any{
		"anthropic": map[string]any{"models": map[string]any{
			"a": map[string]any{"id": "claude-x-high", "cost": map[string]any{"input": 1.0, "output": 1.0}},
			"b": map[string]any{"id": "claude-x", "cost": map[string]any{"input": 2.0, "output": 2.0}},
		}},
	})
	entry := modelsDev["anthropic"]["claude-x"]
	if entry.id != "claude-x" {
		t.Fatalf("exact id lost: %s", entry.id)
	}

	modelsDev = indexModelsDev(map[string]any{
		"anthropic": map[string]any{"models": map[string]any{
			"a": map[string]any{"id": "claude-y-high", "cost": map[string]any{"input": 1.0, "output": 1.0}},
			"b": map[string]any{"id": "claude-y-low", "cost": map[string]any{"input": 2.0, "output": 2.0}},
		}},
	})
	if _, ok := modelsDev["anthropic"]["claude-y"]; ok {
		t.Fatal("a provider whose entries disagree must be skipped")
	}
}

// Protects pricing.md "Identity": an all-zero entry never prices.
func TestZeroPrices(t *testing.T) {
	modelsDev := indexModelsDev(map[string]any{
		"anthropic": map[string]any{"models": map[string]any{
			"free":       map[string]any{"id": "claude-free", "cost": map[string]any{"input": 0.0, "output": 0.0}},
			"zero-input": map[string]any{"id": "claude-zero-input", "cost": map[string]any{"input": 0.0, "output": 5.0}},
		}},
	})
	catalog := selectCatalog(modelsDev, providerCatalog{})
	if _, ok := catalog["claude-free"]; ok {
		t.Fatal("all-zero entry priced a model")
	}
	if price, ok := catalog["claude-zero-input"]; !ok || price.Input != 0 || price.Output != 5 {
		t.Fatalf("published zero input must be real: %+v", price)
	}
	if cost := (Period{Input: 10, Output: 20}).Cost(provider.TokenBreakdown{Reasoning: 1_000_000}); cost != 20 {
		t.Fatalf("reasoning must be priced at output: %v", cost)
	}
}

// Protects pricing.md "Price periods": open, close, update in place.
func TestPeriodOpenCloseAndInPlace(t *testing.T) {
	db := testCatalogDB(t)
	service := fixedService(t)
	service.Catalog = map[string]Price{"m": {Input: 1, Output: 2, Tier: "official-global", Source: "x"}}
	changed, err := service.SyncPeriods(db)
	if err != nil {
		t.Fatal(err)
	}
	if !changed["m"] {
		t.Fatal("first period not reported as changed")
	}
	periods, _ := LoadPeriods(db)
	if len(periods) != 1 || periods[0].From != "0000-01-01" || periods[0].To != "" {
		t.Fatalf("first period = %+v", periods)
	}
	service.Catalog["m"] = Price{Input: 2, Output: 3, Tier: "official-global", Source: "x"}
	if _, err := service.SyncPeriods(db); err != nil {
		t.Fatal(err)
	}
	periods, _ = LoadPeriods(db)
	if len(periods) != 2 || periods[0].To != "2026-11-24" || periods[1].From != "2026-11-25" {
		t.Fatalf("changed rates must close yesterday and open today: %+v", periods)
	}
	service.Catalog["m"] = Price{Input: 3, Output: 4, Tier: "official-global", Source: "x"}
	service.Now = func() time.Time { return time.Date(2026, 11, 25, 13, 0, 0, 0, time.Local) }
	if _, err := service.SyncPeriods(db); err != nil {
		t.Fatal(err)
	}
	periods, _ = LoadPeriods(db)
	if len(periods) != 2 || periods[1].Input != 3 {
		t.Fatalf("a period that starts today is updated in place: %+v", periods)
	}
	delete(service.Catalog, "m")
	service.Catalog["other"] = Price{Input: 1, Output: 1}
	if _, err := service.SyncPeriods(db); err != nil {
		t.Fatal(err)
	}
	periods, _ = LoadPeriods(db)
	for _, period := range periods {
		if period.Model == "m" && period.To == "" {
			t.Fatalf("an unpriced model must close its active period: %+v", periods)
		}
	}
}

// Protects pricing.md "Price periods": unpriced history is backfilled.
func TestUnpricedBackfill(t *testing.T) {
	db := testCatalogDB(t)
	service := fixedService(t)
	service.Catalog = map[string]Price{"m": {Input: 1, Output: 2}}
	if _, err := service.SyncPeriods(db); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`INSERT INTO usage_messages (source, client, provider_id, model_id, canonical_model_id,
        session_id, message_key, timestamp_ms, date, input_tokens, output_tokens, cache_read_tokens,
        cache_write_tokens, reasoning_tokens, total_tokens, cost_usd, pricing_day, parser_version, unpriced)
        VALUES ('claude','claude','x','m','m','s','k',0,'2026-01-01',1000000,0,0,0,0,1000000,0,'2026-01-01','v',1)`); err != nil {
		t.Fatal(err)
	}
	if err := BackfillUnpriced(db); err != nil {
		t.Fatal(err)
	}
	var cost float64
	var unpriced int
	_ = db.QueryRow("SELECT cost_usd, unpriced FROM usage_messages").Scan(&cost, &unpriced)
	if unpriced != 0 || cost != 1 {
		t.Fatalf("backfilled cost=%v unpriced=%d", cost, unpriced)
	}
}

// Protects pricing.md "Catalog sources": the Rust pricing.json is retired.
func TestRustPricingJSONIsIgnored(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(dir+"/pricing.json", []byte(`{"version":1,"models":{"claude-opus-4-6":{"input":1}}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	service := NewService(dir, dir+"/usage.db")
	service.Fetch = func(string) ([]byte, error) { return nil, errors.New("offline") }
	if err := service.Load(false); err != nil {
		t.Fatal(err)
	}
	if service.Available() {
		t.Fatal("an old pricing.json must not make prices available")
	}
}

// Protects pricing.md "Unpriced and offline": a failed fetch retries after ten
// minutes, not on every scan.
func TestRetryIntervalAfterFailure(t *testing.T) {
	dir := t.TempDir()
	service := NewService(dir, dir+"/usage.db")
	now := time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local)
	service.Now = func() time.Time { return now }
	calls := 0
	service.Fetch = func(url string) ([]byte, error) {
		calls++
		return nil, errors.New("offline")
	}
	if err := service.Load(false); err != nil {
		t.Fatal(err)
	}
	if calls != 2 {
		t.Fatalf("both catalogs are fetched in parallel: %d", calls)
	}
	now = now.Add(5 * time.Minute)
	_ = service.Load(false)
	if calls != 2 {
		t.Fatalf("retried before ten minutes: %d calls", calls)
	}
	now = now.Add(6 * time.Minute)
	_ = service.Load(false)
	if calls != 4 {
		t.Fatalf("did not retry after ten minutes: %d calls", calls)
	}
}

// Protects pricing.md "Reprice": a stale fingerprint refuses and writes nothing.
func TestRepriceRefusesStaleFingerprint(t *testing.T) {
	dir := t.TempDir()
	db := openTestDB(t, dir+"/usage.db")
	service := NewService(dir, dir+"/usage.db")
	service.Now = func() time.Time { return time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local) }
	service.Catalog = map[string]Price{"m": {Input: 1, Output: 2, Tier: "official-global", Source: "x"}}
	if _, err := db.Exec(`INSERT INTO usage_messages (source, client, provider_id, model_id, canonical_model_id,
        session_id, message_key, timestamp_ms, date, input_tokens, output_tokens, cache_read_tokens,
        cache_write_tokens, reasoning_tokens, total_tokens, cost_usd, pricing_day, parser_version, unpriced)
        VALUES ('claude','claude','x','m','m','s','k',0,'2026-01-01',1000000,0,0,0,0,1000000,7,'2026-01-01','v',0)`); err != nil {
		t.Fatal(err)
	}
	if _, err := service.Reprice(true, "wrong"); err == nil {
		t.Fatal("expected a stale fingerprint to be refused")
	}
	var cost float64
	_ = db.QueryRow("SELECT cost_usd FROM usage_messages").Scan(&cost)
	if cost != 7 {
		t.Fatalf("refused reprice changed the cost: %v", cost)
	}
	if entries, _ := os.ReadDir(dir); hasBackup(entries) {
		t.Fatal("refused reprice wrote a backup")
	}
}

func hasBackup(entries []os.DirEntry) bool {
	for _, entry := range entries {
		if strings.Contains(entry.Name(), ".bak") {
			return true
		}
	}
	return false
}

// Protects pricing.md "Reprice": apply writes a 0600 backup and new costs.
func TestRepriceApplyWritesBackup(t *testing.T) {
	dir := t.TempDir()
	db := openTestDB(t, dir+"/usage.db")
	service := NewService(dir, dir+"/usage.db")
	service.Now = func() time.Time { return time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local) }
	service.Catalog = map[string]Price{"m": {Input: 1, Output: 2, Tier: "official-global", Source: "x"}}
	if _, err := db.Exec(`INSERT INTO usage_messages (source, client, provider_id, model_id, canonical_model_id,
        session_id, message_key, timestamp_ms, date, input_tokens, output_tokens, cache_read_tokens,
        cache_write_tokens, reasoning_tokens, total_tokens, cost_usd, pricing_day, parser_version, unpriced)
        VALUES ('claude','claude','x','m','m','s','k',0,'2026-01-01',1000000,0,0,0,0,1000000,7,'2026-01-01','v',0)`); err != nil {
		t.Fatal(err)
	}
	result, err := service.Reprice(true, service.Fingerprint())
	if err != nil {
		t.Fatal(err)
	}
	if result.Backup == "" {
		t.Fatal("no backup recorded")
	}
	info, err := os.Stat(result.Backup)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("backup mode = %v, %v", info.Mode(), err)
	}
	var cost float64
	_ = db.QueryRow("SELECT cost_usd FROM usage_messages").Scan(&cost)
	if cost != 1 {
		t.Fatalf("applied cost = %v", cost)
	}
}

// Protects pricing.md "Catalog sources": cached entries are JSON at the stored
// schema version.
func TestCatalogCacheRoundTrip(t *testing.T) {
	dir := t.TempDir()
	service := NewService(dir, dir+"/usage.db")
	service.Now = func() time.Time { return time.Date(2026, 11, 25, 12, 0, 0, 0, time.Local) }
	service.Catalog = map[string]Price{"m": {Input: 1, Output: 2, Tier: "official-global", Source: "x", ReleaseDate: "2026-01-01"}}
	service.FetchedAt = service.Now()
	if err := service.writeCache(service.Now()); err != nil {
		t.Fatal(err)
	}
	loaded := NewService(dir, dir+"/usage.db")
	loaded.Now = service.Now
	if err := loaded.Load(false); err != nil {
		t.Fatal(err)
	}
	if !loaded.Available() || loaded.Catalog["m"].Output != 2 {
		t.Fatalf("cache did not round-trip: %+v", loaded.Catalog)
	}
	data, _ := os.ReadFile(dir + "/pricing-cache.json")
	var cached cacheFile
	if json.Unmarshal(data, &cached) != nil || cached.SchemaVersion != cacheSchema {
		t.Fatal("cache schema version missing")
	}
}

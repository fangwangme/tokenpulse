//go:build ignore

// Command generate seeds a deterministic synthetic ledger and quota cache for
// the README screenshots. It never reads the real data directory.
package main

import (
	"flag"
	"fmt"
	"math/rand"
	"os"
	"path/filepath"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/quota"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

type modelSpec struct {
	model      string
	source     string
	client     string
	providerID string
	weight     float64
	costPerM   float64
	cacheShare float64
}

var models = []modelSpec{
	{"claude-opus-5-5", "claude", "Claude Code", "anthropic", 0.18, 0.35, 0.91},
	{"claude-fable-5-2", "claude", "Claude Code", "anthropic", 0.03, 1.21, 0.88},
	{"claude-sonnet-5-5", "claude", "Claude Code", "anthropic", 0.04, 0.29, 0.89},
	{"gemini-3-8-flash", "antigravity", "Antigravity", "google", 0.12, 0.114, 0.93},
	{"gemini-4-hydrogen", "antigravity", "Antigravity", "google", 0.10, 0.235, 0.91},
	{"gemini-4-neon", "antigravity", "Antigravity", "google", 0.04, 0.538, 0.88},
	{"gemini-4-carbon", "antigravity", "Antigravity", "google", 0.05, 0.309, 0.90},
	{"gemini-4-argon", "antigravity", "Antigravity", "google", 0.03, 0.541, 0.89},
	{"deepseek-v4-1-flash", "opencode", "OpenCode", "deepseek", 0.30, 0.0087, 0.55},
	{"glm-5-3", "opencode", "OpenCode", "zhipu", 0.04, 0.327, 0.60},
	{"minimax-m3-1-flash", "opencode", "OpenCode", "minimax", 0.04, 0.0255, 0.62},
	{"inkling-2-small", "opencode", "OpenCode", "other", 0.03, 0.114, 0.58},
	{"gpt-5-5", "codex", "Codex", "openai", 0.03, 1.50, 0.87},
	{"gpt-5-5-codex", "codex", "Codex", "openai", 0.02, 1.35, 0.86},
}

func main() {
	dataDir := flag.String("data-dir", "", "TokenPulse data directory to create")
	days := flag.Int("days", 365, "days of synthetic usage")
	seed := flag.Int64("seed", 107, "random seed")
	flag.Parse()
	if *dataDir == "" {
		fmt.Fprintln(os.Stderr, "missing -data-dir")
		os.Exit(2)
	}
	if err := os.MkdirAll(*dataDir, 0o700); err != nil {
		panic(err)
	}
	now := time.Now()
	rng := rand.New(rand.NewSource(*seed))
	if err := seedConfig(*dataDir); err != nil {
		panic(err)
	}
	rows, err := seedUsage(*dataDir, *days, now, rng)
	if err != nil {
		panic(err)
	}
	if err := seedQuota(*dataDir, now); err != nil {
		panic(err)
	}
	fmt.Printf("seeded %d usage rows for %d days into %s\n", rows, *days, *dataDir)
}

func seedConfig(dataDir string) error {
	manager := config.NewWithPath(filepath.Join(dataDir, "config.toml"))
	cfg := config.Default()
	cfg.Display.Theme = config.ThemeDark
	cfg.Display.RefreshQuota = false
	cfg.Display.AutoRefreshSecs = 900
	cfg.Display.ScanAntigravity = false
	cfg.Display.ShowEmptyProviders = false
	cfg.Display.AccountDisplay = config.AccountFull
	cfg.Display.QuotaProviderOrder = []string{"claude", "codex", "opencode", "antigravity"}
	cfg.Keeper.Enabled = false
	return manager.Save(&cfg)
}

func seedUsage(dataDir string, days int, now time.Time, rng *rand.Rand) (int, error) {
	db, err := usage.NewStore(filepath.Join(dataDir, "usage.db")).Open()
	if err != nil {
		return 0, err
	}
	defer db.Close()
	if _, err := db.Exec(`INSERT INTO app_state (key, value) VALUES ('scanned_at', ?)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value`, fmt.Sprintf("%d", now.UnixMilli())); err != nil {
		return 0, err
	}
	weightSum := 0.0
	for _, model := range models {
		weightSum += model.weight
	}
	insert := `INSERT INTO usage_messages (
        source, client, provider_id, model_id, canonical_model_id, session_id, message_key,
        timestamp_ms, date, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
        reasoning_tokens, total_tokens, cost_usd, pricing_day, parser_version, served_model, unpriced)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 0)`
	tx, err := db.Begin()
	if err != nil {
		return 0, err
	}
	defer tx.Rollback()
	stmt, err := tx.Prepare(insert)
	if err != nil {
		return 0, err
	}
	defer stmt.Close()
	rows := 0
	baseTokens := 220_000_000.0
	for offset := days - 1; offset >= 0; offset-- {
		day := now.AddDate(0, 0, -offset)
		if offset >= 7 && rng.Float64() < 0.08 {
			continue
		}
		factor := 0.45 + rng.Float64()*1.15
		if day.Weekday() == time.Saturday || day.Weekday() == time.Sunday {
			factor *= 0.45
		}
		date := day.Format("2006-01-02")
		for _, model := range models {
			if rng.Float64() < 0.12 {
				continue
			}
			total := int64(baseTokens * model.weight / weightSum * factor * (0.7 + rng.Float64()*0.6))
			if total < 500_000 {
				continue
			}
			sessions := 1 + rng.Intn(2)
			messages := 2 + rng.Intn(3)
			remaining := total
			for index := 0; index < messages; index++ {
				share := 1.0 / float64(messages-index)
				if messages-index > 1 {
					share = 0.6 / float64(messages-index)
				}
				part := int64(float64(remaining) * share)
				if index == messages-1 {
					part = remaining
				}
				if part < 1_000 {
					continue
				}
				remaining -= part
				session := fmt.Sprintf("demo-%s-%s-%d", date, model.source, index%sessions)
				key := fmt.Sprintf("%s-%s-%d", date, model.model, index)
				cacheRead := int64(float64(part) * model.cacheShare)
				remainingTokens := part - cacheRead
				input := int64(float64(remainingTokens) * 0.55)
				output := int64(float64(remainingTokens) * 0.32)
				reasoning := remainingTokens - input - output
				cacheWrite := int64(float64(part) * 0.015)
				if cacheWrite < 0 {
					cacheWrite = 0
				}
				tokens := input + output + cacheRead + cacheWrite + reasoning
				cost := float64(tokens) / 1_000_000 * model.costPerM
				stamp := time.Date(day.Year(), day.Month(), day.Day(), 9+rng.Intn(9), rng.Intn(60), 0, 0, time.Local)
				if _, err := stmt.Exec(model.source, model.client, model.providerID, model.model, model.model,
					session, key, stamp.UnixMilli(), date, input, output, cacheRead, cacheWrite, reasoning,
					tokens, cost, date, "demo-v1"); err != nil {
					return rows, err
				}
				rows++
			}
		}
	}
	if err := tx.Commit(); err != nil {
		return rows, err
	}
	return rows, nil
}

func seedQuota(dataDir string, now time.Time) error {
	cache := quota.NewCache(dataDir)
	fiveHours := int64((5 * time.Hour) / time.Millisecond)
	sevenDays := int64((7 * 24 * time.Hour) / time.Millisecond)
	thirtyDays := int64((30 * 24 * time.Hour) / time.Millisecond)
	reset := func(d time.Duration) *time.Time {
		value := now.Add(d).UTC()
		return &value
	}
	snapshots := []provider.QuotaSnapshot{
		{Provider: "claude", Plan: "pro", Account: "demo@example.com", FetchedAt: now, Windows: []provider.RateWindow{
			{Label: "Session (5h)", UsedPercent: 42, PeriodMs: &fiveHours, ResetsAt: reset(80 * time.Minute)},
			{Label: "Weekly (7d)", UsedPercent: 68, PeriodMs: &sevenDays, ResetsAt: reset(3 * 24 * time.Hour)},
		}},
		{Provider: "codex", Plan: "plus", Account: "demo@example.com", FetchedAt: now, Windows: []provider.RateWindow{
			{Label: "Session (5h)", UsedPercent: 12, PeriodMs: &fiveHours, ResetsAt: reset(2 * time.Hour)},
			{Label: "Weekly (7d)", UsedPercent: 41, PeriodMs: &sevenDays, ResetsAt: reset(4 * 24 * time.Hour)},
		}},
		{Provider: "opencode", Account: "demo@example.com", FetchedAt: now, Windows: []provider.RateWindow{
			{Label: "5h", UsedPercent: 27, PeriodMs: &fiveHours, ResetsAt: reset(2 * time.Hour)},
			{Label: "Weekly", UsedPercent: 58, PeriodMs: &sevenDays, ResetsAt: reset(2 * 24 * time.Hour)},
			{Label: "Monthly", UsedPercent: 34, PeriodMs: &thirtyDays, ResetsAt: reset(22 * 24 * time.Hour)},
		}},
		{Provider: "antigravity", Plan: "Pro", Account: "demo@example.com", FetchedAt: now, Windows: []provider.RateWindow{
			{Label: "Gemini (5h)", Family: "Gemini", UsedPercent: 18, PeriodMs: &fiveHours, ResetsAt: reset(time.Hour)},
			{Label: "Gemini (7d)", Family: "Gemini", UsedPercent: 46, PeriodMs: &sevenDays, ResetsAt: reset(5 * 24 * time.Hour)},
			{Label: "Claude (5h)", Family: "Claude", UsedPercent: 7, PeriodMs: &fiveHours, ResetsAt: reset(time.Hour)},
			{Label: "Claude (7d)", Family: "Claude", UsedPercent: 31, PeriodMs: &sevenDays, ResetsAt: reset(5 * 24 * time.Hour)},
		}},
	}
	for _, snapshot := range snapshots {
		quota.Normalize(&snapshot)
		if err := cache.Save(snapshot.Provider, now, &snapshot); err != nil {
			return err
		}
	}
	return nil
}

// Package cli implements every tokenpulse command and flag.
package cli

import (
	"encoding/csv"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
	money "github.com/fangwangme/tokenpulse/internal/format"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/notification"
	"github.com/fangwangme/tokenpulse/internal/paths"
	"github.com/fangwangme/tokenpulse/internal/pricing"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/quota"
	"github.com/fangwangme/tokenpulse/internal/remote"
	"github.com/fangwangme/tokenpulse/internal/report"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
	"github.com/fangwangme/tokenpulse/internal/scan"
	"github.com/fangwangme/tokenpulse/internal/summary"
	"github.com/fangwangme/tokenpulse/internal/tui"
	"github.com/fangwangme/tokenpulse/internal/usage"
	"github.com/fangwangme/tokenpulse/internal/version"
)

// Main dispatches one command line.
func Main(args []string) int {
	if err := run(args); err != nil {
		fmt.Fprintln(os.Stderr, "Error:", err)
		return 1
	}
	return 0
}

func run(args []string) error {
	if len(args) > 0 {
		switch args[0] {
		case "-h", "--help":
			printHelp()
			return nil
		case "-V", "--version":
			fmt.Printf("tokenpulse %s\n", version.String())
			return nil
		case "init":
			return runInit(args[1:])
		case "config":
			return runConfig(args[1:])
		case "usage":
			return runUsage(args[1:], true)
		case "reprice":
			return runReprice(args[1:])
		case "serve":
			return runServe(args[1:])
		case "remote":
			return runRemote(args[1:])
		case "service":
			return runService(args[1:])
		case "help":
			printHelp()
			return nil
		case "version":
			fmt.Printf("tokenpulse %s\n", version.String())
			return nil
		}
	}
	return runUsage(args, false)
}

func printHelp() {
	fmt.Println(`Token usage and quota dashboard for coding agents

Usage: tokenpulse [OPTIONS] [COMMAND]

Commands:
  init     Write default configuration (--default)
  config   Show or change configuration
  usage    Inspect usage
  reprice  Recompute costs under the current price rules
  serve    Run the remote server and the phone app
  remote   Manage the remote token and pairing URL
  service  Install or remove the user service
  help     Print this help

Options:
      --since <DATE>          Only show usage since this date
      --refresh-days <RANGE>  Refresh one YYYY-MM-DD:YYYY-MM-DD range
      --refresh-pricing       Force a price catalog fetch
      --rebuild-all           Reparse every source from scratch
      --no-scan               Read the cached ledger without scanning
      --csv <daily|models>    Emit CSV
      --json                  Emit JSON
      --no-tui                Use plain text instead of the interactive dashboard
      --raw                   Show raw model usage
      --date <DATE>           Date for --raw
      --log                   Write a performance log line
  -h, --help                  Print help
  -V, --version               Print version`)
}

func printUsageHelp() {
	fmt.Println(`Usage: tokenpulse usage [OPTIONS]

Inspect the usage ledger. Without --no-scan the command reuses a running
serve scan or refreshes the ledger first.

Options:
      --since <DATE>          Only show usage since this date
      --refresh-days <RANGE>  Refresh one YYYY-MM-DD:YYYY-MM-DD range
      --refresh-pricing       Force a price catalog fetch
      --rebuild-all           Reparse every source from scratch
      --no-scan               Read the cached ledger without scanning
      --csv <daily|models>    Emit CSV
      --json                  Emit JSON
      --no-tui                Use plain text instead of the interactive dashboard
      --raw                   Show raw model usage
      --date <DATE>           Date for --raw
      --log                   Write a performance log line
  -h, --help                  Print this help`)
}

func runUsage(args []string, subcommand bool) error {
	flags := flag.NewFlagSet("usage", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	flags.Usage = printUsageHelp
	var since, refreshDays, csvFormat, rawDate string
	var refreshPricing, rebuildAll, jsonOut, noTUI, noScan, logPerf, raw bool
	flags.StringVar(&since, "since", "", "")
	flags.StringVar(&refreshDays, "refresh-days", "", "")
	flags.BoolVar(&refreshPricing, "refresh-pricing", false, "")
	flags.BoolVar(&rebuildAll, "rebuild-all", false, "")
	flags.BoolVar(&noScan, "no-scan", false, "")
	flags.StringVar(&csvFormat, "csv", "", "")
	flags.BoolVar(&jsonOut, "json", false, "")
	flags.BoolVar(&noTUI, "no-tui", false, "")
	flags.BoolVar(&logPerf, "log", false, "")
	flags.BoolVar(&raw, "raw", false, "")
	flags.StringVar(&rawDate, "date", "", "")
	versionFlag := flags.Bool("version", false, "")
	if err := flags.Parse(args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return nil
		}
		return err
	}
	if *versionFlag {
		fmt.Printf("tokenpulse %s\n", version.String())
		return nil
	}
	if raw {
		return runRaw(rawDate, jsonOut)
	}
	return runUsageReport(since, refreshDays, csvFormat, refreshPricing, rebuildAll, jsonOut, noTUI, noScan, logPerf)
}

func runUsageReport(since, refreshDays, csvFormat string, refreshPricing, rebuildAll, jsonOut, noTUI, noScan, logPerf bool) error {
	started := time.Now()
	dataDir, err := paths.DataDir()
	if err != nil {
		return err
	}
	manager, err := config.New()
	if err != nil {
		return err
	}
	cfg, err := manager.Load()
	if err != nil {
		cfg = config.Default()
	}
	var sinceDate *time.Time
	if since != "" {
		parsed, err := time.ParseInLocation("2006-01-02", since, time.Local)
		if err != nil {
			return fmt.Errorf("invalid --since date %q", since)
		}
		sinceDate = &parsed
	}
	var from, to *time.Time
	if refreshDays != "" {
		parts := strings.SplitN(refreshDays, ":", 2)
		if len(parts) != 2 {
			return errors.New("Expected --refresh-days in YYYY-MM-DD:YYYY-MM-DD format")
		}
		fromDate, err1 := time.ParseInLocation("2006-01-02", parts[0], time.Local)
		toDate, err2 := time.ParseInLocation("2006-01-02", parts[1], time.Local)
		if err1 != nil || err2 != nil {
			return errors.New("invalid --refresh-days range")
		}
		if toDate.Before(fromDate) {
			return errors.New("refresh-days end must not be earlier than start")
		}
		from, to = &fromDate, &toDate
	}

	scanner, err := scan.New(dataDir)
	if err != nil {
		return err
	}
	outputSince := sinceDate
	if outputSince == nil {
		outputSince = from
	}

	client := remote.NewLocalClient(dataDir)
	status, statusErr := client.Status()
	if statusErr == nil && status.ScannedAt == 0 && status.Running && !noScan &&
		refreshDays == "" && !refreshPricing && !rebuildAll {
		fmt.Fprintln(os.Stderr, "Waiting for the running serve scan…")
		deadline := time.Now().Add(2 * time.Minute)
		for time.Now().Before(deadline) {
			time.Sleep(500 * time.Millisecond)
			if current, err := client.Status(); err == nil && current.ScannedAt > 0 {
				status = current
				break
			}
		}
	}
	serveCacheReady := statusErr == nil && status.ScannedAt > 0
	explicit := refreshDays != "" || refreshPricing || rebuildAll
	scanNeeded := !noScan && (explicit || !serveCacheReady)

	if !noTUI && interactive() && !jsonOut && csvFormat == "" {
		if explicit && !noScan {
			lock, err := rt.AcquireLock(dataDir)
			if err != nil {
				if errors.Is(err, rt.ErrLocked) {
					return errors.New("another TokenPulse instance owns this data directory; close it or stop serve before running --rebuild-all/--refresh-days/--refresh-pricing")
				}
				return err
			}
			fmt.Fprintln(os.Stderr, "Scanning usage…")
			scanErr := scanner.Run(scan.Options{
				Since: once(sinceDate), RefreshFrom: from, RefreshTo: to,
				RefreshPricing: refreshPricing, RebuildAll: rebuildAll,
				Providers: scan.ProvidersFromConfig(cfg), ScanAntigravity: cfg.Display.ScanAntigravity,
			})
			_ = lock.Release()
			if scanErr != nil {
				return scanErr
			}
			if logPerf {
				writePerfLog(dataDir, started)
			}
		}
		return tui.Run(tui.Options{
			DataDir: dataDir, Scanner: scanner, Config: cfg, ConfigManager: manager,
			Since: outputSince, RefreshPricing: refreshPricing, NoScan: noScan || explicit,
		})
	}

	if scanNeeded {
		lock, lockErr := rt.AcquireLock(dataDir)
		if lockErr != nil {
			if errors.Is(lockErr, rt.ErrLocked) {
				if explicit {
					return errors.New("another TokenPulse instance owns this data directory; close it or stop serve before running --rebuild-all/--refresh-days/--refresh-pricing")
				}
				// Another runtime holds the lock; delegate refresh via socket if
				// available, then read the ledger read-only.
				if _, err := client.Status(); err == nil {
					_ = client.Post("/api/refresh", map[string]bool{"usage": true, "quota": false}, nil)
					scanNeeded = false
				} else {
					return errors.New("another TokenPulse instance is running; use --no-scan to read cached data")
				}
			} else {
				return lockErr
			}
		}
		if scanNeeded {
			defer lock.Release()
			if !jsonOut {
				fmt.Fprintln(os.Stderr, "Scanning usage… (--no-scan reads the cached ledger)")
			}
			if err := scanner.Run(scan.Options{
				Since: once(sinceDate), RefreshFrom: from, RefreshTo: to,
				RefreshPricing: refreshPricing, RebuildAll: rebuildAll,
				Providers: scan.ProvidersFromConfig(cfg), ScanAntigravity: cfg.Display.ScanAntigravity,
			}); err != nil {
				return err
			}
			if logPerf {
				writePerfLog(dataDir, started)
			}
		}
	}
	if !scanNeeded {
		if _, err := os.Stat(filepath.Join(dataDir, "usage.db")); err != nil {
			return errors.New("no cached usage yet; run tokenpulse once without --no-scan")
		}
		// Read the shared ledger without schema or journal writes so a scan
		// in another process cannot block this command.
		scanner.Store = usage.NewReadOnlyStore(filepath.Join(dataDir, "usage.db"))
		scanner.Report = report.NewService(filepath.Join(dataDir, "usage.db"))
	}
	store := scanner.Store
	messageCount, sessionCount, err := store.LoadSummaryCounts(outputSince, nil, scan.ProvidersFromConfig(cfg))
	if err != nil {
		return err
	}
	if jsonOut {
		return outputJSON(scanner, cfg, messageCount, sessionCount, outputSince)
	}
	if csvFormat != "" {
		return outputCSV(store, cfg, csvFormat, outputSince)
	}
	return outputText(scanner, cfg, messageCount, sessionCount, outputSince)
}

func once(value *time.Time) *time.Time {
	if value == nil {
		return nil
	}
	copy := *value
	return &copy
}

func interactive() bool {
	if os.Getenv("TERM") == "dumb" {
		return false
	}
	in, _ := os.Stdin.Stat()
	out, _ := os.Stdout.Stat()
	return in != nil && out != nil && in.Mode()&os.ModeCharDevice != 0 && out.Mode()&os.ModeCharDevice != 0
}

func writePerfLog(dataDir string, started time.Time) {
	logDir := filepath.Join(dataDir, "log")
	_ = os.MkdirAll(logDir, 0o755)
	path := filepath.Join(logDir, "usage-"+time.Now().Format("2006-01-02")+".log")
	f, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	if err != nil {
		return
	}
	defer f.Close()
	fmt.Fprintf(f, "%s event=complete elapsed_ms=%d\n", time.Now().UTC().Format(time.RFC3339Nano), time.Since(started).Milliseconds())
}

func buildSummary(scanner *scan.Scanner, cfg config.Config, messages, sessions int, since *time.Time) (summary.Summary, error) {
	days, err := scanner.Store.LoadDashboardDays(since, nil, scan.ProvidersFromConfig(cfg))
	if err != nil {
		return summary.Summary{}, err
	}
	providerRows, err := scanner.Store.LoadProviderSummaries(since, nil, scan.ProvidersFromConfig(cfg))
	if err != nil {
		return summary.Summary{}, err
	}
	modelRows, err := scanner.Store.LoadModelSummaries(since, nil, scan.ProvidersFromConfig(cfg))
	if err != nil {
		return summary.Summary{}, err
	}
	daily := make([]summary.Day, 0, len(days))
	for _, day := range days {
		daily = append(daily, summary.Day{
			Date: day.Date, TotalTokens: day.TotalTokens, TotalCostUSD: day.CostUSD,
			InputTokens: day.InputTokens, OutputTokens: day.OutputTokens,
			CacheReadTokens: day.CacheReadTokens, CacheWriteTokens: day.CacheWriteTokens,
			ReasoningTokens: day.ReasoningTokens, MessageCount: day.MessageCount, SessionCount: day.SessionCount,
		})
	}
	providers := make([]summary.Provider, 0, len(providerRows))
	for _, row := range providerRows {
		providers = append(providers, summary.Provider{
			Provider: row.Provider, Cost: row.Cost, Tokens: row.Tokens,
			MessageCount: int(row.MessageCount), SessionCount: int(row.SessionCount),
		})
	}
	models := make([]summary.Model, 0, len(modelRows))
	for _, row := range modelRows {
		models = append(models, summary.Model{
			Model: row.Model, Provider: joinSet(row.Providers), Source: joinSet(row.Sources),
			Cost: row.Cost, Tokens: row.Tokens, InputTokens: row.Input, OutputTokens: row.Output,
			CacheTokens: row.CacheRead + row.CacheWrite, CacheReadTokens: row.CacheRead,
			CacheWriteTokens: row.CacheWrite, MessageCount: row.MessageCount, SessionCount: len(row.Sessions),
		})
	}
	result := summary.Build(daily, providers, models, messages, sessions)
	if result.MessageCount == 0 {
		return result, nil
	}
	return result, nil
}

func joinSet(values map[string]bool) string {
	keys := make([]string, 0, len(values))
	for key := range values {
		keys = append(keys, key)
	}
	sortStrings(keys)
	return strings.Join(keys, ",")
}

func sortStrings(values []string) {
	for i := 1; i < len(values); i++ {
		for j := i; j > 0 && values[j] < values[j-1]; j-- {
			values[j], values[j-1] = values[j-1], values[j]
		}
	}
}

func outputJSON(scanner *scan.Scanner, cfg config.Config, messages, sessions int, since *time.Time) error {
	result, err := buildSummary(scanner, cfg, messages, sessions, since)
	if err != nil {
		return err
	}
	snapshots := currentSnapshots(cfg)
	payload := struct {
		Usage summary.Summary          `json:"usage"`
		Quota []provider.QuotaSnapshot `json:"quota"`
	}{result, snapshots}
	encoder := json.NewEncoder(os.Stdout)
	encoder.SetIndent("", "  ")
	return encoder.Encode(payload)
}

// currentSnapshots only reads the persisted quota cache; usage reports never
// call a provider, and a manual refresh is the only thing that does.
func currentSnapshots(cfg config.Config) []provider.QuotaSnapshot {
	dataDir, err := paths.DataDir()
	if err != nil {
		return nil
	}
	var out []provider.QuotaSnapshot
	cache := quota.NewCache(dataDir)
	for _, id := range cfg.EnabledQuotaProviders(cfg.QuotaProviderOrder(quota.ProviderIDs)) {
		if cached, err := cache.LoadAny(id); err == nil && cached != nil {
			out = append(out, cached.Snapshot)
		}
	}
	return out
}

func outputCSV(store *usage.Store, cfg config.Config, format string, since *time.Time) error {
	if format == "models" {
		result, err := buildSummaryFromStore(store, cfg, since)
		if err != nil {
			return err
		}
		return writeModelsCSV(os.Stdout, result.ByModel)
	}
	rows, err := store.LoadDailyRows(since, nil, scan.ProvidersFromConfig(cfg))
	if err != nil {
		return err
	}
	return writeDailyCSV(os.Stdout, rows)
}

func writeModelsCSV(w io.Writer, models []summary.Model) error {
	writer := csv.NewWriter(w)
	if err := writer.Write([]string{"model", "provider", "source", "tokens", "cost_usd", "messages", "sessions", "percent"}); err != nil {
		return err
	}
	for _, model := range models {
		record := []string{
			model.Model, model.Provider, model.Source, strconv.FormatInt(model.Tokens, 10),
			strconv.FormatFloat(model.Cost, 'f', 6, 64), strconv.Itoa(model.MessageCount),
			strconv.Itoa(model.SessionCount), strconv.FormatFloat(model.Percent, 'f', 2, 64),
		}
		if err := writer.Write(record); err != nil {
			return err
		}
	}
	writer.Flush()
	return writer.Error()
}

func writeDailyCSV(w io.Writer, rows []usage.DailyUsageRow) error {
	writer := csv.NewWriter(w)
	if err := writer.Write([]string{"date", "source", "total_tokens", "cost_usd", "input_tokens", "output_tokens",
		"cache_read_tokens", "cache_write_tokens", "messages", "sessions"}); err != nil {
		return err
	}
	for _, row := range rows {
		record := []string{
			row.Date, row.Source, strconv.FormatInt(row.TotalTokens, 10),
			strconv.FormatFloat(row.CostUSD, 'f', 6, 64), strconv.FormatInt(row.InputTokens, 10),
			strconv.FormatInt(row.OutputTokens, 10), strconv.FormatInt(row.CacheReadTokens, 10),
			strconv.FormatInt(row.CacheWriteTokens, 10), strconv.FormatInt(row.MessageCount, 10),
			strconv.FormatInt(row.SessionCount, 10),
		}
		if err := writer.Write(record); err != nil {
			return err
		}
	}
	writer.Flush()
	return writer.Error()
}

func buildSummaryFromStore(store *usage.Store, cfg config.Config, since *time.Time) (summary.Summary, error) {
	messages, sessions, err := store.LoadSummaryCounts(since, nil, scan.ProvidersFromConfig(cfg))
	if err != nil {
		return summary.Summary{}, err
	}
	scanner := &scan.Scanner{Store: store}
	return buildSummary(scanner, cfg, messages, sessions, since)
}

func outputText(scanner *scan.Scanner, cfg config.Config, messages, sessions int, since *time.Time) error {
	result, err := buildSummary(scanner, cfg, messages, sessions, since)
	if err != nil {
		return err
	}
	if result.MessageCount == 0 {
		fmt.Fprint(os.Stderr, "\nNo usage data found in the local ledger.\n\n")
		fmt.Fprintln(os.Stderr, "Checked providers:")
		fmt.Fprintln(os.Stderr, " - Claude Code: ~/.claude/projects/ or ~/.claude/transcripts/")
		fmt.Fprintln(os.Stderr, " - Codex: ~/.codex/sessions/")
		fmt.Fprintln(os.Stderr, " - Copilot: ~/.local/share/github-copilot/events.jsonl")
		fmt.Fprintln(os.Stderr, " - OpenCode: ~/.local/share/opencode/")
		fmt.Fprintln(os.Stderr, " - Gemini CLI: ~/.gemini/tmp/")
		fmt.Fprintln(os.Stderr, " - PI: ~/.pi/agent/sessions/")
		fmt.Fprintln(os.Stderr, " - Antigravity: ~/.local/share/tokenpulse/antigravity-cache/sessions/")
		return nil
	}
	printSummary(result)
	printQuotaSummary(currentSnapshots(cfg), cfg.Display.QuotaDisplayMode, cfg.Display.AccountDisplay)
	return nil
}

func printSummary(result summary.Summary) {
	fmt.Println("\n=== Usage Summary ===")
	fmt.Printf("Total cost: %s\n", money.Cost(result.TotalCost))
	fmt.Printf("Total tokens: %s\n", formatInt(result.TotalTokens))
	fmt.Printf("Messages: %s\n", formatInt(int64(result.MessageCount)))
	fmt.Printf("Sessions: %s\n", formatInt(int64(result.SessionCount)))
	fmt.Printf("Active days: %s\n", formatInt(int64(result.ActiveDays)))
	fmt.Printf("Avg daily cost: %s\n", money.Cost(result.AvgDailyCost))
	fmt.Printf("Avg daily tokens: %s\n", formatInt(int64(result.AvgDailyTokens+0.5)))

	fmt.Println("\n=== By Provider ===")
	for _, provider := range result.ByProvider {
		fmt.Printf("%s: %s tokens | %s | %s messages | %s sessions\n",
			strings.ToUpper(provider.Provider), formatInt(provider.Tokens), money.Cost(provider.Cost),
			formatInt(int64(provider.MessageCount)), formatInt(int64(provider.SessionCount)))
	}
	fmt.Println("\n=== By Model ===")
	for _, model := range result.ByModel {
		fmt.Printf("%s [%s]: %s tokens | %s | %s messages\n", model.Model, model.Source,
			formatInt(model.Tokens), money.Cost(model.Cost), formatInt(int64(model.MessageCount)))
	}
	fmt.Println("\n=== Recent Daily Totals ===")
	days := result.Daily
	if len(days) > 365 {
		days = days[len(days)-365:]
	}
	for _, day := range days {
		fmt.Printf("%s: %s tokens | %s | %s messages | %s sessions\n", day.Date,
			formatInt(day.TotalTokens), money.Cost(day.TotalCostUSD), formatInt(day.MessageCount), formatInt(day.SessionCount))
	}
	fmt.Println("\n=== Weekly Totals ===")
	weeks := result.Weekly
	if len(weeks) > 8 {
		weeks = weeks[len(weeks)-8:]
	}
	for _, week := range weeks {
		fmt.Printf("%s: %s tokens | %s | %s messages | %s active days\n", week.Label,
			formatInt(week.TotalTokens), money.Cost(week.TotalCostUSD), formatInt(week.MessageCount), formatInt(week.ActiveDays))
	}
	fmt.Println("\n=== Monthly Totals ===")
	months := result.Monthly
	if len(months) > 6 {
		months = months[len(months)-6:]
	}
	for _, month := range months {
		fmt.Printf("%s: %s tokens | %s | %s messages | %s active days\n", month.Label,
			formatInt(month.TotalTokens), money.Cost(month.TotalCostUSD), formatInt(month.MessageCount), formatInt(month.ActiveDays))
	}
	fmt.Printf("\nLoaded %s ledger messages from %s provider(s).\n",
		formatInt(int64(result.MessageCount)), formatInt(int64(len(result.ByProvider))))
}

func printQuotaSummary(snapshots []provider.QuotaSnapshot, mode config.QuotaDisplayMode, account config.AccountDisplay) {
	if len(snapshots) == 0 {
		return
	}
	fmt.Println("\n=== Quota Status ===")
	for _, snapshot := range snapshots {
		fmt.Printf("\nProvider: %s\n", strings.ToUpper(snapshot.Provider))
		if account.ShowsPlan() {
			plan := snapshot.Plan
			if plan == "" {
				plan = "None"
			}
			fmt.Printf("  Plan: %s\n", displayPlan(plan))
		}
		if account.ShowsAccount() {
			accountValue := snapshot.Account
			if accountValue == "" {
				accountValue = "None"
			}
			fmt.Printf("  Account: %s\n", accountValue)
		}
		for _, window := range snapshot.Windows {
			used := window.UsedPercent
			remaining := used
			if mode == config.QuotaRemaining {
				remaining = 100 - used
				if remaining < 0 {
					remaining = 0
				}
			}
			label := "used"
			if mode == config.QuotaRemaining {
				label = "remaining"
			}
			fmt.Printf("  - %s: %.2f%% %s", window.Label, remaining, label)
			if window.ResetsAt != nil {
				if diff := window.ResetsAt.Sub(time.Now()); diff > 0 {
					minutes := int(diff.Minutes())
					fmt.Printf(" (resets in %dh %dm)", minutes/60, minutes%60)
				}
			}
			fmt.Println()
		}
	}
}

func displayPlan(plan string) string {
	runes := []rune(strings.TrimSpace(plan))
	if len(runes) == 0 {
		return ""
	}
	return strings.ToUpper(string(runes[0])) + string(runes[1:])
}

func formatInt(value int64) string {
	raw := strconv.FormatInt(value, 10)
	negative := strings.HasPrefix(raw, "-")
	if negative {
		raw = raw[1:]
	}
	var parts []string
	for len(raw) > 3 {
		parts = append([]string{raw[len(raw)-3:]}, parts...)
		raw = raw[:len(raw)-3]
	}
	parts = append([]string{raw}, parts...)
	out := strings.Join(parts, ",")
	if negative {
		return "-" + out
	}
	return out
}

func runRaw(date string, jsonOut bool) error {
	dataDir, err := paths.DataDir()
	if err != nil {
		return err
	}
	target := time.Now()
	if date != "" {
		target, err = time.ParseInLocation("2006-01-02", date, time.Local)
		if err != nil {
			return fmt.Errorf("Invalid date format '%s', expected YYYY-MM-DD", date)
		}
	}
	store := scanStore(dataDir)
	rows, err := store.LoadRawModelsForDate(target.Format("2006-01-02"))
	if err != nil {
		return err
	}
	var totalTokens, totalMessages int64
	var totalCost float64
	for _, row := range rows {
		totalTokens += row.TotalTokens
		totalCost += row.CostUSD
		totalMessages += row.MessageCount
	}
	sessions, _ := store.CountDistinctSessionsForDate(target.Format("2006-01-02"))
	if jsonOut {
		type modelJSON struct {
			Source           string  `json:"source"`
			ServedModel      string  `json:"served_model"`
			TierModel        string  `json:"tier_model"`
			CanonicalModel   string  `json:"canonical_model"`
			InputTokens      int64   `json:"input_tokens"`
			OutputTokens     int64   `json:"output_tokens"`
			CacheReadTokens  int64   `json:"cache_read_tokens"`
			CacheWriteTokens int64   `json:"cache_write_tokens"`
			ReasoningTokens  int64   `json:"reasoning_tokens"`
			TotalTokens      int64   `json:"total_tokens"`
			CostUSD          float64 `json:"cost_usd"`
			MessageCount     int64   `json:"message_count"`
			SessionCount     int64   `json:"session_count"`
		}
		payload := struct {
			Date         string      `json:"date"`
			TotalTokens  int64       `json:"total_tokens"`
			TotalCostUSD float64     `json:"total_cost_usd"`
			MessageCount int64       `json:"message_count"`
			SessionCount int64       `json:"session_count"`
			Models       []modelJSON `json:"models"`
		}{Date: target.Format("2006-01-02"), TotalTokens: totalTokens,
			TotalCostUSD: round6(totalCost), MessageCount: totalMessages, SessionCount: sessions}
		for _, row := range rows {
			payload.Models = append(payload.Models, modelJSON{
				Source: row.Source, ServedModel: row.RawModelID, TierModel: row.TierModelID,
				CanonicalModel: row.CanonicalModelID, InputTokens: row.InputTokens,
				OutputTokens: row.OutputTokens, CacheReadTokens: row.CacheReadTokens,
				CacheWriteTokens: row.CacheWriteTokens, ReasoningTokens: row.ReasoningTokens,
				TotalTokens: row.TotalTokens, CostUSD: round6(row.CostUSD),
				MessageCount: row.MessageCount, SessionCount: row.SessionCount,
			})
		}
		encoder := json.NewEncoder(os.Stdout)
		encoder.SetIndent("", "  ")
		return encoder.Encode(payload)
	}
	fmt.Printf("\n=== Raw Model Usage (Date: %s) ===\n", target.Format("2006-01-02"))
	if len(rows) == 0 {
		fmt.Printf("No usage records found for %s.\n", target.Format("2006-01-02"))
		return nil
	}
	fmt.Printf("Summary: %d models | %s tokens | %s | %s msgs | %s sessions\n",
		len(rows), formatInt(totalTokens), money.Cost(totalCost), formatInt(totalMessages), formatInt(sessions))
	fmt.Printf("%-12s %-36s %-30s %-28s %12s %10s %8s\n",
		"Source", "Raw / Served Model", "Tier / Alias", "Canonical", "Total Tkns", "Cost USD", "Calls")
	for _, row := range rows {
		tier := row.TierModelID
		if tier == row.RawModelID {
			tier = "-"
		}
		fmt.Printf("%-12s %-36s %-30s %-28s %12s %10s %8s\n",
			row.Source, truncate(row.RawModelID, 36), truncate(tier, 30), truncate(row.CanonicalModelID, 28),
			formatInt(row.TotalTokens), money.Cost(row.CostUSD), formatInt(row.MessageCount))
	}
	return nil
}

func round6(value float64) float64 {
	return float64(int64(value*1_000_000+0.5)) / 1_000_000
}

func truncate(value string, width int) string {
	if width <= 0 {
		return ""
	}
	runes := []rune(value)
	if len(runes) <= width {
		return value
	}
	if width == 1 {
		return string(runes[:1])
	}
	return string(runes[:width-1]) + "…"
}

func runReprice(args []string) error {
	flags := flag.NewFlagSet("reprice", flag.ContinueOnError)
	var apply, yes, jsonOut bool
	var expect string
	flags.BoolVar(&apply, "apply", false, "write the recomputed costs to the ledger")
	flags.BoolVar(&yes, "yes", false, "confirm --apply")
	flags.BoolVar(&jsonOut, "json", false, "emit JSON")
	flags.StringVar(&expect, "expect", "", "require this catalog fingerprint")
	if err := flags.Parse(args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return nil
		}
		return err
	}
	if apply && !yes {
		return errors.New("--apply requires --yes")
	}
	dataDir, err := paths.DataDir()
	if err != nil {
		return err
	}
	client := remote.NewLocalClient(dataDir)
	if _, err := client.Status(); err == nil {
		var result pricing.RepriceReport
		body := map[string]any{"apply": apply, "yes": yes, "expect": expect}
		if err := client.Post("/api/reprice", body, &result); err != nil {
			return err
		}
		return printReprice(result, jsonOut)
	}
	lock, err := rt.AcquireLock(dataDir)
	if err != nil {
		if errors.Is(err, rt.ErrLocked) {
			return errors.New("another TokenPulse instance owns this data directory; close it or stop serve before running reprice")
		}
		return err
	}
	defer lock.Release()
	// Reprice reads a ledger written by 0.6.x too; opening the store applies
	// the forward schema migration before any cost is computed.
	store := usage.NewStore(filepath.Join(dataDir, "usage.db"))
	migrated, err := store.Open()
	if err != nil {
		return err
	}
	migrated.Close()
	service := pricing.NewService(dataDir, filepath.Join(dataDir, "usage.db"))
	if err := service.Load(false); err != nil {
		return err
	}
	result, err := service.Reprice(apply, expect)
	if err != nil {
		return err
	}
	return printReprice(result, jsonOut)
}

func printReprice(result pricing.RepriceReport, jsonOut bool) error {
	if jsonOut {
		encoder := json.NewEncoder(os.Stdout)
		encoder.SetIndent("", "  ")
		return encoder.Encode(result)
	}
	fmt.Printf("Fingerprint: %s\n\n", result.Fingerprint)
	fmt.Printf("%-36s %10s %10s %12s %12s  %s\n", "Model", "Messages", "Tokens", "Current", "New", "Tier")
	for _, model := range result.Models {
		fmt.Printf("%-36s %10d %10s %12.4f %12.4f  %s\n", truncate(model.Model, 36), model.Messages,
			formatInt(model.Tokens), model.CurrentCost, model.NewCost, model.Tier)
	}
	fmt.Printf("\nCurrent total: $%.4f\nNew total: $%.4f\n", result.CurrentCost, result.NewCost)
	if len(result.Unpriced) > 0 {
		fmt.Printf("Unpriced models: %s\n", strings.Join(result.Unpriced, ", "))
	}
	if result.Applied {
		fmt.Printf("Applied. Backup: %s\n", result.Backup)
	}
	return nil
}

func scanStore(dataDir string) *usage.Store {
	path := filepath.Join(dataDir, "usage.db")
	if _, err := os.Stat(path); err != nil {
		return usage.NewStore(path)
	}
	return usage.NewReadOnlyStore(path)
}

func printConfigHelp() {
	fmt.Println(`Usage: tokenpulse config <show|enable|disable|set|test-notification>

  show                 Print the effective configuration
  enable <provider>    Enable a quota provider
  disable <provider>   Disable a quota provider
  set KEY=VALUE        Change one setting
  test-notification    Send a sample notification`)
}

func runConfig(args []string) error {
	if len(args) == 0 {
		return runConfigShow()
	}
	switch args[0] {
	case "-h", "--help", "help":
		printConfigHelp()
		return nil
	case "show":
		return runConfigShow()
	case "enable", "disable":
		if len(args) < 2 {
			return errors.New("missing provider")
		}
		if !quota.IsProvider(args[1]) {
			return fmt.Errorf("Unknown quota provider '%s'. Configurable quota providers: %s. (Usage parsing is not configured here.)",
				args[1], strings.Join(quota.ProviderIDs, ", "))
		}
		manager, err := config.New()
		if err != nil {
			return err
		}
		if args[0] == "enable" {
			if err := manager.EnableProvider(args[1]); err != nil {
				return err
			}
			fmt.Printf("Quota provider '%s' enabled\n", args[1])
		} else {
			if err := manager.DisableProvider(args[1]); err != nil {
				return err
			}
			fmt.Printf("Quota provider '%s' disabled\n", args[1])
		}
		return nil
	case "set":
		if len(args) < 2 {
			return errors.New("Expected KEY=VALUE format (e.g. quota_display_mode=used)")
		}
		return runConfigSet(args[1])
	case "test-notification":
		return runTestNotification()
	default:
		return fmt.Errorf("unknown config command %q", args[0])
	}
}

func runConfigShow() error {
	manager, err := config.New()
	if err != nil {
		return err
	}
	cfg, err := manager.Load()
	if err != nil {
		return err
	}
	fmt.Printf("Config file: %s\n\nProviders:\n", manager.Path)
	for _, name := range cfg.QuotaProviderOrder(quota.ProviderIDs) {
		status := "disabled"
		if p, ok := cfg.Providers[name]; ok && p.Enabled {
			status = "enabled"
		}
		fmt.Printf("  %s: %s\n", name, status)
	}
	fmt.Println("\nDisplay:")
	fmt.Printf("  show_empty_providers: %v\n", cfg.Display.ShowEmptyProviders)
	fmt.Printf("  account_display: %s\n", cfg.Display.AccountDisplay)
	fmt.Printf("  theme: %s\n", cfg.Display.Theme)
	mode := "remaining"
	if cfg.Display.QuotaDisplayMode == config.QuotaUsed {
		mode = "used"
	}
	fmt.Printf("  quota_display_mode: %s\n", mode)
	fmt.Printf("  quota_provider_order: %s\n", strings.Join(cfg.QuotaProviderOrder(quota.ProviderIDs), ", "))
	refresh := "disabled"
	if cfg.Display.AutoRefreshSecs > 0 {
		if cfg.Display.AutoRefreshSecs%60 == 0 {
			refresh = fmt.Sprintf("%d min", cfg.Display.AutoRefreshSecs/60)
		} else {
			refresh = fmt.Sprintf("%ds", cfg.Display.AutoRefreshSecs)
		}
	}
	fmt.Printf("  auto_refresh_interval: %s\n", refresh)
	fmt.Printf("  refresh_quota: %v\n", cfg.Display.RefreshQuota)
	fmt.Printf("  notification_level: %s\n", cfg.Display.NotificationLevel)
	fmt.Printf("  notification_sound: %s\n", cfg.Display.NotificationSound)
	fmt.Printf("\nKeeper:\n  keeper_engine: %v\n  keeper_antigravity_groups: %s\n",
		cfg.Keeper.Enabled, cfg.Keeper.AntigravityGroupsLabel())
	for _, name := range keeper.Agents {
		agent, ok := cfg.Keeper.Agents[name]
		if !ok {
			continue
		}
		dataDir, _ := paths.DataDir()
		model, err := keeper.ResolveCheapestModel(name, dataDir)
		if err != nil {
			model = "unresolved: " + err.Error()
		}
		fmt.Printf("  %s: 5h=%v weekly=%v at %s (%s)\n", name, agent.SessionKeeperEnabled,
			agent.WeeklyKeeperEnabled, agent.DailyWakeupTime, model)
	}
	return nil
}

func runConfigSet(setting string) error {
	key, value, ok := strings.Cut(setting, "=")
	if !ok {
		return errors.New("Expected KEY=VALUE format (e.g. quota_display_mode=used)")
	}
	key, value = strings.TrimSpace(key), strings.TrimSpace(value)
	manager, err := config.New()
	if err != nil {
		return err
	}
	cfg, err := manager.Load()
	if err != nil {
		return err
	}
	switch key {
	case "quota_display_mode":
		switch value {
		case "used":
			cfg.Display.QuotaDisplayMode = config.QuotaUsed
		case "remaining":
			cfg.Display.QuotaDisplayMode = config.QuotaRemaining
		default:
			return fmt.Errorf("Invalid value '%s' for quota_display_mode. Expected: used, remaining", value)
		}
	case "show_empty_providers":
		parsed, err := parseBool(value)
		if err != nil {
			return fmt.Errorf("Invalid value '%s' for show_empty_providers. Expected: true, false", value)
		}
		cfg.Display.ShowEmptyProviders = parsed
	case "account_display":
		parsed, ok := config.ParseAccountDisplay(value)
		if !ok {
			return fmt.Errorf("Invalid value '%s' for account_display. Expected: none, plan, full", value)
		}
		cfg.Display.AccountDisplay = parsed
	case "theme":
		switch value {
		case "auto":
			cfg.Display.Theme = config.ThemeAuto
		case "dark":
			cfg.Display.Theme = config.ThemeDark
		case "light":
			cfg.Display.Theme = config.ThemeLight
		default:
			return fmt.Errorf("Invalid value '%s' for theme. Expected: auto, dark, light", value)
		}
	case "auto_refresh_interval":
		minutes, err := strconv.Atoi(value)
		if err != nil {
			return fmt.Errorf("Invalid value '%s' for auto_refresh_interval. Expected: 0, 1, 2, 5, 10, 15", value)
		}
		seconds := 0
		switch minutes {
		case 0:
			seconds = 0
		case 1:
			seconds = 60
		case 2:
			seconds = 120
		case 5:
			seconds = 300
		case 10:
			seconds = 600
		case 15:
			seconds = 900
		default:
			return fmt.Errorf("Invalid value '%s' for auto_refresh_interval. Supported intervals: 0, 1, 2, 5, 10, 15 (minutes)", value)
		}
		cfg.Display.AutoRefreshSecs = uint32(seconds)
	case "keeper_engine":
		parsed, err := parseBool(value)
		if err != nil {
			return fmt.Errorf("Invalid value '%s' for keeper_engine. Expected: true, false", value)
		}
		cfg.Keeper.Enabled = parsed
	case "keeper_antigravity_groups":
		switch value {
		case config.AntigravityGroupsPrimary:
			cfg.Keeper.SetAntigravityAllGroups(false)
		case config.AntigravityGroupsAll:
			cfg.Keeper.SetAntigravityAllGroups(true)
		default:
			return fmt.Errorf("Invalid value '%s' for keeper_antigravity_groups. Expected: %s, %s",
				value, config.AntigravityGroupsPrimary, config.AntigravityGroupsAll)
		}
	case "refresh_quota":
		parsed, err := parseBool(value)
		if err != nil {
			return fmt.Errorf("Invalid value '%s' for refresh_quota. Expected: true, false", value)
		}
		cfg.Display.RefreshQuota = parsed
	case "notification_level":
		switch value {
		case "off":
			cfg.Display.NotificationLevel = config.NotificationOff
		case "in_app":
			cfg.Display.NotificationLevel = config.NotificationInApp
		case "terminal":
			cfg.Display.NotificationLevel = config.NotificationTerminal
		case "system":
			cfg.Display.NotificationLevel = config.NotificationSystem
		default:
			return fmt.Errorf("Invalid value '%s' for notification_level. Expected: off, in_app, terminal, system", value)
		}
	case "notification_sound":
		if !validSound(value) {
			return fmt.Errorf("Invalid value '%s' for notification_sound. Expected: chime (built-in), none (silent), or the name of a sound under /System/Library/Sounds (e.g. Hero, Glass, Submarine)", value)
		}
		cfg.Display.NotificationSound = value
	case "quota_provider_order":
		ids := strings.Split(value, ",")
		order := make([]string, 0, len(ids))
		for _, id := range ids {
			id = strings.TrimSpace(id)
			if id == "" {
				continue
			}
			if !quota.IsProvider(id) {
				return fmt.Errorf("Invalid provider '%s' for quota_provider_order. Expected: %s", id, strings.Join(quota.ProviderIDs, ", "))
			}
			order = append(order, id)
		}
		cfg.Display.QuotaProviderOrder = order
		cfg.Display.QuotaProviderOrder = cfg.QuotaProviderOrder(quota.ProviderIDs)
	default:
		return fmt.Errorf("Unknown setting '%s'. Available settings:\n  quota_display_mode     (used | remaining)\n  show_empty_providers   (true | false)\n  account_display        (none | plan | full)\n  theme                  (auto | dark | light)\n  auto_refresh_interval  (0 | 1 | 2 | 5 | 10 | 15 — minutes, 0 = disabled)\n  refresh_quota          (true | false)\n  notification_level     (off | in_app | terminal | system)\n  notification_sound     (chime | none | a name under /System/Library/Sounds, e.g. Hero)\n  keeper_engine          (true | false)\n  keeper_antigravity_groups (gemini_flash | all)\n  quota_provider_order   (comma-separated provider IDs)", key)
	}
	if err := manager.Save(&cfg); err != nil {
		return err
	}
	fmt.Printf("%s = %s\n", key, value)
	return nil
}

func parseBool(value string) (bool, error) {
	switch value {
	case "true", "1", "yes":
		return true, nil
	case "false", "0", "no":
		return false, nil
	}
	return false, errors.New("invalid boolean")
}

func validSound(value string) bool {
	if strings.EqualFold(value, notification.SOUNDChime) || strings.EqualFold(value, notification.SOUNDNone) {
		return true
	}
	info, err := os.Stat("/System/Library/Sounds/" + value + ".aiff")
	return err == nil && !info.IsDir()
}

func runTestNotification() error {
	manager, err := config.New()
	if err != nil {
		return err
	}
	cfg, err := manager.Load()
	if err != nil {
		return err
	}
	fmt.Printf("notification_level: %s\nnotification_sound: %s\n", cfg.Display.NotificationLevel, cfg.Display.NotificationSound)
	if cfg.Display.NotificationLevel == config.NotificationOff {
		fmt.Println("\nLevel is 'off', so nothing will fire. Try: tokenpulse config set notification_level=system")
		return nil
	}
	notification.TestNotification(cfg.Display.NotificationLevel, cfg.Display.NotificationSound)
	fmt.Println("\nSent a sample notification.")
	return nil
}

func printInitHelp() {
	fmt.Println(`Usage: tokenpulse init --default

Detect installed providers and write a configuration that enables them.`)
}

func runInit(args []string) error {
	useDefaults := false
	for _, arg := range args {
		switch arg {
		case "-h", "--help":
			printInitHelp()
			return nil
		case "--default":
			useDefaults = true
		}
	}
	manager, err := config.New()
	if err != nil {
		return err
	}
	fmt.Print("\nWelcome to TokenPulse! Let's set up your configuration.\n\n")
	fmt.Println("Detecting installed providers...")
	detected := detectProviders()
	for _, entry := range []struct{ name, label string }{
		{"claude", "Claude Code"}, {"codex", "Codex"}, {"opencode", "OpenCode Go"}, {"antigravity", "Antigravity"},
	} {
		found := detected[entry.name]
		icon := "\u2717"
		hint := "not detected"
		if found {
			icon = "\u2713"
			hint = "detected"
		}
		fmt.Printf("  %s %-13s (%s)\n", icon, entry.label, hint)
	}
	fmt.Println()
	selected := map[string]bool{}
	if useDefaults {
		for name, found := range detected {
			if found {
				selected[name] = true
			}
		}
		if len(selected) == 0 {
			fmt.Println("No providers detected. All providers will be disabled.")
		} else {
			fmt.Printf("Auto-enabling %d detected provider(s).\n", len(selected))
		}
	} else {
		return errors.New("interactive init is not available in this build; run `tokenpulse init --default`")
	}
	cfg := config.Default()
	for _, name := range []string{"claude", "codex", "opencode", "antigravity"} {
		cfg.Providers[name] = config.ProviderConfig{Enabled: selected[name]}
	}
	if err := manager.Save(&cfg); err != nil {
		return err
	}
	fmt.Printf("\nConfiguration saved to %s\n", manager.Path)
	fmt.Println("\nRun `tokenpulse` to check your usage!")
	return nil
}

func detectProviders() map[string]bool {
	home, _ := os.UserHomeDir()
	exists := func(path string) bool {
		_, err := os.Stat(path)
		return err == nil
	}
	return map[string]bool{
		"claude":      exists(filepath.Join(home, ".claude", ".credentials.json")),
		"codex":       exists(filepath.Join(home, ".codex")) || exists(filepath.Join(home, ".config", "codex")),
		"opencode":    exists(quota.OpenCodeDatabasePath(home)),
		"antigravity": exists(filepath.Join(home, ".gemini", "antigravity-cli", "conversations")),
	}
}

// Package scan runs one usage refresh: price, parse, ingest, rebuild.
package scan

import (
	"log"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/pricing"
	"github.com/fangwangme/tokenpulse/internal/provider"
	"github.com/fangwangme/tokenpulse/internal/report"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

const codexRepairMigration = "codex-zstd-repair-v1"

type Options struct {
	Since           *time.Time
	RefreshFrom     *time.Time
	RefreshTo       *time.Time
	RefreshPricing  bool
	RebuildAll      bool
	Providers       []string
	ScanAntigravity bool
}

type Scanner struct {
	DataDir string
	Store   *usage.Store
	Pricing *pricing.Service
	Report  *report.Service
	Logger  *log.Logger
	Debug   bool

	testParsers []usage.Parser
}

func (s *Scanner) debugEnabled() bool {
	if s.Debug {
		return true
	}
	v := os.Getenv("TOKENPULSE_DEBUG")
	return v == "1" || v == "true"
}

func New(dataDir string) (*Scanner, error) {
	store := usage.NewStore(filepath.Join(dataDir, "usage.db"))
	return &Scanner{
		DataDir: dataDir,
		Store:   store,
		Pricing: pricing.NewService(dataDir, filepath.Join(dataDir, "usage.db")),
		Report:  report.NewService(filepath.Join(dataDir, "usage.db")),
	}, nil
}

// Run scans every selected source and writes the ledger.
func (s *Scanner) Run(options Options) error {
	db, err := s.Store.Open()
	if err != nil {
		return err
	}
	defer db.Close()

	if err := s.Pricing.Load(options.RefreshPricing); err != nil {
		return err
	}
	if err := s.Pricing.Sync(db); err != nil {
		return err
	}
	releases, err := pricing.LoadReleases(db)
	if err != nil {
		return err
	}
	cost, err := s.Pricing.CostFunc(db)
	if err != nil {
		return err
	}

	parsers := s.parsers(options, releases)
	versions := map[string]string{}
	for _, parser := range parsers {
		versions[parser.Name()] = parser.ParserVersion()
	}
	stale := map[string]bool{}
	if !options.RebuildAll && options.RefreshFrom == nil {
		stale, err = s.Store.CheckStaleParserVersions(versions)
		if err != nil {
			return err
		}
	}

	codexRepair := false
	if options.Since == nil && options.RefreshFrom == nil {
		for _, parser := range parsers {
			if parser.Name() != "codex" {
				continue
			}
			done, err := s.Store.MigrationDone(codexRepairMigration)
			if err != nil {
				return err
			}
			codexRepair = !done
			break
		}
	}

	type syncCursorSetter interface {
		SetSyncCursor(*int64)
	}
	type debugLoggerSetter interface {
		SetDebugLogger(*log.Logger)
	}
	debugOn := s.debugEnabled()
	type parseTarget struct {
		parser usage.Parser
		since  *time.Time
		full   bool
	}
	targets := make([]parseTarget, 0, len(parsers))
	for _, parser := range parsers {
		since := options.Since
		full := options.RebuildAll || options.RefreshFrom != nil || stale[parser.Name()] ||
			(parser.Name() == "codex" && codexRepair)
		var cursorMs *int64
		if full {
			since = nil
		} else {
			since, err = s.Store.DefaultSince(parser.Name(), options.Since)
			if err != nil {
				return err
			}
			if options.Since == nil {
				cursorMs, _ = s.Store.LastRefreshStartedMs(parser.Name())
			}
		}
		if setter, ok := parser.(syncCursorSetter); ok {
			setter.SetSyncCursor(cursorMs)
		}
		if debugOn && s.Logger != nil {
			if setter, ok := parser.(debugLoggerSetter); ok {
				setter.SetDebugLogger(s.Logger)
			}
		}
		targets = append(targets, parseTarget{parser, since, full})
	}

	// Capture refreshStarted before Parse so sessions updated during this scan
	// are never stamped earlier than the recorded cursor.
	refreshStarted := time.Now().UnixMilli()
	type parsed struct {
		target   parseTarget
		messages []provider.UnifiedMessage
		elapsed  time.Duration
		err      error
	}
	results := make([]parsed, len(targets))
	var wg sync.WaitGroup
	for i, target := range targets {
		wg.Add(1)
		go func(index int, target parseTarget) {
			defer wg.Done()
			t0 := time.Now()
			messages, err := target.parser.Parse(target.since)
			results[index] = parsed{target: target, messages: messages, elapsed: time.Since(t0), err: err}
		}(i, target)
	}
	wg.Wait()
	if s.Logger != nil {
		for _, r := range results {
			if debugOn || r.elapsed >= 200*time.Millisecond || r.err != nil {
				sinceStr := "all"
				if r.target.since != nil {
					sinceStr = r.target.since.Format("2006-01-02")
				}
				s.Logger.Printf("scan parse %s: %s msgs=%d since=%s full=%v err=%v",
					r.target.parser.Name(), r.elapsed.Round(time.Millisecond), len(r.messages), sinceStr, r.target.full, r.err)
			}
		}
	}

	recordsRefresh := options.Since == nil && options.RefreshFrom == nil
	for _, result := range results {
		if result.err != nil {
			continue
		}
		messages := result.messages
		if options.RefreshFrom != nil || options.RefreshTo != nil {
			messages = filterRange(messages, options.RefreshFrom, options.RefreshTo)
		}
		replaceSource := options.RebuildAll || options.RefreshFrom != nil || stale[result.target.parser.Name()] ||
			(result.target.parser.Name() == "codex" && codexRepair)
		preserveMissing := false
		if policy, ok := result.target.parser.(usage.SessionPreservingParser); ok {
			preserveMissing = policy.PreserveMissingSessions()
		}
		replaceParsedSessions := replaceSource && preserveMissing && options.RefreshFrom == nil
		if len(messages) == 0 {
			// An explicit rebuild or range refresh is authoritative: when the
			// source yields nothing, its old rows must go. A parser without
			// session-level replacement is the only case where an empty parse
			// can be applied; Codex rebuilds keep missing-source sessions.
			if (options.RebuildAll || options.RefreshFrom != nil) && !replaceParsedSessions {
				if _, err := s.Store.ReplaceSourceMessages(result.target.parser.Name(), options.RefreshFrom, options.RefreshTo, nil, cost); err != nil {
					return err
				}
			}
			if recordsRefresh {
				_ = s.Store.RecordRefreshStarted(result.target.parser.Name(), refreshStarted)
			}
			if result.target.parser.Name() == "codex" && result.target.full && codexRepair {
				if err := s.Store.MarkMigration(codexRepairMigration); err != nil {
					return err
				}
			}
			continue
		}
		ingestStart := time.Now()
		if replaceParsedSessions {
			_, err = s.Store.ReplaceParsedSessions(messages, cost)
		} else if replaceSource {
			_, err = s.Store.ReplaceSourceMessages(result.target.parser.Name(), options.RefreshFrom, options.RefreshTo, messages, cost)
		} else if result.target.parser.IncrementalMode() == usage.ReplaceChangedSessions {
			_, err = s.Store.ReplaceSessionsMessages(messages, cost)
		} else {
			_, err = s.Store.IngestMessages(messages, cost)
		}
		if s.Logger != nil {
			if d := time.Since(ingestStart); debugOn || d >= 200*time.Millisecond {
				s.Logger.Printf("scan ingest %s: %s msgs=%d replace=%v",
					result.target.parser.Name(), d.Round(time.Millisecond), len(messages), replaceSource)
			}
		}
		if err != nil {
			return err
		}
		if recordsRefresh {
			if err := s.Store.RecordRefreshStarted(result.target.parser.Name(), refreshStarted); err != nil {
				return err
			}
		}
		if result.target.parser.Name() == "codex" && result.target.full && codexRepair {
			if err := s.Store.MarkMigration(codexRepairMigration); err != nil {
				return err
			}
		}
	}
	if err := s.Report.SetScannedAt(time.Now().UnixMilli()); err != nil {
		return err
	}
	s.Report.Clear()
	return nil
}

func (s *Scanner) parsers(options Options, releases usage.ReleaseMap) []usage.Parser {
	if s.testParsers != nil {
		return s.testParsers
	}
	wanted := map[string]bool{}
	for _, name := range options.Providers {
		wanted[name] = true
	}
	var parsers []usage.Parser
	if wanted["claude"] {
		parsers = append(parsers, usage.NewClaudeParser())
	}
	if wanted["codex"] {
		parsers = append(parsers, usage.NewCodexParser())
	}
	if wanted["copilot"] {
		parsers = append(parsers, usage.NewCopilotParser())
	}
	if wanted["opencode"] {
		parsers = append(parsers, usage.NewOpenCodeParser())
	}
	if wanted["gemini"] {
		parsers = append(parsers, usage.NewGeminiParser())
	}
	if wanted["pi"] {
		parsers = append(parsers, usage.NewPiParser())
	}
	if wanted["antigravity"] && options.ScanAntigravity {
		parser := usage.NewAntigravityParser(filepath.Join(s.DataDir, "antigravity-cache"), releases)
		parser.RebuildCache = options.RebuildAll
		parsers = append(parsers, parser)
	}
	return parsers
}

func filterRange(messages []provider.UnifiedMessage, from, to *time.Time) []provider.UnifiedMessage {
	out := make([]provider.UnifiedMessage, 0, len(messages))
	for _, message := range messages {
		date, err := time.ParseInLocation("2006-01-02", message.Date, time.Local)
		if err != nil {
			continue
		}
		if from != nil && date.Before(*from) {
			continue
		}
		if to != nil && date.After(*to) {
			continue
		}
		out = append(out, message)
	}
	return out
}

// ProvidersFromConfig is the full usage source list, independent of the quota
// provider map.
func ProvidersFromConfig(cfg config.Config) []string {
	return []string{"claude", "codex", "copilot", "opencode", "gemini", "pi", "antigravity"}
}

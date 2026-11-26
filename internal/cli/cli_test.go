package cli

import (
	"bytes"
	"encoding/csv"
	"strings"
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/summary"
	"github.com/fangwangme/tokenpulse/internal/usage"
)

// Protects cli.md "CSV": model fields containing commas or quotes stay in one
// column.
func TestModelsCSVUsesEscaping(t *testing.T) {
	var buffer bytes.Buffer
	err := writeModelsCSV(&buffer, []summary.Model{{
		Model: `gpt,"quoted"`, Provider: "openai,inc", Source: "codex", Tokens: 10, Cost: 0.5,
		MessageCount: 1, SessionCount: 1, Percent: 100,
	}})
	if err != nil {
		t.Fatal(err)
	}
	records, err := csv.NewReader(strings.NewReader(buffer.String())).ReadAll()
	if err != nil {
		t.Fatal(err)
	}
	if len(records) != 2 || records[1][0] != `gpt,"quoted"` || records[1][1] != "openai,inc" || len(records[1]) != 8 {
		t.Fatalf("records = %#v", records)
	}
}

func TestDailyCSVUsesEscaping(t *testing.T) {
	var buffer bytes.Buffer
	err := writeDailyCSV(&buffer, []usage.DailyUsageRow{{Date: "2026-11-20", Source: "antigravity,legacy", TotalTokens: 10}})
	if err != nil {
		t.Fatal(err)
	}
	records, err := csv.NewReader(strings.NewReader(buffer.String())).ReadAll()
	if err != nil {
		t.Fatal(err)
	}
	if len(records) != 2 || records[1][1] != "antigravity,legacy" || len(records[1]) != 10 {
		t.Fatalf("records = %#v", records)
	}
}

// Protects cli.md "remote status": the timestamp is human-readable, not a bare
// epoch.
func TestDisplayEpoch(t *testing.T) {
	if got := displayEpoch(0); got != "never" {
		t.Fatalf("zero = %q", got)
	}
	stamp := time.Now().Add(-2 * time.Hour).UnixMilli()
	got := displayEpoch(stamp)
	if strings.Contains(got, "1795") || !strings.Contains(got, "ago") || !strings.Contains(got, ":") {
		t.Fatalf("display = %q", got)
	}
}

func TestSubcommandHelp(t *testing.T) {
	for name, run := range map[string]func([]string) error{
		"usage":   func(args []string) error { return runUsage(args, true) },
		"config":  runConfig,
		"init":    runInit,
		"remote":  runRemote,
		"service": runService,
	} {
		if err := run([]string{"--help"}); err != nil {
			t.Errorf("%s --help: %v", name, err)
		}
	}
}

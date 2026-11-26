package config

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Protects config.md "Config": a missing file is written with defaults.
func TestMissingFileGetsDefaults(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.toml")
	manager := NewWithPath(path)
	cfg, err := manager.Load()
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Version != Version || !cfg.Providers["claude"].Enabled || cfg.Display.AutoRefreshSecs != 900 ||
		cfg.Display.RefreshQuota || cfg.Keeper.Agents["claude"].Command == "" {
		t.Fatalf("defaults = %+v", cfg)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatal(err)
	}
}

// Protects config.md "Config": an older file is upgraded in place, keeping
// values and filling missing Keeper agents and commands.
func TestOlderFileIsUpgradedInPlace(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.toml")
	body := `version = 4

[providers.claude]
enabled = false

[display]
theme = "dark"
auto_refresh_secs = 60
refresh_quota = true

[keeper]
enabled = true

[keeper.agents.claude]
session_keeper_enabled = false
daily_wakeup_time = "08:15"
weekly_keeper_enabled = false
command = ""
prompt = "Hi"
`
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	cfg, err := NewWithPath(path).Load()
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Version != Version || cfg.Providers["claude"].Enabled || cfg.Display.Theme != ThemeDark ||
		cfg.Display.AutoRefreshSecs != 60 || !cfg.Display.RefreshQuota || !cfg.Keeper.Enabled || cfg.Keeper.Agents["claude"].DailyWakeupTime != "08:15" ||
		cfg.Keeper.Agents["claude"].SessionKeeperEnabled ||
		cfg.Keeper.Agents["claude"].Command == "" || cfg.Keeper.Agents["codex"].Command == "" {
		t.Fatalf("upgraded = %+v", cfg)
	}
	written, _ := os.ReadFile(path)
	if !strings.Contains(string(written), "version = 6") || !strings.Contains(string(written), "codex") {
		t.Fatalf("file = %s", written)
	}
}

// Protects config.md "Config": a file that does not parse is never replaced.
func TestInvalidFileIsNeverOverwritten(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.toml")
	body := []byte("version = [not toml")
	if err := os.WriteFile(path, body, 0o644); err != nil {
		t.Fatal(err)
	}
	manager := NewWithPath(path)
	if _, err := manager.Load(); err == nil {
		t.Fatal("invalid file loaded")
	}
	if err := manager.EnableProvider("claude"); err == nil {
		t.Fatal("enable accepted an invalid file")
	}
	if err := manager.DisableProvider("claude"); err == nil {
		t.Fatal("disable accepted an invalid file")
	}
	if _, err := manager.ToggleAgentSessionKeeper("claude"); err == nil {
		t.Fatal("toggle accepted an invalid file")
	}
	cfg := Default()
	if err := manager.Save(&cfg); err == nil {
		t.Fatal("save accepted an invalid file")
	}
	after, _ := os.ReadFile(path)
	if !bytes.Equal(body, after) {
		t.Fatal("invalid config was rewritten")
	}
}

// Protects config.md "Keys and defaults": an absent key keeps its default,
// while an explicit empty notification sound survives parsing normalized to
// the documented default.
func TestParseDefaults(t *testing.T) {
	cfg, err := Parse("[display]\nshow_empty_providers = true\n")
	if err != nil {
		t.Fatal(err)
	}
	if !cfg.Display.ShowEmptyProviders || cfg.Display.Theme != ThemeAuto ||
		cfg.Display.QuotaDisplayMode != QuotaRemaining || cfg.Display.AccountDisplay != AccountFull ||
		cfg.Display.NotificationLevel != NotificationSystem || cfg.Display.NotificationSound != "chime" {
		t.Fatalf("parsed defaults = %+v", cfg.Display)
	}
}

// Protects config.md "CLI values": quota provider order keeps known IDs once
// and appends missing providers in the default order.
func TestQuotaProviderOrder(t *testing.T) {
	ids := []string{"claude", "codex", "opencode", "antigravity"}
	wantDefault := []string{"claude", "codex", "opencode", "antigravity"}
	cfg := Default()
	if got := cfg.QuotaProviderOrder(ids); !sameStrings(got, wantDefault) {
		t.Fatalf("default order = %v", got)
	}

	cfg.Display.QuotaProviderOrder = []string{"antigravity", "opencode", "missing", "claude", "claude"}
	wantCustom := []string{"antigravity", "opencode", "claude", "codex"}
	if got := cfg.QuotaProviderOrder(ids); !sameStrings(got, wantCustom) {
		t.Fatalf("custom order = %v, want %v", got, wantCustom)
	}

	parsed, err := Parse("[display]\nquota_provider_order = [\"opencode\", \"claude\"]\n")
	if err != nil {
		t.Fatal(err)
	}
	wantParsed := []string{"opencode", "claude", "codex", "antigravity"}
	if got := parsed.QuotaProviderOrder(ids); !sameStrings(got, wantParsed) {
		t.Fatalf("parsed order = %v, want %v", got, wantParsed)
	}
}

func sameStrings(left, right []string) bool {
	if len(left) != len(right) {
		return false
	}
	for i := range left {
		if left[i] != right[i] {
			return false
		}
	}
	return true
}

// Protects config.md "CLI values": Keeper agents are only the documented three.
func TestKeeperDefaults(t *testing.T) {
	agents := DefaultKeeperAgents()
	for _, name := range []string{"claude", "codex", "antigravity"} {
		agent, ok := agents[name]
		if !ok || agent.Command == "" || agent.Prompt != "Hi" || agent.DailyWakeupTime != "10:30" {
			t.Fatalf("%s = %+v", name, agent)
		}
	}
}

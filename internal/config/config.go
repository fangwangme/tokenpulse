// Package config reads and writes config.toml, keeping every 0.6.x key,
// default and validation.
package config

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/BurntSushi/toml"

	"github.com/fangwangme/tokenpulse/internal/paths"
)

// Version is the config schema version; Load rewrites an older file in the
// current shape.
const Version = 6

var defaultQuotaProviderOrder = []string{"claude", "codex", "opencode", "antigravity"}

type Config struct {
	Version   uint32                    `toml:"version"`
	Providers map[string]ProviderConfig `toml:"providers"`
	Display   DisplayConfig             `toml:"display"`
	Keeper    KeeperConfig              `toml:"keeper"`
}

type ProviderConfig struct {
	Enabled bool    `toml:"enabled"`
	Path    *string `toml:"path,omitempty"`
}

type DisplayConfig struct {
	ShowEmptyProviders bool              `toml:"show_empty_providers"`
	Theme              ThemePreference   `toml:"theme"`
	QuotaDisplayMode   QuotaDisplayMode  `toml:"quota_display_mode"`
	QuotaProviderOrder []string          `toml:"quota_provider_order"`
	AutoRefreshSecs    uint32            `toml:"auto_refresh_secs"`
	AccountDisplay     AccountDisplay    `toml:"account_display"`
	ScanAntigravity    bool              `toml:"scan_antigravity"`
	RefreshQuota       bool              `toml:"refresh_quota"`
	NotificationLevel  NotificationLevel `toml:"notification_level"`
	NotificationSound  string            `toml:"notification_sound"`
}

type NotificationLevel string

const (
	NotificationOff      NotificationLevel = "off"
	NotificationInApp    NotificationLevel = "in_app"
	NotificationTerminal NotificationLevel = "terminal"
	NotificationSystem   NotificationLevel = "system"
)

func (n NotificationLevel) Label() string { return string(n) }

func (n NotificationLevel) DisplayLabel() string {
	if n == NotificationInApp {
		return "in-app (toast)"
	}
	return string(n)
}

func (n NotificationLevel) Next() NotificationLevel {
	switch n {
	case NotificationOff:
		return NotificationInApp
	case NotificationInApp:
		return NotificationTerminal
	case NotificationTerminal:
		return NotificationSystem
	default:
		return NotificationOff
	}
}

type AccountDisplay string

const (
	AccountNone AccountDisplay = "none"
	AccountPlan AccountDisplay = "plan"
	AccountFull AccountDisplay = "full"
)

func (a AccountDisplay) Label() string { return string(a) }

func (a AccountDisplay) Next() AccountDisplay {
	switch a {
	case AccountNone:
		return AccountPlan
	case AccountPlan:
		return AccountFull
	default:
		return AccountNone
	}
}

func ParseAccountDisplay(value string) (AccountDisplay, bool) {
	switch AccountDisplay(value) {
	case AccountNone, AccountPlan, AccountFull:
		return AccountDisplay(value), true
	}
	return "", false
}

func (a AccountDisplay) ShowsPlan() bool    { return a != AccountNone }
func (a AccountDisplay) ShowsAccount() bool { return a == AccountFull }

type ThemePreference string

const (
	ThemeAuto  ThemePreference = "auto"
	ThemeDark  ThemePreference = "dark"
	ThemeLight ThemePreference = "light"
)

func (t ThemePreference) Label() string { return string(t) }

func (t ThemePreference) Next() ThemePreference {
	switch t {
	case ThemeAuto:
		return ThemeDark
	case ThemeDark:
		return ThemeLight
	default:
		return ThemeAuto
	}
}

type QuotaDisplayMode string

const (
	QuotaUsed      QuotaDisplayMode = "used"
	QuotaRemaining QuotaDisplayMode = "remaining"
)

type KeeperConfig struct {
	Enabled           bool                         `toml:"enabled"`
	CheckIntervalSecs uint32                       `toml:"check_interval_secs"`
	Agents            map[string]AgentKeeperConfig `toml:"agents"`
}

type AgentKeeperConfig struct {
	SessionKeeperEnabled bool   `toml:"session_keeper_enabled"`
	DailyWakeupTime      string `toml:"daily_wakeup_time"`
	WeeklyKeeperEnabled  bool   `toml:"weekly_keeper_enabled"`
	Command              string `toml:"command"`
	Prompt               string `toml:"prompt"`
	WeeklyAllGroups      bool   `toml:"weekly_all_groups"`
}

// Values of the keeper_antigravity_groups setting.
const (
	AntigravityGroupsPrimary = "gemini_flash"
	AntigravityGroupsAll     = "all"
)

func (k KeeperConfig) AntigravityAllGroups() bool {
	agent, ok := k.Agents["antigravity"]
	return ok && agent.WeeklyAllGroups
}

func (k *KeeperConfig) SetAntigravityAllGroups(all bool) {
	if k.Agents == nil {
		k.Agents = map[string]AgentKeeperConfig{}
	}
	agent, ok := k.Agents["antigravity"]
	if !ok {
		agent = DefaultKeeperAgents()["antigravity"]
	}
	agent.WeeklyAllGroups = all
	k.Agents["antigravity"] = agent
}

func (k KeeperConfig) AntigravityGroupsLabel() string {
	if k.AntigravityAllGroups() {
		return AntigravityGroupsAll
	}
	return AntigravityGroupsPrimary
}

func Default() Config {
	return Config{
		Version: Version,
		Providers: map[string]ProviderConfig{
			"claude":      {Enabled: true},
			"codex":       {Enabled: true},
			"opencode":    {Enabled: true},
			"antigravity": {Enabled: true},
		},
		Display: DisplayConfig{
			ShowEmptyProviders: false,
			Theme:              ThemeAuto,
			QuotaDisplayMode:   QuotaRemaining,
			QuotaProviderOrder: append([]string(nil), defaultQuotaProviderOrder...),
			AutoRefreshSecs:    900,
			AccountDisplay:     AccountFull,
			ScanAntigravity:    true,
			RefreshQuota:       false,
			NotificationLevel:  NotificationSystem,
			NotificationSound:  "chime",
		},
		Keeper: KeeperConfig{
			Enabled:           false,
			CheckIntervalSecs: 60,
			Agents:            DefaultKeeperAgents(),
		},
	}
}

func DefaultKeeperAgents() map[string]AgentKeeperConfig {
	return map[string]AgentKeeperConfig{
		"claude": {
			SessionKeeperEnabled: true,
			DailyWakeupTime:      "10:30",
			WeeklyKeeperEnabled:  true,
			Command:              `claude -p "{prompt}" --model {model} --no-session-persistence`,
			Prompt:               "Hi",
		},
		"codex": {
			SessionKeeperEnabled: true,
			DailyWakeupTime:      "10:30",
			WeeklyKeeperEnabled:  true,
			Command:              `codex exec --skip-git-repo-check --ephemeral --model {model} "{prompt}"`,
			Prompt:               "Hi",
		},
		"antigravity": {
			SessionKeeperEnabled: true,
			DailyWakeupTime:      "10:30",
			WeeklyKeeperEnabled:  true,
			Command:              `agy --model {model} --prompt "{prompt}"`,
			Prompt:               "Hi",
		},
	}
}

// Manager is bound to one config file.
type Manager struct {
	Path string
}

// New returns a manager for TOKENPULSE_CONFIG_PATH or the data directory.
func New() (*Manager, error) {
	path, err := paths.ConfigPath()
	if err != nil {
		return nil, err
	}
	return &Manager{Path: path}, nil
}

func NewWithPath(path string) *Manager { return &Manager{Path: path} }

// KeeperStatePath is where the Keeper's last-fired bookkeeping lives.
func (m *Manager) KeeperStatePath() string {
	return filepath.Join(filepath.Dir(m.Path), "keeper_state.json")
}

func (m *Manager) Exists() bool {
	_, err := os.Stat(m.Path)
	return err == nil
}

// Load reads the config, writing defaults when it does not exist and upgrading
// an older version in place. A file that does not parse is an error: no caller
// may silently replace what the user wrote.
func (m *Manager) Load() (Config, error) {
	if !m.Exists() {
		cfg := Default()
		_ = m.Save(&cfg)
		return cfg, nil
	}
	content, err := os.ReadFile(m.Path)
	if err != nil {
		return Config{}, err
	}
	cfg, err := Parse(string(content))
	if err != nil {
		return Config{}, fmt.Errorf("invalid config file %s: %w", m.Path, err)
	}

	modified := false
	defaults := DefaultKeeperAgents()
	for name, def := range defaults {
		if existing, ok := cfg.Keeper.Agents[name]; ok {
			if existing.Command == "" {
				existing.Command = def.Command
				cfg.Keeper.Agents[name] = existing
				modified = true
			}
		} else {
			cfg.Keeper.Agents[name] = def
			modified = true
		}
	}
	if cfg.Version < Version || modified {
		cfg.Version = Version
		_ = m.Save(&cfg)
	}
	return cfg, nil
}

// Parse decodes TOML over the defaults, so an absent key keeps its default.
func Parse(content string) (Config, error) {
	cfg := Default()
	if _, err := toml.Decode(content, &cfg); err != nil {
		return Config{}, err
	}
	if cfg.Providers == nil {
		cfg.Providers = map[string]ProviderConfig{}
	}
	if cfg.Keeper.Agents == nil {
		cfg.Keeper.Agents = map[string]AgentKeeperConfig{}
	}
	normalizeDefaults(&cfg)
	return cfg, nil
}

// normalizeDefaults restores the one default a pre-filled TOML decode cannot
// distinguish from an explicit empty value.
func normalizeDefaults(cfg *Config) {
	if cfg.Display.NotificationSound == "" {
		cfg.Display.NotificationSound = "chime"
	}
	if len(cfg.Display.QuotaProviderOrder) == 0 {
		cfg.Display.QuotaProviderOrder = append([]string(nil), defaultQuotaProviderOrder...)
	}
}

// QuotaProviderOrder returns the configured display order normalized against
// the providers this build supports: unknown IDs are dropped, duplicates
// collapse, and missing providers append in the default order.
func (c Config) QuotaProviderOrder(quotaIDs []string) []string {
	known := make(map[string]bool, len(quotaIDs))
	for _, id := range quotaIDs {
		known[id] = true
	}
	seen := make(map[string]bool, len(quotaIDs))
	out := make([]string, 0, len(quotaIDs))
	for _, id := range c.Display.QuotaProviderOrder {
		if !known[id] || seen[id] {
			continue
		}
		seen[id] = true
		out = append(out, id)
	}
	for _, id := range quotaIDs {
		if !seen[id] {
			out = append(out, id)
			seen[id] = true
		}
	}
	return out
}

// Save writes the config, refusing to overwrite a file that does not parse.
func (m *Manager) Save(cfg *Config) error {
	if content, err := os.ReadFile(m.Path); err == nil {
		if _, perr := Parse(string(content)); perr != nil {
			return fmt.Errorf("%s has an error and was left unchanged; fix or remove it, then retry: %w", m.Path, perr)
		}
	} else if !errors.Is(err, os.ErrNotExist) {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(m.Path), 0o755); err != nil {
		return err
	}
	var b strings.Builder
	enc := toml.NewEncoder(&b)
	enc.Indent = "  "
	if err := enc.Encode(cfg); err != nil {
		return err
	}
	dir := filepath.Dir(m.Path)
	tmp, err := os.CreateTemp(dir, ".config-*.toml.tmp")
	if err != nil {
		return err
	}
	tmpPath := tmp.Name()
	cleanup := true
	defer func() {
		if cleanup {
			_ = os.Remove(tmpPath)
		}
	}()
	if _, err := tmp.WriteString(b.String()); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Chmod(0o644); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Sync(); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if err := os.Rename(tmpPath, m.Path); err != nil {
		return err
	}
	cleanup = false
	return nil
}

func (m *Manager) EnableProvider(provider string) error {
	cfg, err := m.Load()
	if err != nil {
		return err
	}
	p := cfg.Providers[provider]
	p.Enabled = true
	cfg.Providers[provider] = p
	return m.Save(&cfg)
}

func (m *Manager) DisableProvider(provider string) error {
	cfg, err := m.Load()
	if err != nil {
		return err
	}
	if p, ok := cfg.Providers[provider]; ok {
		p.Enabled = false
		cfg.Providers[provider] = p
	}
	return m.Save(&cfg)
}

func (m *Manager) ToggleAgentSessionKeeper(agent string) (bool, error) {
	return m.toggleAgentKeeper(agent, func(c *AgentKeeperConfig) *bool { return &c.SessionKeeperEnabled })
}

func (m *Manager) ToggleAgentWeeklyKeeper(agent string) (bool, error) {
	return m.toggleAgentKeeper(agent, func(c *AgentKeeperConfig) *bool { return &c.WeeklyKeeperEnabled })
}

func (m *Manager) toggleAgentKeeper(agent string, field func(*AgentKeeperConfig) *bool) (bool, error) {
	cfg, err := m.Load()
	if err != nil {
		return false, err
	}
	entry, ok := cfg.Keeper.Agents[agent]
	if !ok {
		entry = DefaultKeeperAgents()[agent]
	}
	flag := field(&entry)
	*flag = !*flag
	cfg.Keeper.Agents[agent] = entry
	return *flag, m.Save(&cfg)
}

// EnabledQuotaProviders lists enabled config entries that are quota providers,
// in config order. `providers` is free-form, so entries without a quota fetcher
// (a legacy `copilot`, a hand-written `gemini`) are ignored rather than shown.
func (c Config) EnabledQuotaProviders(quotaIDs []string) []string {
	var out []string
	for _, id := range quotaIDs {
		if p, ok := c.Providers[id]; ok && p.Enabled {
			out = append(out, id)
		}
	}
	return out
}

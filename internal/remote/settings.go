package remote

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
)

var soundCycle = []string{"chime", "Hero", "Glass", "Submarine", "none"}

type SettingSpec struct {
	Allowed []any
}

func phoneSettings() map[string]SettingSpec {
	specs := map[string]SettingSpec{
		"display.quota_display_mode":          {Allowed: []any{"used", "remaining"}},
		"display.auto_refresh_secs":           {Allowed: []any{60, 120, 300, 600, 900}},
		"display.account_display":             {Allowed: []any{"none", "plan", "full"}},
		"display.refresh_quota":               {Allowed: []any{true, false}},
		"display.scan_antigravity":            {Allowed: []any{true, false}},
		"display.show_empty_providers":        {Allowed: []any{true, false}},
		"display.notification_level":          {Allowed: []any{"off", "in_app", "terminal", "system"}},
		"display.notification_sound":          {Allowed: []any{"chime", "Hero", "Glass", "Submarine", "none"}},
		"keeper.enabled":                      {Allowed: []any{true, false}},
		"keeper.agents.agy.weekly_all_groups": {Allowed: []any{true, false}},
	}
	for _, id := range []string{"claude", "codex", "agy"} {
		specs["providers."+id+".enabled"] = SettingSpec{Allowed: []any{true, false}}
	}
	for _, agent := range []string{"claude", "codex", "agy"} {
		specs["keeper.agents."+agent+".session_keeper_enabled"] = SettingSpec{Allowed: []any{true, false}}
		specs["keeper.agents."+agent+".weekly_keeper_enabled"] = SettingSpec{Allowed: []any{true, false}}
		specs["keeper.agents."+agent+".daily_wakeup_time"] = SettingSpec{Allowed: []any{"HH:MM"}}
	}
	return specs
}

// RejectKeys are refused with 400 even alongside valid keys.
var rejectSuffixes = []string{".command", ".prompt", ".path"}

type SettingsSnapshot map[string]any

func currentSettings(cfg config.Config) SettingsSnapshot {
	return SettingsSnapshot{
		"display.quota_display_mode":                  string(cfg.Display.QuotaDisplayMode),
		"display.auto_refresh_secs":                   int(cfg.Display.AutoRefreshSecs),
		"display.account_display":                     string(cfg.Display.AccountDisplay),
		"display.refresh_quota":                       cfg.Display.RefreshQuota,
		"display.scan_antigravity":                    cfg.Display.ScanAntigravity,
		"display.show_empty_providers":                cfg.Display.ShowEmptyProviders,
		"display.notification_level":                  string(cfg.Display.NotificationLevel),
		"display.notification_sound":                  cfg.Display.NotificationSound,
		"keeper.enabled":                              cfg.Keeper.Enabled,
		"keeper.agents.agy.weekly_all_groups":         cfg.Keeper.AntigravityAllGroups(),
		"providers.claude.enabled":                    providerEnabled(cfg, "claude"),
		"providers.codex.enabled":                     providerEnabled(cfg, "codex"),
		"providers.agy.enabled":                       providerEnabled(cfg, "antigravity"),
		"keeper.agents.claude.session_keeper_enabled": agentKeeper(cfg, "claude").SessionKeeperEnabled,
		"keeper.agents.claude.weekly_keeper_enabled":  agentKeeper(cfg, "claude").WeeklyKeeperEnabled,
		"keeper.agents.claude.daily_wakeup_time":      agentKeeper(cfg, "claude").DailyWakeupTime,
		"keeper.agents.codex.session_keeper_enabled":  agentKeeper(cfg, "codex").SessionKeeperEnabled,
		"keeper.agents.codex.weekly_keeper_enabled":   agentKeeper(cfg, "codex").WeeklyKeeperEnabled,
		"keeper.agents.codex.daily_wakeup_time":       agentKeeper(cfg, "codex").DailyWakeupTime,
		"keeper.agents.agy.session_keeper_enabled":    agentKeeper(cfg, "antigravity").SessionKeeperEnabled,
		"keeper.agents.agy.weekly_keeper_enabled":     agentKeeper(cfg, "antigravity").WeeklyKeeperEnabled,
		"keeper.agents.agy.daily_wakeup_time":         agentKeeper(cfg, "antigravity").DailyWakeupTime,
	}
}

func providerEnabled(cfg config.Config, id string) bool {
	if provider, ok := cfg.Providers[id]; ok {
		return provider.Enabled
	}
	return false
}

func agentKeeper(cfg config.Config, agent string) config.AgentKeeperConfig {
	if value, ok := cfg.Keeper.Agents[agent]; ok {
		return value
	}
	return config.DefaultKeeperAgents()[agent]
}

// settingsResponse is the GET/PATCH answer.
type settingsResponse struct {
	Settings map[string]settingView `json:"settings"`
}

type settingView struct {
	Value   any   `json:"value"`
	Allowed []any `json:"allowed,omitempty"`
}

func settingsView(snapshot SettingsSnapshot) settingsResponse {
	specs := phoneSettings()
	out := settingsResponse{Settings: map[string]settingView{}}
	for key, value := range snapshot {
		out.Settings[key] = settingView{Value: value, Allowed: specs[key].Allowed}
	}
	return out
}

type settingChange struct {
	key   string
	value any
}

func validatePhoneUpdates(updates map[string]json.RawMessage) ([]settingChange, error) {
	if updates == nil {
		return nil, fmt.Errorf("no settings given")
	}
	specs := phoneSettings()
	var changes []settingChange
	for key, raw := range updates {
		for _, suffix := range rejectSuffixes {
			if strings.HasSuffix(key, suffix) {
				return nil, fmt.Errorf("setting %q cannot be changed from the phone", key)
			}
		}
		spec, ok := specs[key]
		if !ok {
			return nil, fmt.Errorf("unknown setting %q", key)
		}
		value := decodeValue(raw)
		if !allowed(spec, value) {
			return nil, fmt.Errorf("invalid value for %s", key)
		}
		changes = append(changes, settingChange{key, value})
	}
	return changes, nil
}

// ApplySettings validates every entry before writing anything. It returns the
// new snapshot.
func ApplySettings(manager *config.Manager, updates map[string]json.RawMessage) (SettingsSnapshot, error) {
	changes, err := validatePhoneUpdates(updates)
	if err != nil {
		return nil, err
	}
	cfg, err := manager.Load()
	if err != nil {
		return nil, err
	}
	for _, change := range changes {
		if err := applyOne(&cfg, change.key, change.value); err != nil {
			return nil, err
		}
	}
	if err := manager.Save(&cfg); err != nil {
		return nil, err
	}
	return currentSettings(cfg), nil
}

func decodeValue(raw json.RawMessage) any {
	var value any
	if err := json.Unmarshal(raw, &value); err != nil {
		return nil
	}
	return value
}

func allowed(spec SettingSpec, value any) bool {
	if len(spec.Allowed) == 1 && spec.Allowed[0] == "HH:MM" {
		text, ok := value.(string)
		if !ok {
			return false
		}
		_, err := time.Parse("15:04", text)
		return err == nil
	}
	for _, candidate := range spec.Allowed {
		switch want := candidate.(type) {
		case bool:
			if got, ok := value.(bool); ok && got == want {
				return true
			}
		case string:
			if got, ok := value.(string); ok && got == want {
				return true
			}
		case int:
			switch got := value.(type) {
			case float64:
				if got == float64(want) {
					return true
				}
			case int:
				if got == want {
					return true
				}
			}
		}
	}
	return false
}

func applyOne(cfg *config.Config, key string, value any) error {
	boolValue, _ := value.(bool)
	stringValue, _ := value.(string)
	switch {
	case key == "display.quota_display_mode":
		cfg.Display.QuotaDisplayMode = config.QuotaDisplayMode(stringValue)
	case key == "display.auto_refresh_secs":
		number := int(value.(float64))
		cfg.Display.AutoRefreshSecs = uint32(number)
	case key == "display.account_display":
		cfg.Display.AccountDisplay = config.AccountDisplay(stringValue)
	case key == "display.refresh_quota":
		cfg.Display.RefreshQuota = boolValue
	case key == "display.scan_antigravity":
		cfg.Display.ScanAntigravity = boolValue
	case key == "display.show_empty_providers":
		cfg.Display.ShowEmptyProviders = boolValue
	case key == "display.notification_level":
		cfg.Display.NotificationLevel = config.NotificationLevel(stringValue)
	case key == "display.notification_sound":
		cfg.Display.NotificationSound = stringValue
	case key == "keeper.enabled":
		cfg.Keeper.Enabled = boolValue
	case key == "keeper.agents.agy.weekly_all_groups":
		cfg.Keeper.SetAntigravityAllGroups(boolValue)
	case strings.HasPrefix(key, "providers."):
		id := strings.TrimSuffix(strings.TrimPrefix(key, "providers."), ".enabled")
		if id == "agy" {
			id = "antigravity"
		}
		provider := cfg.Providers[id]
		provider.Enabled = boolValue
		cfg.Providers[id] = provider
	case strings.HasPrefix(key, "keeper.agents."):
		parts := strings.Split(key, ".")
		if len(parts) != 4 {
			return fmt.Errorf("invalid setting %q", key)
		}
		agentName, field := parts[2], parts[3]
		if agentName == "agy" {
			agentName = "antigravity"
		}
		agent, ok := cfg.Keeper.Agents[agentName]
		if !ok {
			agent = config.DefaultKeeperAgents()[agentName]
		}
		switch field {
		case "session_keeper_enabled":
			agent.SessionKeeperEnabled = boolValue
		case "weekly_keeper_enabled":
			agent.WeeklyKeeperEnabled = boolValue
		case "daily_wakeup_time":
			agent.DailyWakeupTime = stringValue
		default:
			return fmt.Errorf("invalid setting %q", key)
		}
		cfg.Keeper.Agents[agentName] = agent
	default:
		return fmt.Errorf("unknown setting %q", key)
	}
	return nil
}

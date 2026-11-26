package runtime

import (
	"fmt"
	"strings"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/report"
)

// Client is the thin-frontend contract used by the TUI regardless of whether
// the runtime is hosted in-process or in a background serve daemon.
type Client interface {
	Status() (StatusSnapshot, error)
	Config() (config.Config, error)
	Usage(params report.Params) (report.Report, error)
	UsageDay(date string, agents []string) (report.DayDetail, error)
	UsageModel(model, from, to string, agents []string) (report.ModelDetail, error)
	Quota() (QuotaView, error)
	KeeperRecords(limit int) ([]keeper.ExecutionRecord, error)
	Refresh(usage, quota bool) (RefreshResult, error)
	PatchSettings(updates map[string]any) (config.Config, error)
	KeeperPing(agent, group string) error
	Subscribe() (<-chan Event, func(), error)
	IsRemote() bool
	Close() error
}

// InProcessClient talks directly to a *Runtime hosted in the same process.
type InProcessClient struct {
	Runtime *Runtime
	Lock    *Lock
}

func NewInProcessClient(rt *Runtime, lock *Lock) *InProcessClient {
	return &InProcessClient{Runtime: rt, Lock: lock}
}

func (c *InProcessClient) Status() (StatusSnapshot, error) {
	return c.Runtime.Status(), nil
}

func (c *InProcessClient) Config() (config.Config, error) {
	return c.Runtime.Config.Load()
}

func (c *InProcessClient) Usage(params report.Params) (report.Report, error) {
	return c.Runtime.Usage(params)
}

func (c *InProcessClient) UsageDay(date string, agents []string) (report.DayDetail, error) {
	return c.Runtime.UsageDay(date, agents)
}

func (c *InProcessClient) UsageModel(model, from, to string, agents []string) (report.ModelDetail, error) {
	return c.Runtime.UsageModel(model, from, to, agents)
}

func (c *InProcessClient) Quota() (QuotaView, error) {
	return c.Runtime.Quota(), nil
}

func (c *InProcessClient) KeeperRecords(limit int) ([]keeper.ExecutionRecord, error) {
	return c.Runtime.KeeperRecords(limit), nil
}

func (c *InProcessClient) Refresh(usage, quota bool) (RefreshResult, error) {
	return c.Runtime.RequestRefresh(usage, quota), nil
}

func (c *InProcessClient) PatchSettings(updates map[string]any) (config.Config, error) {
	cfg, err := c.Runtime.Config.Load()
	if err != nil {
		return config.Config{}, err
	}
	intervalChanged := false
	for key, value := range updates {
		if key == "display.auto_refresh_secs" {
			intervalChanged = true
		}
		if err := ApplyLocalSetting(&cfg, key, value); err != nil {
			return config.Config{}, err
		}
	}
	if err := c.Runtime.Config.Save(&cfg); err != nil {
		return config.Config{}, err
	}
	c.Runtime.NotifySettingsUpdated(cfg, intervalChanged)
	return cfg, nil
}

func (c *InProcessClient) KeeperPing(agent, group string) error {
	_, err := c.Runtime.TriggerKeeperPing(agent, group)
	return err
}

func (c *InProcessClient) Subscribe() (<-chan Event, func(), error) {
	ch, cancel := c.Runtime.Events.Subscribe()
	return ch, cancel, nil
}

func (c *InProcessClient) IsRemote() bool { return false }

func (c *InProcessClient) Close() error {
	if c.Runtime != nil {
		c.Runtime.Stop()
	}
	if c.Lock != nil {
		return c.Lock.Release()
	}
	return nil
}

// ApplyLocalSetting applies a setting key/value pair from local trusted clients
// (TUI / CLI) onto cfg.
func ApplyLocalSetting(cfg *config.Config, key string, value any) error {
	boolVal, _ := value.(bool)
	strVal, _ := value.(string)
	switch {
	case key == "display.theme":
		cfg.Display.Theme = config.ThemePreference(strVal)
	case key == "display.quota_display_mode":
		cfg.Display.QuotaDisplayMode = config.QuotaDisplayMode(strVal)
	case key == "display.auto_refresh_secs":
		switch n := value.(type) {
		case int:
			cfg.Display.AutoRefreshSecs = uint32(n)
		case uint32:
			cfg.Display.AutoRefreshSecs = n
		case float64:
			cfg.Display.AutoRefreshSecs = uint32(n)
		default:
			return fmt.Errorf("invalid value for %s", key)
		}
	case key == "display.account_display":
		cfg.Display.AccountDisplay = config.AccountDisplay(strVal)
	case key == "display.refresh_quota":
		cfg.Display.RefreshQuota = boolVal
	case key == "display.scan_antigravity":
		cfg.Display.ScanAntigravity = boolVal
	case key == "display.show_empty_providers":
		cfg.Display.ShowEmptyProviders = boolVal
	case key == "display.notification_level":
		cfg.Display.NotificationLevel = config.NotificationLevel(strVal)
	case key == "display.notification_sound":
		cfg.Display.NotificationSound = strVal
	case key == "keeper.enabled":
		cfg.Keeper.Enabled = boolVal
	case key == "keeper.agents.agy.weekly_all_groups", key == "keeper.agents.antigravity.weekly_all_groups":
		cfg.Keeper.SetAntigravityAllGroups(boolVal)
	case strings.HasPrefix(key, "providers."):
		id := strings.TrimSuffix(strings.TrimPrefix(key, "providers."), ".enabled")
		if id == "agy" {
			id = "antigravity"
		}
		p := cfg.Providers[id]
		p.Enabled = boolVal
		cfg.Providers[id] = p
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
			agent.SessionKeeperEnabled = boolVal
		case "weekly_keeper_enabled":
			agent.WeeklyKeeperEnabled = boolVal
		case "daily_wakeup_time":
			agent.DailyWakeupTime = strVal
		default:
			return fmt.Errorf("invalid setting %q", key)
		}
		cfg.Keeper.Agents[agentName] = agent
	default:
		return fmt.Errorf("unknown setting %q", key)
	}
	return nil
}

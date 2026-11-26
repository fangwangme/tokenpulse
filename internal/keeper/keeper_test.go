package keeper

import (
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
)

// Protects remote.md "Keeper": the scheduler, manual pings, completion and
// readers may run concurrently without touching Config or the trigger maps
// under different locks.
func TestEngineConcurrentStateAccess(t *testing.T) {
	cfg := config.Default().Keeper
	cfg.Enabled = true
	for agent, value := range cfg.Agents {
		value.SessionKeeperEnabled = true
		value.WeeklyKeeperEnabled = true
		value.DailyWakeupTime = "10:30"
		cfg.Agents[agent] = value
	}
	engine := NewEngine(cfg, filepath.Join(t.TempDir(), "keeper_state.json"))
	now := time.Date(2026, 11, 26, 11, 0, 0, 0, time.Local)

	var wg sync.WaitGroup
	for i := 0; i < 20; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_ = engine.CheckScheduled(nil, now)
			_ = engine.CheckQuota(nil, now)
			request, err := engine.Manual("claude", "")
			if err == nil {
				engine.Finish(request, ExecutionRecord{Success: true})
			}
			engine.InFlight("claude")
			_, _ = engine.WeeklyTriggered("claude")
			_ = engine.StateSnapshot()
			engine.UpdateConfig(cfg)
			engine.Persist()
		}()
	}
	wg.Wait()
}

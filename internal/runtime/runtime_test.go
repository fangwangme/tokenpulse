package runtime

import (
	"errors"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/scan"
)

func TestAcquireLockExclusivity(t *testing.T) {
	dir := t.TempDir()
	first, err := AcquireLock(dir)
	if err != nil {
		t.Fatalf("first AcquireLock: %v", err)
	}
	if _, err := AcquireLock(dir); !errors.Is(err, ErrLocked) {
		t.Fatalf("second AcquireLock = %v, want ErrLocked", err)
	}
	if err := first.Release(); err != nil {
		t.Fatalf("Release: %v", err)
	}
	second, err := AcquireLock(dir)
	if err != nil {
		t.Fatalf("AcquireLock after release: %v", err)
	}
	_ = second.Release()
}

func TestBusConcurrentPublishAndSubscribe(t *testing.T) {
	bus := NewBus()
	ch, cancel := bus.Subscribe()
	defer cancel()

	var wg sync.WaitGroup
	for i := 0; i < 4; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			for j := 0; j < 20; j++ {
				bus.Publish("usage_updated", map[string]any{"worker": id, "n": j})
			}
		}(i)
	}
	wg.Wait()

	select {
	case ev := <-ch:
		if ev.Type != "usage_updated" {
			t.Fatalf("event type = %q", ev.Type)
		}
	case <-time.After(time.Second):
		t.Fatal("expected event on subscriber channel")
	}
}

func TestRefreshGateCooldown(t *testing.T) {
	now := time.Now()
	allowU, allowQ, retry := RefreshGate(now, time.Time{}, time.Time{}, true, true)
	if !allowU || !allowQ || retry != 0 {
		t.Fatalf("initial gate = (%v, %v, %d)", allowU, allowQ, retry)
	}
	allowU, allowQ, retry = RefreshGate(now.Add(10*time.Second), now, now, true, false)
	if allowU || allowQ || retry < 49 || retry > 51 {
		t.Fatalf("throttled gate = (%v, %v, %d)", allowU, allowQ, retry)
	}
}

func TestInProcessClientPatchSettingsAndEvents(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("TOKENPULSE_DATA_DIR", dir)
	t.Setenv("TOKENPULSE_CONFIG_PATH", filepath.Join(dir, "config.toml"))
	manager, err := config.New()
	if err != nil {
		t.Fatal(err)
	}
	scanner, err := scan.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	rt, err := New(dir, manager, scanner, "standalone", nil)
	if err != nil {
		t.Fatal(err)
	}
	lock, err := AcquireLock(dir)
	if err != nil {
		t.Fatal(err)
	}
	client := NewInProcessClient(rt, lock)
	defer client.Close()

	events, cancel, err := client.Subscribe()
	if err != nil {
		t.Fatal(err)
	}
	defer cancel()

	cfg, err := client.PatchSettings(map[string]any{
		"display.theme":             "dark",
		"display.auto_refresh_secs": 120,
		"keeper.enabled":            true,
	})
	if err != nil {
		t.Fatalf("PatchSettings: %v", err)
	}
	if cfg.Display.Theme != config.ThemeDark || cfg.Display.AutoRefreshSecs != 120 || !cfg.Keeper.Enabled {
		t.Fatalf("unexpected cfg: %+v", cfg.Display)
	}
	select {
	case ev := <-events:
		if ev.Type != "settings_updated" {
			t.Fatalf("event = %+v", ev)
		}
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for settings_updated event")
	}
}

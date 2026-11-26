package runtime

import (
	"errors"
	"os"
	"os/exec"
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

func TestStartTriggersInitialRefresh(t *testing.T) {
	dir := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_DATA_HOME", filepath.Join(home, "xdg"))
	t.Setenv("TOKENPULSE_ENV", "test")
	t.Setenv("TOKENPULSE_DATA_DIR", dir)
	t.Setenv("TOKENPULSE_CONFIG_PATH", filepath.Join(dir, "config.toml"))

	manager, err := config.New()
	if err != nil {
		t.Fatal(err)
	}
	cfg := config.Default()
	cfg.Display.RefreshQuota = false
	cfg.Display.ScanAntigravity = false
	if err := manager.Save(&cfg); err != nil {
		t.Fatal(err)
	}
	scanner, err := scan.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	scanner.Pricing.Fetch = func(string) ([]byte, error) { return []byte(`{}`), nil }
	rt, err := New(dir, manager, scanner, "daemon", nil)
	if err != nil {
		t.Fatal(err)
	}
	rt.Start(StartOptions{InitialRefresh: true, AutoRefresh: true})
	defer rt.Stop()

	select {
	case <-rt.firstScan:
	case <-time.After(5 * time.Second):
		t.Fatal("Start(InitialRefresh: true) did not run initial scan")
	}
}

func TestNoScanDoesNotStartRefreshLoop(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("TOKENPULSE_ENV", "test")
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
	rt.Start(StartOptions{InitialRefresh: false, AutoRefresh: false})
	defer rt.Stop()

	time.Sleep(150 * time.Millisecond)
	rt.mu.Lock()
	ran := !rt.lastUsageRefresh.IsZero() || !rt.lastQuotaRefresh.IsZero()
	rt.mu.Unlock()
	if ran {
		t.Fatal("no-scan mode unexpectedly ran a refresh")
	}
}

func TestStopWaitsForInFlightRefresh(t *testing.T) {
	dir := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_DATA_HOME", filepath.Join(home, "xdg"))
	t.Setenv("TOKENPULSE_ENV", "test")
	t.Setenv("TOKENPULSE_DATA_DIR", dir)
	t.Setenv("TOKENPULSE_CONFIG_PATH", filepath.Join(dir, "config.toml"))

	manager, err := config.New()
	if err != nil {
		t.Fatal(err)
	}
	cfg := config.Default()
	cfg.Display.RefreshQuota = false
	cfg.Display.ScanAntigravity = false
	_ = manager.Save(&cfg)
	scanner, err := scan.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	scanner.Pricing.Fetch = func(string) ([]byte, error) { return []byte(`{}`), nil }
	rt, err := New(dir, manager, scanner, "standalone", nil)
	if err != nil {
		t.Fatal(err)
	}

	// Block refreshMu so RequestRefresh enters execWG and waits in-flight.
	rt.refreshMu.Lock()
	reqStarted := make(chan struct{})
	go func() {
		close(reqStarted)
		rt.RequestRefresh(true, false)
	}()
	<-reqStarted
	time.Sleep(40 * time.Millisecond)

	stopDone := make(chan struct{})
	go func() {
		rt.Stop()
		close(stopDone)
	}()

	select {
	case <-stopDone:
		t.Fatal("Stop returned while in-flight RequestRefresh was still active")
	case <-time.After(80 * time.Millisecond):
	}
	rt.refreshMu.Unlock()

	select {
	case <-stopDone:
	case <-time.After(3 * time.Second):
		t.Fatal("Stop did not return after in-flight refresh finished")
	}
}

func TestStopConcurrentWithRefreshAndKeeper(t *testing.T) {
	dir := t.TempDir()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_DATA_HOME", filepath.Join(home, "xdg"))
	t.Setenv("TOKENPULSE_ENV", "test")
	t.Setenv("TOKENPULSE_DATA_DIR", dir)
	t.Setenv("TOKENPULSE_CONFIG_PATH", filepath.Join(dir, "config.toml"))

	manager, err := config.New()
	if err != nil {
		t.Fatal(err)
	}
	cfg := config.Default()
	cfg.Display.RefreshQuota = false
	cfg.Display.ScanAntigravity = false
	for k, agent := range cfg.Keeper.Agents {
		agent.Command = "true"
		cfg.Keeper.Agents[k] = agent
	}
	_ = manager.Save(&cfg)
	scanner, err := scan.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	scanner.Pricing.Fetch = func(string) ([]byte, error) { return []byte(`{}`), nil }
	rt, err := New(dir, manager, scanner, "standalone", nil)
	if err != nil {
		t.Fatal(err)
	}
	rt.Start(StartOptions{InitialRefresh: false, AutoRefresh: false})

	var callers sync.WaitGroup
	for i := 0; i < 4; i++ {
		callers.Add(1)
		go func() {
			defer callers.Done()
			rt.RequestRefresh(true, false)
			_, _ = rt.TriggerKeeperPing("claude", "")
		}()
	}
	time.Sleep(10 * time.Millisecond)
	rt.Stop()
	callers.Wait()

	if _, err := rt.TriggerKeeperPing("claude", ""); err == nil {
		t.Fatal("TriggerKeeperPing accepted a ping after Stop")
	}
	if rt.Keeper.InFlight("claude") {
		t.Fatal("keeper still in flight after Stop returned")
	}
}

func TestStopWaitsForInFlightRepriceAndRejectsAfterStop(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("TOKENPULSE_ENV", "test")
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
	scanner.Pricing.Fetch = func(string) ([]byte, error) { return []byte(`{}`), nil }
	rt, err := New(dir, manager, scanner, "standalone", nil)
	if err != nil {
		t.Fatal(err)
	}

	rt.refreshMu.Lock()
	started := make(chan struct{})
	go func() {
		close(started)
		_, _ = rt.Reprice(false, false, "")
	}()
	<-started
	time.Sleep(40 * time.Millisecond)

	stopDone := make(chan struct{})
	go func() {
		rt.Stop()
		close(stopDone)
	}()

	select {
	case <-stopDone:
		t.Fatal("Stop returned while Reprice was still in flight")
	case <-time.After(80 * time.Millisecond):
	}
	rt.refreshMu.Unlock()

	select {
	case <-stopDone:
	case <-time.After(3 * time.Second):
		t.Fatal("Stop did not return after Reprice finished")
	}

	if _, err := rt.Reprice(false, false, ""); err == nil || err.Error() != "runtime is stopping" {
		t.Fatalf("Reprice after Stop = %v, want runtime is stopping", err)
	}
	if _, err := rt.UpdateSettings(func(*config.Config) error { return nil }, false); err == nil || err.Error() != "runtime is stopping" {
		t.Fatalf("UpdateSettings after Stop = %v, want runtime is stopping", err)
	}
}

func TestCrossProcessLockContention(t *testing.T) {
	dir := t.TempDir()
	lock, err := AcquireLock(dir)
	if err != nil {
		t.Fatalf("AcquireLock: %v", err)
	}
	cmd := exec.Command(os.Args[0], "-test.run=TestLockHelperProcess")
	cmd.Env = append(os.Environ(), "TOKENPULSE_LOCK_HELPER_DIR="+dir)
	out, err := cmd.CombinedOutput()
	if err == nil {
		t.Fatalf("child process unexpectedly acquired lock: %s", out)
	}
	if err := lock.Release(); err != nil {
		t.Fatal(err)
	}
	cmd2 := exec.Command(os.Args[0], "-test.run=TestLockHelperProcess")
	cmd2.Env = append(os.Environ(), "TOKENPULSE_LOCK_HELPER_DIR="+dir)
	if out, err := cmd2.CombinedOutput(); err != nil {
		t.Fatalf("child process failed to acquire released lock: %v (%s)", err, out)
	}
}

func TestLockHelperProcess(t *testing.T) {
	dir := os.Getenv("TOKENPULSE_LOCK_HELPER_DIR")
	if dir == "" {
		return
	}
	lock, err := AcquireLock(dir)
	if err != nil {
		os.Exit(2)
	}
	_ = lock.Release()
}

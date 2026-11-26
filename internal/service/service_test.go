package service

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/remote"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
	"github.com/fangwangme/tokenpulse/internal/scan"
)

func TestInstallRefusesTestRun(t *testing.T) {
	t.Setenv("TOKENPULSE_ENV", "test")
	if err := Install(); err == nil {
		t.Fatal("Install succeeded in a test run")
	}
}

func TestUnitQuotesPathsAndIncludesConfig(t *testing.T) {
	unit := systemdUnit("/Applications/Token Pulse/tokenpulse", "/home/me/Token Pulse", "/home/me/Token Pulse/config.toml", "")
	if !strings.Contains(unit, `ExecStart="/Applications/Token Pulse/tokenpulse" serve`) {
		t.Fatalf("unquoted ExecStart:\n%s", unit)
	}
	if !strings.Contains(unit, `Environment="TOKENPULSE_DATA_DIR=/home/me/Token Pulse"`) ||
		!strings.Contains(unit, `Environment="TOKENPULSE_CONFIG_PATH=/home/me/Token Pulse/config.toml"`) {
		t.Fatalf("unquoted environment:\n%s", unit)
	}
}

func TestLaunchdPlistEscapesAndIncludesConfig(t *testing.T) {
	plist := launchdPlist("/tmp/A&B/tokenpulse", "/tmp/A&B/data", "/tmp/A&B/config.toml", "test")
	if strings.Contains(plist, "A&B") {
		t.Fatalf("unescaped ampersand:\n%s", plist)
	}
	for _, want := range []string{"A&amp;B", "TOKENPULSE_CONFIG_PATH", "/tmp/A&amp;B/data/log/serve.out"} {
		if !strings.Contains(plist, want) {
			t.Fatalf("missing %q in:\n%s", want, plist)
		}
	}
}

func TestPrepareRuntimeForInstallRefusesStandalone(t *testing.T) {
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
	engine, err := rt.New(dir, manager, scanner, "standalone", nil)
	if err != nil {
		t.Fatal(err)
	}
	stopSocket, err := remote.StartLocalSocket(engine, dir)
	if err != nil {
		t.Fatal(err)
	}
	defer stopSocket()

	err = prepareRuntimeForInstall(dir)
	if err == nil || !strings.Contains(err.Error(), "close it before running") {
		t.Fatalf("prepareRuntimeForInstall(standalone) = %v, want TUI close error", err)
	}
}

// Package service installs serve as a user service.
package service

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/fangwangme/tokenpulse/internal/paths"
	"github.com/fangwangme/tokenpulse/internal/remote"
	"github.com/fangwangme/tokenpulse/internal/version"
)

func unitPath() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".config", "systemd", "user", "tokenpulse.service"), nil
}

func launchdPath() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, "Library", "LaunchAgents", "com.tokenpulse.serve.plist"), nil
}

// Install writes and enables the user service.
func Install() error {
	if version.IsTest() {
		return errors.New("refusing to install the user service from a test run; run without TOKENPULSE_ENV=test")
	}
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	dataDir, err := paths.DataDir()
	if err != nil {
		return err
	}
	configPath := os.Getenv("TOKENPULSE_CONFIG_PATH")
	environment := os.Getenv("TOKENPULSE_ENV")
	switch runtime.GOOS {
	case "linux":
		path, err := unitPath()
		if err != nil {
			return err
		}
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return err
		}
		unit := systemdUnit(executable, dataDir, configPath, environment)
		if err := os.WriteFile(path, []byte(unit), 0o644); err != nil {
			return err
		}
		if output, err := exec.Command("systemctl", "--user", "daemon-reload").CombinedOutput(); err != nil {
			return fmt.Errorf("systemctl daemon-reload: %s", strings.TrimSpace(string(output)))
		}
		if output, err := exec.Command("systemctl", "--user", "enable", "tokenpulse.service").CombinedOutput(); err != nil {
			return fmt.Errorf("systemctl enable: %s", strings.TrimSpace(string(output)))
		}
		// Restart an already-running unit so an upgrade takes effect.
		if output, err := exec.Command("systemctl", "--user", "restart", "tokenpulse.service").CombinedOutput(); err != nil {
			return fmt.Errorf("systemctl restart: %s", strings.TrimSpace(string(output)))
		}
		return nil
	case "darwin":
		path, err := launchdPath()
		if err != nil {
			return err
		}
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return err
		}
		if err := os.MkdirAll(filepath.Join(dataDir, "log"), 0o755); err != nil {
			return err
		}
		plist := launchdPlist(executable, dataDir, configPath, environment)
		if err := os.WriteFile(path, []byte(plist), 0o644); err != nil {
			return err
		}
		_ = exec.Command("launchctl", "bootout", "gui/"+currentUID(), path).Run()
		if output, err := exec.Command("launchctl", "bootstrap", "gui/"+currentUID(), path).CombinedOutput(); err != nil {
			return fmt.Errorf("launchctl bootstrap: %s", strings.TrimSpace(string(output)))
		}
		return nil
	default:
		return errors.New("service install is supported on Linux and macOS only")
	}
}

func systemdUnit(executable, dataDir, configPath, environment string) string {
	var environmentLines strings.Builder
	write := func(key, value string) {
		if value != "" {
			fmt.Fprintf(&environmentLines, "Environment=%s\n", systemdQuote(key+"="+value))
		}
	}
	write("TOKENPULSE_DATA_DIR", dataDir)
	write("TOKENPULSE_CONFIG_PATH", configPath)
	write("TOKENPULSE_ENV", environment)
	return fmt.Sprintf(`[Unit]
Description=TokenPulse remote server
After=network-online.target

[Service]
Type=simple
ExecStart=%s serve
Restart=on-failure
%s
[Install]
WantedBy=default.target
`, systemdQuote(executable), environmentLines.String())
}

func systemdQuote(value string) string {
	escaped := strings.NewReplacer("\\", "\\\\", `"`, `\"`).Replace(value)
	return `"` + escaped + `"`
}

func launchdPlist(executable, dataDir, configPath, environment string) string {
	escaped := func(value string) string {
		return strings.NewReplacer(
			"&", "&amp;", "<", "&lt;", ">", "&gt;", `"`, "&quot;", "'", "&apos;",
		).Replace(value)
	}
	var variables strings.Builder
	write := func(key, value string) {
		if value != "" {
			fmt.Fprintf(&variables, "<key>%s</key><string>%s</string>", key, escaped(value))
		}
	}
	write("TOKENPULSE_DATA_DIR", dataDir)
	write("TOKENPULSE_CONFIG_PATH", configPath)
	write("TOKENPULSE_ENV", environment)
	logDir := escaped(filepath.Join(dataDir, "log"))
	return fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.tokenpulse.serve</string>
  <key>ProgramArguments</key><array><string>%s</string><string>serve</string></array>
  <key>EnvironmentVariables</key><dict>%s</dict>
  <key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>%s/serve.out</string>
  <key>StandardErrorPath</key><string>%s/serve.err</string>
</dict></plist>
`, escaped(executable), variables.String(), logDir, logDir)
}

// Uninstall stops and removes the user service.
func Uninstall() error {
	switch runtime.GOOS {
	case "linux":
		path, err := unitPath()
		if err != nil {
			return err
		}
		_ = exec.Command("systemctl", "--user", "stop", "tokenpulse.service").Run()
		_ = exec.Command("systemctl", "--user", "disable", "tokenpulse.service").Run()
		if err := os.Remove(path); err != nil && !errors.Is(err, os.ErrNotExist) {
			return err
		}
		_ = exec.Command("systemctl", "--user", "daemon-reload").Run()
		return nil
	case "darwin":
		path, err := launchdPath()
		if err != nil {
			return err
		}
		_ = exec.Command("launchctl", "bootout", "gui/"+currentUID(), path).Run()
		if err := os.Remove(path); err != nil && !errors.Is(err, os.ErrNotExist) {
			return err
		}
		return nil
	default:
		return errors.New("service uninstall is supported on Linux and macOS only")
	}
}

// Status reports the unit and whether serve answers on the socket.
func Status() error {
	dataDir, err := paths.DataDir()
	if err != nil {
		return err
	}
	switch runtime.GOOS {
	case "linux":
		path, err := unitPath()
		if err != nil {
			return err
		}
		if _, err := os.Stat(path); err == nil {
			fmt.Println("Unit: installed")
		} else {
			fmt.Println("Unit: not installed")
		}
	case "darwin":
		path, err := launchdPath()
		if err != nil {
			return err
		}
		if _, err := os.Stat(path); err == nil {
			fmt.Println("Agent: installed")
		} else {
			fmt.Println("Agent: not installed")
		}
	}
	client := remote.NewLocalClient(dataDir)
	if _, err := client.State(); err != nil {
		fmt.Println("Serve: not running")
	} else {
		fmt.Println("Serve: running")
	}
	return nil
}

func currentUID() string {
	return fmt.Sprintf("%d", os.Getuid())
}

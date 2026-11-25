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
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	dataDir, err := paths.DataDir()
	if err != nil {
		return err
	}
	environment := fmt.Sprintf("Environment=TOKENPULSE_DATA_DIR=%s", dataDir)
	if value := os.Getenv("TOKENPULSE_CONFIG_PATH"); value != "" {
		environment += fmt.Sprintf("\nEnvironment=TOKENPULSE_CONFIG_PATH=%s", value)
	}
	if value := os.Getenv("TOKENPULSE_ENV"); value != "" {
		environment += fmt.Sprintf("\nEnvironment=TOKENPULSE_ENV=%s", value)
	}
	switch runtime.GOOS {
	case "linux":
		path, err := unitPath()
		if err != nil {
			return err
		}
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return err
		}
		unit := fmt.Sprintf(`[Unit]
Description=TokenPulse remote server
After=network-online.target

[Service]
Type=simple
ExecStart=%s serve
Restart=on-failure
%s

[Install]
WantedBy=default.target
`, executable, environment)
		if err := os.WriteFile(path, []byte(unit), 0o644); err != nil {
			return err
		}
		if output, err := exec.Command("systemctl", "--user", "daemon-reload").CombinedOutput(); err != nil {
			return fmt.Errorf("systemctl daemon-reload: %s", strings.TrimSpace(string(output)))
		}
		if output, err := exec.Command("systemctl", "--user", "enable", "--now", "tokenpulse.service").CombinedOutput(); err != nil {
			return fmt.Errorf("systemctl enable: %s", strings.TrimSpace(string(output)))
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
		plist := fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.tokenpulse.serve</string>
  <key>ProgramArguments</key><array><string>%s</string><string>serve</string></array>
  <key>EnvironmentVariables</key><dict><key>TOKENPULSE_DATA_DIR</key><string>%s</string></dict>
  <key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>%s/log/serve.out</string>
  <key>StandardErrorPath</key><string>%s/log/serve.err</string>
</dict></plist>
`, executable, dataDir, dataDir, dataDir)
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

// Package notification announces quota recoveries: sound, terminal escape
// sequences and native system banners.
package notification

import (
	_ "embed"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
)

//go:embed quota-restored.wav
var chime []byte

const (
	SOUNDChime = "chime"
	SOUNDNone  = "none"
)

// Recovery is one window that transitioned from exhausted back to available.
type Recovery struct {
	Provider         string  `json:"provider"`
	WindowLabel      string  `json:"windowLabel"`
	RemainingPercent float64 `json:"remainingPercent"`
}

// NotifyQuotaRestored announces a batch at the configured level.
func NotifyQuotaRestored(level config.NotificationLevel, sound string, events []Recovery) {
	if level == config.NotificationOff || len(events) == 0 {
		return
	}
	message := Summarize(events)
	PlayAlertSound(sound)
	if level == config.NotificationTerminal || level == config.NotificationSystem {
		SendTerminalNotification(message)
	}
	if level == config.NotificationSystem {
		SendSystemNotification(message)
	}
}

// Summarize is the notification text for a batch.
func Summarize(events []Recovery) string {
	switch len(events) {
	case 0:
		return ""
	case 1:
		return fmt.Sprintf("%s %s quota restored (%.0f%% remaining)",
			events[0].Provider, events[0].WindowLabel, events[0].RemainingPercent)
	default:
		return fmt.Sprintf("%d quota windows restored", len(events))
	}
}

// PlayAlertSound plays a sound on a background goroutine.
func PlayAlertSound(sound string) {
	sound = strings.TrimSpace(sound)
	if sound == "" || strings.EqualFold(sound, SOUNDNone) {
		return
	}
	go func() {
		if runtime.GOOS == "darwin" {
			path := resolveSoundPath(sound)
			if path == "" {
				return
			}
			_ = exec.Command("afplay", path).Run()
			return
		}
		_, _ = os.Stdout.WriteString("\x07")
	}()
}

func resolveSoundPath(sound string) string {
	if !strings.EqualFold(sound, SOUNDChime) {
		system := fmt.Sprintf("/System/Library/Sounds/%s.aiff", sound)
		if info, err := os.Stat(system); err == nil && !info.IsDir() {
			return system
		}
	}
	path := fmt.Sprintf("%s/tokenpulse-quota-restored.wav", os.TempDir())
	if info, err := os.Stat(path); err == nil && info.Size() == int64(len(chime)) {
		return path
	}
	staging := fmt.Sprintf("%s.%d.tmp", path, os.Getpid())
	if err := os.WriteFile(staging, chime, 0o644); err != nil {
		return ""
	}
	if err := os.Rename(staging, path); err != nil {
		return ""
	}
	return path
}

// SendTerminalNotification rings the terminal bell and emits OSC 9.
func SendTerminalNotification(message string) {
	_, _ = os.Stdout.WriteString("\x07")
	_, _ = os.Stdout.WriteString("\x1b]9;TokenPulse: " + message + "\x1b\\")
}

// SendSystemNotification shows a macOS Notification Center banner.
func SendSystemNotification(message string) {
	go func() {
		if runtime.GOOS != "darwin" {
			return
		}
		script := fmt.Sprintf("display notification \"%s\" with title \"TokenPulse\" subtitle \"Quota Restored\"",
			escapeAppleScript(message))
		_ = exec.Command("osascript", "-e", script).Run()
	}()
}

func escapeAppleScript(value string) string {
	return strings.NewReplacer("\\", "\\\\", "\"", "\\\"").Replace(value)
}

// TestNotification sends a sample recovery at the configured level.
func TestNotification(level config.NotificationLevel, sound string) {
	if level == config.NotificationOff {
		return
	}
	NotifyQuotaRestored(level, sound, []Recovery{{Provider: "claude", WindowLabel: "5h", RemainingPercent: 100}})
	time.Sleep(2500 * time.Millisecond)
}

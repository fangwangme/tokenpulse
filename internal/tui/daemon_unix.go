//go:build unix

package tui

import (
	"os"
	"os/exec"
	"syscall"
	"time"

	"github.com/fangwangme/tokenpulse/internal/remote"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
)

func startDetachedServe(dataDir string) (*remote.LocalClient, error) {
	executable, err := os.Executable()
	if err != nil {
		return nil, err
	}
	cmd := exec.Command(executable, "serve", "--quiet")
	cmd.Stdin = nil
	cmd.Stdout = nil
	cmd.Stderr = nil
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	_ = cmd.Process.Release()
	return remote.WaitForSocket(dataDir, 5*time.Second)
}

func stopDaemonAndAcquire(dataDir string) (*rt.Lock, error) {
	local := remote.NewLocalClient(dataDir)
	if err := local.Shutdown(); err != nil {
		return nil, err
	}
	deadline := time.Now().Add(5 * time.Second)
	var lastErr error
	for {
		lock, err := rt.AcquireLock(dataDir)
		if err == nil {
			return lock, nil
		}
		lastErr = err
		if time.Now().After(deadline) {
			return nil, lastErr
		}
		time.Sleep(50 * time.Millisecond)
	}
}

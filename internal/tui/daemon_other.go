//go:build !unix

package tui

import (
	"errors"

	"github.com/fangwangme/tokenpulse/internal/remote"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
)

func startDetachedServe(dataDir string) (*remote.LocalClient, error) {
	return nil, errors.New("starting background serve from TUI is supported on Linux and macOS")
}

func stopDaemonAndAcquire(dataDir string) (*rt.Lock, error) {
	return nil, errors.New("stopping background serve from TUI is supported on Linux and macOS")
}

package tui

import (
	"errors"
	"time"

	"github.com/fangwangme/tokenpulse/internal/remote"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
)

// attachClient connects to an existing serve daemon when runtime.lock is held,
// or starts an in-process Runtime otherwise.
func attachClient(opts Options) (rt.Client, error) {
	lock, err := rt.AcquireLock(opts.DataDir)
	if err != nil {
		if errors.Is(err, rt.ErrLocked) {
			local, waitErr := remote.WaitForSocket(opts.DataDir, 2*time.Second)
			if waitErr != nil {
				return nil, waitErr
			}
			return remote.NewSocketClient(local), nil
		}
		return nil, err
	}
	return newInProcess(opts, lock, !opts.NoScan)
}

func newInProcess(opts Options, lock *rt.Lock, initialRefresh bool) (rt.Client, error) {
	engine, err := rt.New(opts.DataDir, opts.ConfigManager, opts.Scanner, "standalone", nil)
	if err != nil {
		_ = lock.Release()
		return nil, err
	}
	engine.WarmCache()
	engine.Start(initialRefresh)
	return rt.NewInProcessClient(engine, lock), nil
}

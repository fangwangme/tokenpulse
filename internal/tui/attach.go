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
	return newInProcess(opts, lock, !opts.NoScan, !opts.NoScan)
}

func newInProcess(opts Options, lock *rt.Lock, initialRefresh, autoRefresh bool) (rt.Client, error) {
	engine, err := rt.New(opts.DataDir, opts.ConfigManager, opts.Scanner, "standalone", nil)
	if err != nil {
		_ = lock.Release()
		return nil, err
	}
	stopSocket, sockErr := remote.StartLocalSocket(engine, opts.DataDir)
	if sockErr != nil {
		engine.SetWebState("", "local socket unavailable: "+sockErr.Error())
	}
	engine.WarmCache()
	engine.Start(rt.StartOptions{InitialRefresh: initialRefresh, AutoRefresh: autoRefresh})
	client := rt.NewInProcessClient(engine, lock)
	if sockErr == nil {
		client.OnClose = stopSocket
	}
	return client, nil
}

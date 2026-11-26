//go:build unix

package runtime

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"syscall"
)

// ErrLocked is returned when another TokenPulse runtime already holds the
// exclusive lock for a data directory.
var ErrLocked = errors.New("another tokenpulse runtime owns this data directory")

// Lock represents an OS-managed exclusive flock on <dataDir>/runtime.lock.
// The kernel automatically releases the lock if the owning process crashes.
type Lock struct {
	Path string
	file *os.File
}

// AcquireLock takes a non-blocking exclusive lock for dataDir.
func AcquireLock(dataDir string) (*Lock, error) {
	if err := os.MkdirAll(dataDir, 0o700); err != nil {
		return nil, err
	}
	path := filepath.Join(dataDir, "runtime.lock")
	file, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		file.Close()
		if errors.Is(err, syscall.EWOULDBLOCK) || errors.Is(err, syscall.EAGAIN) {
			return nil, ErrLocked
		}
		return nil, err
	}
	_ = file.Truncate(0)
	_, _ = file.Seek(0, 0)
	_, _ = fmt.Fprintf(file, "%d\n", os.Getpid())
	_ = file.Sync()
	return &Lock{Path: path, file: file}, nil
}

// Release unlocks and closes the lock file.
func (l *Lock) Release() error {
	if l == nil || l.file == nil {
		return nil
	}
	file := l.file
	l.file = nil
	_ = syscall.Flock(int(file.Fd()), syscall.LOCK_UN)
	return file.Close()
}

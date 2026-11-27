// Package paths resolves every data file the program uses.
package paths

import (
	"errors"
	"os"
	"path/filepath"
	"sync"
)

var dirMu sync.Mutex
var dirReady = map[string]bool{}

// DataDir is the directory of every data file, lock, socket and log.
//
// TOKENPULSE_DATA_DIR replaces ~/.local/share/tokenpulse. A test run must set
// it, so a test can never read or write the production directory. The directory
// is created private (0700); an existing one is tightened on first use.
func DataDir() (string, error) {
	dir, err := resolveDataDir()
	if err != nil {
		return "", err
	}
	dirMu.Lock()
	defer dirMu.Unlock()
	if !dirReady[dir] {
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return "", err
		}
		_ = os.Chmod(dir, 0o700)
		dirReady[dir] = true
	}
	return dir, nil
}

func resolveDataDir() (string, error) {
	if dir := os.Getenv("TOKENPULSE_DATA_DIR"); dir != "" {
		return dir, nil
	}
	if os.Getenv("TOKENPULSE_ENV") == "test" {
		return "", errors.New("TOKENPULSE_ENV=test requires TOKENPULSE_DATA_DIR")
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".local", "share", "tokenpulse"), nil
}

// ConfigPath is TOKENPULSE_CONFIG_PATH when set, else config.toml in DataDir.
func ConfigPath() (string, error) {
	if path := os.Getenv("TOKENPULSE_CONFIG_PATH"); path != "" {
		return path, nil
	}
	dir, err := DataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "config.toml"), nil
}

// Package paths resolves every data file the program uses.
package paths

import (
	"errors"
	"os"
	"path/filepath"
)

// DataDir is the directory of every data file, lock, socket and log.
//
// TOKENPULSE_DATA_DIR replaces ~/.local/share/tokenpulse. A test run must set
// it, so a test can never read or write the production directory.
func DataDir() (string, error) {
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

// File returns a file inside DataDir.
func File(name string) (string, error) {
	dir, err := DataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, name), nil
}

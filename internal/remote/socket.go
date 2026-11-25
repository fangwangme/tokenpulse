package remote

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"os"
	"path/filepath"
	"runtime"
)

// Unix-domain socket paths are limited by the kernel's sockaddr_un. Linux
// allows 107 bytes before the terminating NUL and macOS 103. A data directory
// can be longer than that, so the socket moves to the runtime directory.
const socketName = "serve.sock"

func unixSocketPathLimit() int {
	if runtime.GOOS == "darwin" {
		return 103
	}
	return 107
}

// SocketPath returns the Unix socket serve listens on. It is normally
// <dataDir>/serve.sock. When that path does not fit the platform limit, the
// socket moves to $XDG_RUNTIME_DIR, or the system temporary directory, named
// by a hash of the absolute data directory. Server and clients share this
// function, so both always resolve the same file.
func SocketPath(dataDir string) (string, error) {
	direct := filepath.Join(dataDir, socketName)
	if len(direct) <= unixSocketPathLimit() {
		return direct, nil
	}
	absolute, err := filepath.Abs(dataDir)
	if err != nil {
		return "", err
	}
	sum := sha256.Sum256([]byte(absolute))
	name := "tokenpulse-" + hex.EncodeToString(sum[:16]) + ".sock"
	directories := []string{}
	if runtimeDir := os.Getenv("XDG_RUNTIME_DIR"); runtimeDir != "" {
		directories = append(directories, runtimeDir)
	}
	directories = append(directories, os.TempDir())
	for _, directory := range directories {
		candidate := filepath.Join(directory, name)
		if len(candidate) <= unixSocketPathLimit() {
			return candidate, nil
		}
	}
	return "", errors.New("no directory can hold the serve socket path")
}

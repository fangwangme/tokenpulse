package remote

import (
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// remote.md "Local control": a data directory longer than the kernel's
// sockaddr_un limit must not stop serve from listening, and the TUI/CLI client
// must find the same socket.
func TestLongDataDirSocketMovesToRuntimeDir(t *testing.T) {
	runtimeDir := t.TempDir()
	t.Setenv("XDG_RUNTIME_DIR", runtimeDir)
	dataDir := filepath.Join(t.TempDir(), strings.Repeat("a", 80), strings.Repeat("b", 80))
	if err := os.MkdirAll(dataDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if len(filepath.Join(dataDir, socketName)) <= unixSocketPathLimit() {
		t.Fatal("test data directory is not long enough")
	}

	socket, err := SocketPath(dataDir)
	if err != nil {
		t.Fatal(err)
	}
	if socket == filepath.Join(dataDir, socketName) {
		t.Fatalf("socket stayed in the long data directory: %s", socket)
	}
	if directory := filepath.Dir(socket); directory != runtimeDir && directory != os.TempDir() {
		t.Fatalf("socket directory = %s, want %s or %s", directory, runtimeDir, os.TempDir())
	}
	if len(socket) > unixSocketPathLimit() {
		t.Fatalf("socket path is still too long: %s", socket)
	}

	server := &Server{DataDir: dataDir}
	listener, err := server.listenLocalSocket(socket)
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	defer os.Remove(socket)
	info, err := os.Stat(socket)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("socket mode = %o, want 600", info.Mode().Perm())
	}

	httpServer := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{"version": "test"})
	})}
	go func() { _ = httpServer.Serve(listener) }()
	defer httpServer.Close()

	client := NewLocalClient(dataDir)
	if client.Socket != socket {
		t.Fatalf("client socket = %s, want %s", client.Socket, socket)
	}
	state, err := client.State()
	if err != nil {
		t.Fatal(err)
	}
	if state.Version != "test" {
		t.Fatalf("version = %q", state.Version)
	}

	// A listener that still answers is never replaced.
	if _, err := server.listenLocalSocket(socket); err == nil || !strings.Contains(err.Error(), "already accepting") {
		t.Fatalf("second listen error = %v", err)
	}
}

package paths

import (
	"os"
	"path/filepath"
	"testing"
)

func TestDataDirIsPrivate(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "tokenpulse")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("TOKENPULSE_DATA_DIR", dir)
	got, err := DataDir()
	if err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(got)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o700 {
		t.Fatalf("mode = %o, want 700", info.Mode().Perm())
	}
}

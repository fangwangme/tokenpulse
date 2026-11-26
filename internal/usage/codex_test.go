package usage

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"

	"github.com/klauspost/compress/zstd"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

func writeZstd(t *testing.T, path, content string) {
	t.Helper()
	var buffer bytes.Buffer
	writer, err := zstd.NewWriter(&buffer)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := writer.Write([]byte(content)); err != nil {
		t.Fatal(err)
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, buffer.Bytes(), 0o644); err != nil {
		t.Fatal(err)
	}
}

// usage-sources.md "Codex": compressed rollout files are parsed, and a
// forked session counts nothing until its first turn_context.
func TestCodexCompressedForkSkipsInheritedHistory(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("CODEX_HOME", "")

	writeZstd(t, filepath.Join(home, ".codex", "sessions", "2026", "11", "20", "rollout-zst-fork.jsonl.zst"), `{"type":"session_meta","payload":{"id":"codex-zst-fork","forked_from_id":"codex-root","model_provider":"openai","model":"gpt-5"}}
{"type":"event_msg","timestamp":"2026-11-20T15:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":0,"output_tokens":0},"total_token_usage":{"input_tokens":5000000,"output_tokens":200000}}}}
{"type":"turn_context","payload":{"model":"gpt-5.6-sol"}}
{"type":"event_msg","timestamp":"2026-11-20T15:05:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":40000,"output_tokens":2000},"total_token_usage":{"input_tokens":5040000,"output_tokens":202000}}}}
`)
	writeZstd(t, filepath.Join(home, ".codex", "archived_sessions", "2026", "11", "20", "rollout-archived.zst"), `{"type":"session_meta","payload":{"id":"codex-archived","model_provider":"openai","model":"gpt-5"}}
{"type":"event_msg","timestamp":"2026-11-20T16:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":7,"output_tokens":3},"total_token_usage":{"input_tokens":7,"output_tokens":3}}}}
`)

	messages, err := NewCodexParser().Parse(nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(messages) != 2 {
		t.Fatalf("messages = %d, want 2", len(messages))
	}
	fork := messages[0]
	if fork.SessionID != "codex-zst-fork" || fork.ModelID != "gpt-5.6-sol" {
		t.Fatalf("fork = %+v", fork)
	}
	if fork.Tokens.Input != 40000 || fork.Tokens.Output != 2000 || fork.TotalTokens() != 42000 {
		t.Fatalf("fork tokens = %+v, total %d", fork.Tokens, fork.TotalTokens())
	}
	if messages[1].SessionID != "codex-archived" || messages[1].TotalTokens() != 10 {
		t.Fatalf("archived = %+v", messages[1])
	}
}

// go-rewrite.md "Existing data": replacing readable Codex sessions removes
// stale inherited rows but keeps a session whose source file is gone.
func TestReplaceParsedSessionsKeepsMissingSourceSessions(t *testing.T) {
	store := NewStore(filepath.Join(t.TempDir(), "usage.db"))
	makeMessage := func(session, key string, input int64) provider.UnifiedMessage {
		message := provider.NewMessage("codex", "gpt-5", "openai", session, key, 1764000000000, provider.TokenBreakdown{Input: input})
		message.ParserVersion = "codex-v4"
		return message
	}
	cost := func(string, string, string, provider.TokenBreakdown) (float64, bool) { return 0, false }
	if _, err := store.IngestMessages([]provider.UnifiedMessage{
		makeMessage("readable", "inherited", 1_000_000),
		makeMessage("gone", "kept", 10),
	}, cost); err != nil {
		t.Fatal(err)
	}
	if _, err := store.ReplaceParsedSessions([]provider.UnifiedMessage{
		makeMessage("readable", "fresh", 40_000),
	}, cost); err != nil {
		t.Fatal(err)
	}
	db, err := store.Open()
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	rows, err := db.Query("SELECT session_id, message_key FROM usage_messages WHERE source = 'codex' ORDER BY session_id, message_key")
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var got []string
	for rows.Next() {
		var session, key string
		if err := rows.Scan(&session, &key); err != nil {
			t.Fatal(err)
		}
		got = append(got, session+":"+key)
	}
	want := []string{"gone:kept", "readable:fresh"}
	if len(got) != len(want) || got[0] != want[0] || got[1] != want[1] {
		t.Fatalf("rows = %v, want %v", got, want)
	}
}

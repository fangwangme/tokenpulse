#!/usr/bin/env python3
"""Builds the synthetic parity home shared by the Go and Rust test runs.

Every file here is invented: no conversation text, account name, token or
home path from a real machine is used. The layout mirrors the shapes the usage
parsers read; see testdata/parity/README.md for the case list.
"""
import json
import os
import shutil
import sqlite3
import struct
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent
HOME = ROOT / "home"


def write_jsonl(path: Path, entries):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as fh:
        for entry in entries:
            fh.write(json.dumps(entry, separators=(",", ":")) + "\n")


def proto_varint(value: int) -> bytes:
    out = bytearray()
    while value >= 0x80:
        out.append((value & 0x7F) | 0x80)
        value >>= 7
    out.append(value)
    return bytes(out)


def proto_key(field: int, wire: int) -> bytes:
    return proto_varint(field << 3 | wire)


def proto_bytes(field: int, value: bytes) -> bytes:
    return proto_key(field, 2) + proto_varint(len(value)) + value


def proto_uint(field: int, value: int) -> bytes:
    return proto_key(field, 0) + proto_varint(value)


def antigravity_blob(request_uuid, response_id, model_enum, served, input_tokens,
                     total_output, cache_read, thinking, response_output):
    usage = b"".join([
        proto_uint(2, input_tokens),
        proto_uint(3, total_output),
        proto_uint(5, cache_read),
        proto_uint(9, thinking),
        proto_uint(10, response_output),
        proto_bytes(11, response_id.encode()),
    ])
    payload = proto_bytes(4, usage)
    if served:
        payload += proto_bytes(19, served.encode())
    if model_enum:
        pair = proto_bytes(1, b"model_enum") + proto_bytes(2, model_enum.encode())
        payload += proto_bytes(20, pair)
    return proto_bytes(1, payload) + proto_bytes(4, request_uuid.encode())


def antigravity_step(request_uuid, seconds):
    stamp = proto_uint(1, seconds) + proto_uint(2, 123_000_000)
    return proto_bytes(12, request_uuid.encode()) + proto_bytes(1, stamp)


def build_antigravity_conversation(path: Path, session_id: str, records):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.unlink()
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE trajectory_meta (trajectory_id TEXT);
        CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
        CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB);
        """
    )
    conn.execute("INSERT INTO trajectory_meta VALUES (?)", ("trajectory-" + session_id,))
    idx = 0
    for record in records:
        request_uuid, response_id, enum, served, tokens, seconds = record
        if seconds is not None:
            conn.execute("INSERT INTO steps VALUES (?, ?)",
                         (idx, antigravity_step(request_uuid, seconds)))
        conn.execute("INSERT INTO gen_metadata VALUES (?, ?)",
                     (idx, antigravity_blob(request_uuid, response_id, enum, served, *tokens)))
        idx += 1
    conn.commit()
    conn.close()


def build_opencode(path: Path):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.unlink()
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE message (
          id text PRIMARY KEY, session_id text NOT NULL, time_created integer NOT NULL,
          time_updated integer NOT NULL, data text NOT NULL
        );
        CREATE TABLE session_v2 (
          id text PRIMARY KEY, parent_id text, fork_session_id text, fork_boundary text,
          model text, time_created integer NOT NULL, time_updated integer NOT NULL
        );
        CREATE TABLE session_message (
          id text PRIMARY KEY, session_id text NOT NULL, type text NOT NULL,
          seq integer NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL,
          data text NOT NULL
        );
        """
    )
    t0 = 1_764_000_000_000  # 2025-11-24; dates only matter to the parsers.

    def tokens(n):
        return {"input": n, "output": 10, "reasoning": 1,
                "cache": {"read": 5, "write": 2}}

    def v1(msg_id, session, data, created=t0):
        conn.execute("INSERT INTO message VALUES (?, ?, ?, ?, ?)",
                     (msg_id, session, created, created, json.dumps(data)))

    def v2(msg_id, session, kind, seq, data, created=t0):
        conn.execute("INSERT INTO session_message VALUES (?, ?, ?, ?, ?, ?, ?)",
                     (msg_id, session, kind, seq, created, created, json.dumps(data)))

    # A migrated session: both layouts, v2 must win for the shared id.
    conn.execute("INSERT INTO session_v2 VALUES ('ses_migrated', NULL, NULL, NULL, NULL, 0, 0)")
    conn.execute("INSERT INTO session_v2 VALUES ('ses_old', NULL, NULL, NULL, NULL, 0, 0)")
    conn.execute("INSERT INTO session_v2 VALUES ('ses_carbon', NULL, NULL, NULL, ?, 0, 0)",
                 (json.dumps({"id": "gemini-4-carbon", "providerID": "google"}),))
    v1("msg_shared", "ses_migrated", {
        "role": "assistant", "modelID": "v1-model", "providerID": "anthropic",
        "tokens": tokens(100), "time": {"created": t0}})
    v2("msg_shared", "ses_migrated", "assistant", 2, {
        "model": {"id": "v2-model", "providerID": "opencode"},
        "tokens": tokens(100), "time": {"created": t0}})
    v1("msg_dropped", "ses_migrated", {
        "role": "assistant", "modelID": "v1-model", "providerID": "anthropic",
        "tokens": tokens(7), "time": {"created": t0}})
    v2("msg_old", "ses_old", "assistant", 1, {
        "model": {"id": "old-model", "providerID": "old"},
        "tokens": tokens(3), "time": {"created": t0 / 1000}})
    v2("msg_compact", "ses_carbon", "compaction", 1, {
        "status": "completed", "tokens": tokens(50), "time": {"created": t0}})
    v2("msg_compact_session_model", "ses_carbon", "compaction", 2, {
        "status": "failed", "tokens": tokens(20), "time": {"created": t0}})
    v2("msg_running", "ses_carbon", "compaction", 3, {
        "status": "running", "time": {"created": t0}})
    # Fork copies up to and including the parent assistant row are dropped.
    conn.execute("INSERT INTO session_v2 VALUES ('ses_parent', NULL, NULL, NULL, NULL, 0, 0)")
    conn.execute("INSERT INTO session_v2 VALUES ('ses_fork', NULL, 'ses_parent', ?, NULL, 0, 0)",
                 (json.dumps({"type": "through", "messageID": "p_a1"}),))
    v2("p_a1", "ses_parent", "assistant", 2, {
        "model": {"id": "fork-model", "providerID": "google"},
        "tokens": tokens(100), "time": {"created": t0}})
    v2("f_copy", "ses_fork", "assistant", 2, {
        "model": {"id": "fork-model", "providerID": "google"},
        "tokens": tokens(100), "time": {"created": t0}})
    v2("f_own", "ses_fork", "assistant", 3, {
        "model": {"id": "fork-model", "providerID": "google"},
        "tokens": tokens(5), "time": {"created": t0 + 1000}})
    conn.commit()
    conn.close()


def build_opencode_channel(path: Path):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.unlink()
    conn = sqlite3.connect(path)
    conn.execute(
        "CREATE TABLE message (id text PRIMARY KEY, session_id text, time_created integer, data text)")
    conn.execute(
        "INSERT INTO message VALUES (?, ?, ?, ?)",
        ("channel_msg", "ses_channel", 1_764_000_000_000,
         json.dumps({"role": "assistant", "modelID": "channel-model",
                     "providerID": "openai", "tokens": {"input": 11, "output": 2},
                     "time": {"created": 1_764_000_000.0}})))
    conn.commit()
    conn.close()


def main():
    if HOME.exists():
        shutil.rmtree(HOME)
    HOME.mkdir(parents=True)

    # Claude: projects and transcripts, duplicate id+request id, fallback key,
    # route hint, and negative-token clamping.
    write_jsonl(HOME / ".claude/projects/p1/session-a.jsonl", [
        {"type": "user", "message": {"role": "user", "content": [
            {"type": "text", "text": "<local-command-stdout>Set model to anthropic/nvidia_nim/deepseek-ai/deepseek-v4-pro</local-command-stdout>"}]}},
        {"type": "assistant", "session_id": "claude-session-1", "requestId": "req-1",
         "timestamp": "2026-11-20T10:00:00.000Z",
         "message": {"id": "msg-1", "model": "deepseek-ai/deepseek-v4-pro",
                     "usage": {"input_tokens": 10, "output_tokens": 5}}},
        {"type": "assistant", "session_id": "claude-session-1", "requestId": "req-1",
         "timestamp": "2026-11-20T10:00:01.000Z",
         "message": {"id": "msg-1", "model": "deepseek-ai/deepseek-v4-pro",
                     "usage": {"input_tokens": 99, "output_tokens": 99}}},
        {"type": "assistant", "session_id": "claude-session-1",
         "timestamp": "2026-11-20T10:05:00.000Z",
         "message": {"model": "claude-opus-4-6",
                     "usage": {"input_tokens": -10, "output_tokens": 20,
                               "cache_read_input_tokens": 5,
                               "cache_creation_input_tokens": 2}}},
    ])
    write_jsonl(HOME / ".claude/transcripts/transcript-a.jsonl", [
        {"type": "assistant", "session_id": "claude-transcript-1", "requestId": "req-t",
         "timestamp": "2026-11-21T09:00:00.000Z",
         "message": {"id": "msg-t", "model": "claude-sonnet-4-5-20250929",
                     "usage": {"input_tokens": 1, "output_tokens": 117,
                               "cache_read_input_tokens": 55_725,
                               "cache_creation_input_tokens": 3_911}}},
    ])

    # Codex: last usage, cumulative fallback, repeated/regressed totals, a
    # forked replay, and a compressed file the reference never reads.
    write_jsonl(HOME / ".codex/sessions/2026/11/20/rollout-root.jsonl", [
        {"type": "session_meta", "payload": {"id": "codex-root", "model_provider": "openai",
                                             "model": "gpt-5"}},
        {"type": "event_msg", "timestamp": "2026-11-20T12:00:00.000Z",
         "payload": {"type": "token_count", "info": {
             "model": "gpt-5",
             "last_token_usage": {"input_tokens": 100, "output_tokens": 20,
                                  "cached_input_tokens": 120,
                                  "reasoning_output_tokens": 5},
             "total_token_usage": {"input_tokens": 100, "output_tokens": 20,
                                   "cached_input_tokens": 120,
                                   "reasoning_output_tokens": 5}}}},
        {"type": "event_msg", "timestamp": "2026-11-20T12:01:00.000Z",
         "payload": {"type": "token_count", "info": {
             "total_token_usage": {"input_tokens": 180, "output_tokens": 40,
                                   "cached_input_tokens": 140,
                                   "reasoning_output_tokens": 7}}}},
        {"type": "event_msg", "timestamp": "2026-11-20T12:02:00.000Z",
         "payload": {"type": "token_count", "info": {
             "last_token_usage": {"input_tokens": 20, "output_tokens": 5},
             "total_token_usage": {"input_tokens": 180, "output_tokens": 40,
                                   "cached_input_tokens": 140,
                                   "reasoning_output_tokens": 7}}}},
        {"type": "event_msg", "timestamp": "2026-11-20T12:03:00.000Z",
         "payload": {"type": "token_count", "info": {
             "last_token_usage": {"input_tokens": 30, "output_tokens": 6},
             "total_token_usage": {"input_tokens": 140, "output_tokens": 30}}}},
    ])
    write_jsonl(HOME / ".codex/sessions/2026/11/20/rollout-fork.jsonl", [
        {"type": "session_meta", "payload": {"id": "codex-fork", "forked_from_id": "codex-root",
                                             "model_provider": "openai", "model": "gpt-5",
                                             "source": {"subagent": {"thread_spawn": {"depth": 1}}}}},
        {"type": "event_msg", "timestamp": "2026-11-20T13:00:00.000Z",
         "payload": {"type": "token_count", "info": {
             "last_token_usage": {"input_tokens": 0, "output_tokens": 0},
             "total_token_usage": {"input_tokens": 28_000_000, "output_tokens": 700_000}}}},
        {"type": "event_msg", "timestamp": "2026-11-20T13:00:01.000Z",
         "payload": {"type": "token_count", "info": {
             "last_token_usage": {"input_tokens": 30_000, "output_tokens": 1_000},
             "total_token_usage": {"input_tokens": 28_030_000, "output_tokens": 701_000}}}},
        {"type": "turn_context", "payload": {"model": "gpt-5.6-sol"}},
        {"type": "event_msg", "timestamp": "2026-11-20T13:05:00.000Z",
         "payload": {"type": "token_count", "info": {
             "last_token_usage": {"input_tokens": 40_000, "output_tokens": 2_000},
             "total_token_usage": {"input_tokens": 28_070_000, "output_tokens": 703_000}}}},
    ])
    # CODEX_HOME discovery.
    write_jsonl(HOME / ".codex-home/sessions/2026/11/20/rollout-custom.jsonl", [
        {"type": "session_meta", "payload": {"id": "codex-custom", "model_provider": "openai",
                                             "model": "gpt-5"}},
        {"type": "event_msg", "timestamp": "2026-11-20T14:00:00.000Z",
         "payload": {"type": "token_count", "info": {
             "last_token_usage": {"input_tokens": 7, "output_tokens": 3},
             "total_token_usage": {"input_tokens": 7, "output_tokens": 3}}}},
    ])
    # Never read by 0.6.16; proves compressed files are ignored by both.
    (HOME / ".codex/sessions/2026/11/20/rollout-zipped.jsonl.zst").write_bytes(b"\x28\xb5\x2f\xfdnot-a-real-zstd-stream")

    # Gemini: legacy JSON with cache-inclusive input, streamed JSONL with tool
    # tokens, a duplicate id where the later record wins, and a non-gemini row.
    gem = HOME / ".gemini/tmp"
    gem.mkdir(parents=True)
    (gem / "session-legacy.json").write_text(json.dumps({
        "sessionId": "gem-legacy",
        "startTime": "2026-11-20T12:00:00Z",
        "lastUpdated": "2026-11-20T12:05:00Z",
        "messages": [
            {"id": "user-1", "type": "user"},
            {"id": "gem-json", "type": "gemini", "timestamp": "2026-11-20T12:01:00Z",
             "model": "gemini-2.5-pro",
             "tokens": {"input": 120, "output": 45, "cached": 30, "thoughts": 12, "total": 177}},
        ],
    }), encoding="utf-8")
    write_jsonl(gem / "session-stream.jsonl", [
        {"sessionId": "gem-stream", "startTime": "2026-11-21T12:28:18.748Z",
         "lastUpdated": "2026-11-21T12:28:18.748Z", "kind": "main"},
        {"id": "gem-dup", "timestamp": "2026-11-21T12:29:01.000Z", "type": "gemini",
         "model": "gemini-3.1-pro-preview",
         "tokens": {"input": 100, "output": 10, "cached": 40, "thoughts": 5,
                    "tool": 0, "total": 115}},
        {"id": "gem-dup", "timestamp": "2026-11-21T12:29:02.000Z", "type": "gemini",
         "model": "gemini-3.1-pro-preview",
         "tokens": {"input": 130, "output": 20, "cached": 50, "thoughts": 7,
                    "tool": 0, "total": 157}},
        {"id": "gem-tool", "timestamp": "2026-11-21T12:30:00.000Z", "type": "gemini",
         "model": "gemini-3.1-pro-preview",
         "tokens": {"input": 100, "output": 20, "cached": 30, "thoughts": 5,
                    "tool": 7, "total": 162}},
    ])

    # Pi: session/model_change/responseId plus entry-id and _tokens spellings.
    write_jsonl(HOME / ".pi/agent/sessions/session-pi.jsonl", [
        {"type": "session", "id": "pi-session-1"},
        {"type": "model_change", "provider": "nvidia-nim", "modelId": "gpt-4.1"},
        {"type": "message", "id": "pi-msg-1", "timestamp": "2026-11-22T08:50:30.109Z",
         "message": {"role": "assistant", "provider": "nvidia-nim", "model": "gpt-4.1",
                     "usage": {"input": 10, "output": 20, "cacheRead": 3, "cacheWrite": 1},
                     "timestamp": 1_795_337_430_109, "responseId": "pi-resp-1"}},
        {"type": "assistant", "id": "pi-msg-2", "timestamp": "2026-11-22T08:51:00.000Z",
         "message": {"role": "assistant", "provider": "openai", "model": "gpt-5.1",
                     "usage": {"input_tokens": 4, "output_tokens": 2}}},
    ])

    # Copilot: OTEL cache fallback and response-id de-duplication, plus the
    # session-state shutdown summary that replaces per-message rows.
    write_jsonl(HOME / ".local/share/github-copilot/otel.jsonl", [
        {"hrTime": [1_763_700_000, 0],
         "resource": {"_rawAttributes": [["session.id", {"value": "copilot-sess-1"}]]},
         "attributes": {"event.name": "gen_ai.client.inference.operation.details",
                        "gen_ai.response.model": "gpt-4o-mini-2024-07-18",
                        "gen_ai.usage.input_tokens": 1000,
                        "gen_ai.usage.output_tokens": 100,
                        "gen_ai.response.id": "resp-1"}},
        {"hrTime": [1_763_700_001, 0],
         "resource": {"_rawAttributes": [["session.id", {"value": "copilot-sess-1"}]]},
         "attributes": {"event.name": "gen_ai.client.inference.operation.details",
                        "gen_ai.response.model": "gpt-4o-mini-2024-07-18",
                        "gen_ai.usage.input_tokens": 1500,
                        "gen_ai.usage.output_tokens": 200,
                        "gen_ai.usage.cache_read.input_tokens": 200,
                        "gen_ai.usage.cache_creation.input_tokens": 50,
                        "gen_ai.response.id": "resp-2"}},
        {"hrTime": [1_763_700_002, 0],
         "resource": {"_rawAttributes": [["session.id", {"value": "copilot-sess-1"}]]},
         "attributes": {"event.name": "gen_ai.client.inference.operation.details",
                        "gen_ai.response.model": "gpt-4o-mini-2024-07-18",
                        "gen_ai.usage.input_tokens": 1500,
                        "gen_ai.usage.output_tokens": 200,
                        "gen_ai.response.id": "resp-2"}},
    ])
    write_jsonl(HOME / ".copilot/session-state/copilot-agent/events.jsonl", [
        {"type": "session.start",
         "data": {"sessionId": "copilot-agent", "selectedModel": "claude-opus-4.6"},
         "timestamp": "2026-11-23T18:12:06.646Z"},
        {"type": "assistant.message",
         "data": {"messageId": "msg-1", "interactionId": "interaction-1", "outputTokens": 482},
         "timestamp": "2026-11-23T18:12:19.679Z"},
        {"type": "session.shutdown",
         "data": {"modelMetrics": {"claude-opus-4.6": {
             "requests": {"count": 3, "cost": 3.0},
             "usage": {"inputTokens": 9, "outputTokens": 6,
                       "cacheReadTokens": 3, "cacheWriteTokens": 0}}}},
         "timestamp": "2026-11-23T19:48:47.935Z"},
    ])

    # OpenCode: both layouts in one database and a channel database.
    build_opencode(HOME / ".local/share/opencode/opencode.db")
    build_opencode_channel(HOME / ".local/share/opencode/opencode-beta.db")

    # Antigravity CLI conversation: alias hit, served name, unversioned worker,
    # unversioned orchestrator, an undated generation, and zero output.
    session_id = "parity-antigravity-session"
    build_antigravity_conversation(
        HOME / f".gemini/antigravity-cli/conversations/{session_id}.db", session_id, [
            ("req-alias", "resp-alias", "MODEL_PLACEHOLDER_M525", "gemini-4-hydrogen",
             (10, 100, 5, 30, 70), 1_763_900_000),
            ("req-served", "resp-served", "MODEL_PLACEHOLDER_UNKNOWN", "gemini-4-neon",
             (11, 50, 0, 10, 40), 1_763_900_100),
            ("req-family", "resp-family", "MODEL_PLACEHOLDER_UNKNOWN", "gemini-flash-worker-agy",
             (12, 40, 0, 8, 32), 1_763_900_200),
            ("req-orch", "resp-orch", "MODEL_PLACEHOLDER_UNKNOWN", "gemini-pro-orchestrator",
             (13, 30, 0, 6, 24), 1_763_900_300),
            ("req-undated", "resp-undated", "MODEL_PLACEHOLDER_M319", "gemini-3.8-flash-n",
             (14, 20, 0, 4, 16), None),
            ("req-zero", "resp-zero", "", "gemini-3.6-flash",
             (0, 0, 0, 0, 0), 1_763_900_400),
        ])

    alias_path = HOME / ".local/share/tokenpulse/antigravity-cache/model-aliases.json"
    alias_path.parent.mkdir(parents=True, exist_ok=True)
    alias_path.write_text(json.dumps({
        "version": 1,
        "updatedAt": "2026-11-20T00:00:00Z",
        "aliases": {
            "model_placeholder_m525": {
                "rawModelId": "MODEL_PLACEHOLDER_M525",
                "modelId": "gemini-4-hydrogen",
                "label": "Gemini 4 Hydrogen",
                "source": "parity-fixture",
                "firstSeenAt": "2026-11-20T00:00:00Z",
                "lastSeenAt": "2026-11-20T00:00:00Z",
            },
        },
    }, indent=2) + "\n", encoding="utf-8")

    print(HOME)


if __name__ == "__main__":
    sys.exit(main())

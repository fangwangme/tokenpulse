# Parity Fixtures

Synthetic inputs for the Go 0.7.0 port. No real conversation text, account
name, token or home path appears here. `generate.py` builds `home/`; the
SQLite files and JSON/JSONL session files are committed so the test does not
need Python.

## Running the parity check

```bash
go test ./internal/parity -run TestParityFixtures -v
```

`golden.json` holds the ledger recorded from the 0.6.16 Rust binary on the
same home. The only allowed differences are the two records listed under
`expectedOverrides`: unversioned Antigravity families resolve on purpose in
0.7.0. Everything else — source, client, session, key, date, timestamp, model
ids, served model, parser version and every token count — must match.

The harness also runs the `OPENCODE_DB` override scenario recorded under
`opencodeOverride`.

## Usage cases

| Source | Cases in `home/` |
| :--- | :--- |
| Claude | `projects` and `transcripts`; model route hint; negative input clamped; duplicate id+request id keeps the later copy; fallback key |
| Codex | `sessions` and `CODEX_HOME`; last usage; cumulative fallback; repeated and regressed totals; forked replay skipped until the first `turn_context`; `.jsonl.zst` ignored |
| Gemini | legacy JSON with cache-inclusive input; JSONL tool tokens; duplicate message id where the later record wins |
| Pi | session/model change; `responseId`; entry id; `input_tokens` spelling; numeric timestamp |
| Copilot | OTEL event, explicit cache fields, duplicate response id; session-state shutdown summary and model change |
| OpenCode | 1.x only and migrated 1.x+2.x in one database; v2 wins shared ids; v1-only rows kept; forks; compaction rows; channel database; `OPENCODE_DB` |
| Antigravity | local conversation SQLite/protobuf; learned alias; served name; unversioned worker and orchestrator; undated and zero-output records dropped |

## Quota fixtures

`quota/*.json` are recorded provider response shapes; `quota/golden/*.json`
were written by a temporary helper built against the 0.6.16 Rust quota module.
`TestClaudeQuotaParity`, `TestCodexQuotaParity` and
`TestAntigravityQuotaParity` compare windows, percentages, reset times, plan,
account, extra usage and reset credits. Window order is compared where 0.7.0
keeps it and checked separately against `quota.md` where the release reorders
Codex windows.

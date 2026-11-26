# Usage Sources

The ledger is built from agent session files. Every source keeps its parser
version string: a change makes that source's rows stale, and the next refresh
replaces them with a fresh parse.

## Refresh windows

- A source with nothing stored is scanned in full.
- Otherwise a scan starts one day before the previous scan's start, recorded in
  `source_refresh_state`. With no recorded start it starts two days before the
  newest stored message.
- A narrow run (`--since`, `--refresh-days`, `--rebuild-all`, or a stale parser)
  does not advance that anchor.
- Files are discovered by extension and filtered by arrival date, the later of
  modification and inode change time, so a restored file with an old mtime is
  still read.
- Claude, Codex, Gemini, Pi and Copilot merge a refresh by changed session;
  stale-parser rows of those sessions are replaced only after the parse
  succeeds. OpenCode upserts by message id. `--rebuild-all` and
  `--refresh-days` replace the whole source (or the range) after a successful
  parse.
- Ledger identity is `(source, client, message_key)`. Antigravity rows observed
  under several runtimes collapse to the widest observation per `message_key`.

## Claude (`claude-v4`)

Paths: `~/.claude/projects/**/*.jsonl` and `~/.claude/transcripts/**/*.jsonl`.

Assistant entries with a model and usage become messages. The session id is the
entry's `session_id`, else the file name. The key is `<message.id>:<requestId>`,
`<message.id>` when the request id is empty, else a key built from the session,
timestamp, model, line index and input/output/cache-read counts. A repeated key
keeps the later timestamp, then the larger total. Input, output, cache read and
cache write are stored separately and negative values clamp to 0. A `user`
entry whose text says `Set model to <route>` (optionally wrapped in
`<local-command-stdout>` and followed by ` with …`) sets a provider hint: a
route with three or more slash-separated segments uses the third-from-last
segment, lowercased with `_` as `-`; otherwise the message's own `provider`,
else the model family. ANSI escapes are stripped before reading the route.

## Codex (`codex-v4`)

Paths: `~/.codex/sessions`, `~/.codex/archived_sessions`,
`$CODEX_HOME/sessions` and `$CODEX_HOME/archived_sessions` (`~/.codex` when
unset), `**/*.jsonl`, `**/*.jsonl.zst` and `**/*.zst`. Compressed files are
decoded with a pure-Go zstd decoder; no external `zstd` and no cgo. The parser
version stays `codex-v4`: compressed support and the upgrade repair do not
change the row shape for files the Rust release already read.

`session_meta` gives the session id, `model_provider` (default `openai`) and
model; a forked or sub-agent session (`forked_from_id`, `parent_thread_id`, or
`source.subagent`) inherits history and counts nothing until its first
`turn_context`. Token counts come from `event_msg`/`token_count`:
`last_token_usage` when its total is non-zero, else the delta of
`total_token_usage`; a repeated or regressed cumulative total is skipped.
Cached input is subtracted from input and reasoning from output. A compacted
step can carry only `total_tokens` with every breakdown field zero; that total
is kept, with the unaccounted remainder in input so the ledger total is
correct. The key is session, timestamp, model, input, output, cache read and
reasoning.

On the first 0.7.0 run against an existing ledger, every readable Codex
session is reparsed once and its rows are replaced. Rows of sessions whose
source file no longer exists are left untouched; `--rebuild-all` uses the same
session-level rule for Codex. Later refreshes are incremental and include
compressed files whose arrival time is in the window.

## Gemini (`gemini-v3`)

Path: `~/.gemini/tmp`, files whose name starts with `session-` (`*.json`) or
ends with `.jsonl`.

JSON sessions read `messages`; only `type: gemini` entries with model and
tokens count. JSONL sessions read one record per line and keep the last record
of a repeated message id. The key is the message id, else session, index, model
and input. Timestamps fall back to `lastUpdated`, then `startTime`. Tokens keep
input, output, cache read and thoughts; tool tokens and unaccounted total are
added to input. When `total` equals input+output+thoughts+tool, the input is
cache-inclusive and cache read is subtracted from it.

## Pi (`pi-v3`)

Path: `~/.pi/agent/sessions/**/*.jsonl`.

`session`/`header` entries set session, model and provider; `model_change`
updates them. `message`/`assistant` entries with role `assistant`, usage and a
non-zero total count. Usage accepts `input`/`output` or `input_tokens`/
`output_tokens`, plus `cacheRead`/`cacheWrite`. The message's own model and
provider win, then the session's, then model-based detection. The key is
`responseId`, else the entry id, else session, timestamp and model. A numeric
timestamp is milliseconds; an RFC3339 string is parsed.

## Copilot (`copilot-v3`)

Paths: `$XDG_DATA_HOME/github-copilot` (`~/.local/share/github-copilot`) and
`~/.copilot/session-state`, `**/*.jsonl`.

OTEL logs count `gen_ai.client.inference.operation.details` events. The response
id dedups; the key is `copilot:<responseId>` or a synthetic key. The model is
`gen_ai.response.model`, then `gen_ai.request.model`, else `unknown`; the
session comes from `resource._rawAttributes` `session.id`; the timestamp from
`hrTime`. Explicit cache tokens split the input; otherwise the previous total
input for the same session and model is treated as cache read. A
`session-state/events.jsonl` file is parsed as an agent session instead:
`session.start`/`session.model_change` set session and model, and the latest
`session.shutdown` distributes `modelMetrics` totals across its request count,
replacing per-message `assistant.message` rows. Output-only assistant messages
count as output tokens.

## OpenCode (`opencode-v3`)

Data dir: `$XDG_DATA_HOME/opencode` (`~/.local/share/opencode`). Databases:
`opencode.db` plus sorted `opencode-<channel>.db`; `$OPENCODE_DB` replaces
discovery (relative to the data dir). Any database that cannot be read fails the
whole scan.

One read transaction covers table detection, fork cutoffs and both layouts. 2.x
`session_message` rows of type `assistant`/`compaction` with a JSON `tokens`
object are read first; 1.x `message` rows whose id has no 2.x copy are read
after. The message id is the ledger key in both layouts. Model and provider come
from `data.model.id`/`data.model.providerID` (1.x: `modelID`/`providerID`, then
`model`/`provider`), falling back to the 2.x session's model. Tokens are input,
output, reasoning, cache read and cache write. The timestamp is `time_created`,
else `data.time.created`/`completed` (milliseconds above 1e10, seconds below).
A forked session drops rows up to its fork boundary (`through` = that message's
seq, `before` = the highest seq below it); an unresolvable boundary keeps all
rows. 2.x compaction rows without a model use the session's model.

## Antigravity (`antigravity-v7`)

Cache: `antigravity-cache/cache.db` in the data directory, with `sessions`
(`session_id`, `client`, metadata, `synced_at`) and `session_usage`
(`id`, model identity, tokens, timestamp, `response_id`, `pricing_day`,
`parser_version`, `model_enum`, `served_model`).

Local conversations: SQLite databases of at least 20 characters under
`~/.gemini/antigravity-cli/conversations` (CLI) and
`~/.gemini/antigravity/conversations` (Desktop). Each `gen_metadata.data` blob
is decoded as protobuf: outer field 4 request UUID, field 1 payload; payload
field 4 usage (`.2` input, `.3` total output, `.5` cache read, `.9` thinking,
`.10` response output, `.11` response id), field 19 served name, field 20
repeated key/value metadata (`model_enum`). A record with zero total output is
not a generation. `thinking + response` must equal total output or the record is
dropped. Step times come from `steps.metadata` field 12 (request UUID) and field
1 (protobuf Timestamp); a generation without one is not dated and its cached row
is withdrawn. Other runtimes are read over the Antigravity language server:
process discovery reads `ps`, per-process environment and listening ports,
probes a heartbeat plus an Antigravity identity call, then reads
`GetAllCascadeTrajectories`, `GetCascadeTrajectoryGeneratorMetadata` and
`GetUserStatus`. Local data wins for a runtime when both exist.

A transaction writes each session's rows as
`{client}:{session_id}:{raw_message_key}` with
`raw_message_key = response_id` or `timestamp:model:step`. Reading the cache
dedups by `(session_id, message_key)`: widest total first, then runtime priority
Desktop, IDE, CLI. Usage rows are re-resolved on every read from
`model_enum`/`served_model` (see `antigravity-model-aliases.md`), and a changed
row re-stamps its session so the ingest carries the correction into the ledger.
The cache is read incrementally from the window plus every session synced within
it.

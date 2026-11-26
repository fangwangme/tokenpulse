# Go Implementation

From 0.7.0 TokenPulse is a Go program. It replaced the Rust workspace because
Rust builds cost more memory and CPU than the maintainer's machine comfortably
has, and the release that moved languages also added remote mode.

## What stays the same

The Rust release 0.6.16 is the reference for every behavior no spec of this
release changes. The Go program keeps:

- every CLI command, flag, exit status and machine-readable output
  (`--json`), and the text output's content;
- every config key, default and validation, the config location
  (`~/.local/share/tokenpulse/config.toml`, `TOKENPULSE_CONFIG_PATH`), and the
  config version upgrade;
- every usage source, its file locations, parsing, de-duplication, incremental
  refresh windows and parser-version invalidation, and model-id normalization
  and aliases (`antigravity-model-aliases.md`, `raw-model-usage.md`);
- every quota provider, its endpoints, response mapping, account display and
  observation history, except what `quota.md` changes;
- the Keeper (`keeper.md`), notifications, themes (`tui-theme.md`) and table
  layout rules (`tui-table-layout.md`);
- the data files (listed under "Existing data"), and logging never to stdout
  while the TUI runs.

The behavior changes of this release are `usage-report.md`, `pricing.md`,
`quota.md`, `tui-usage.md`, `remote.md`, the unversioned-family resolution of
`antigravity-model-aliases.md`, and this file's data-directory, test-mode,
migration and build rules.

## Existing data

Every persistent file the Rust release keeps in the data directory is read in
place and kept: `config.toml`, `usage.db` (ledger, daily rollups, refresh
state), `tokenpulse.db` (quota cache, observation, credit-observation and
fetch-failure history, Keeper executions), `keeper_state.json`,
`antigravity-cache/cache.db`, `antigravity-cache/model-aliases.json` (learned
aliases), and the log directory. The Rust `pricing.json` is retired: it is
neither read nor deleted (`pricing.md`).

- Schema changes are forward migrations; nothing is rebuilt from session
  files, because some session files behind the ledger no longer exist.
- Each source's `parser_version` keeps the exact string the Rust release
  writes, so the stale-parser rule (a version change replaces that source's
  ledger rows with a fresh parse) never fires on upgrade. Changing a parser
  version later is a deliberate act with the same consequence as before.
- One 0.7.0 repair runs once for Codex: every session whose rollout file is
  readable (plain or zstd) is reparsed and its ledger rows are replaced; rows
  of sessions whose source file is gone are kept. The run is recorded in
  `app_state` (`codex-zstd-repair-v1`), so later runs are incremental. The
  same session-level rule applies to `--rebuild-all` for Codex.
- Before its first migration of a database, the program writes a WAL-safe
  backup next to it, `<name>.pre-0.7.<timestamp>.bak`, mode 0600. An existing
  backup is never overwritten, so an interrupted migration that is retried
  keeps the original. A database migrated by 0.7.0 is not readable by the Rust
  releases; downgrading means restoring the backup.
- Every backup this program writes (migration or `reprice`) has mode 0600.
- SQLite runs in WAL mode with a busy timeout, as before, so `serve`, the TUI
  and CLI commands can read concurrently.

## Parity

Parity with 0.6.16 is the acceptance bar for the port. A fixture set covering
every usage source and every quota provider lives in the repository: synthetic
or redacted session files and recorded provider responses, never real
conversation text, account names or tokens. Golden outputs recorded from the
0.6.16 build are committed next to them before the Rust code is removed. A
parity check runs the Go build on the fixtures and compares:

- ledger rows: source, client, session, message key, date, timestamp, model
  id, canonical model id, served model, parser version and every token count;
- per-day and per-model token totals, messages and sessions;
- quota snapshots: windows, percentages, reset times, plan and account.

Costs, window order and pace are judged by `pricing.md` and `quota.md`
instead, and Antigravity records whose model is an unversioned family by
`antigravity-model-aliases.md`, because this release changes them on purpose;
the fixtures list those records with their expected new ids.

Codex compressed rollouts are the other deliberate exception. 0.6.16 ignored
`.jsonl.zst`/`.zst`, so its ledger lacks those sessions and can keep inherited
fork history for rows written before fork handling existed. 0.7.0 parses them
and replaces the affected sessions once; `expectedExtraRows` in the parity
golden holds the compressed fork rows the Rust build never produced. Any other
difference remains a port defect.

The fixture set has at least one case for every distinction a source's parser
makes (for example Codex cumulative counters, forks and compressed files;
Claude duplicate message and request ids; OpenCode's database layouts and
forks; Antigravity CLI databases, Language Server cache and alias learning).

## Data directory and test runs

- `TOKENPULSE_DATA_DIR` replaces `~/.local/share/tokenpulse` as the directory
  of every data file, lock, socket and log. The directory is created with mode
  `0700`, and an existing directory is tightened to `0700` on first use.
  `TOKENPULSE_CONFIG_PATH` still overrides the config file alone.
- `TOKENPULSE_ENV=test` marks a test run: the version is shown as
  `<version> (test)` everywhere it appears (`--version`, the TUI, `/api/state`,
  the phone header), `serve` defaults to ports `14120`/`18444`, a test run
  refuses to start when `TOKENPULSE_DATA_DIR` is not set, and
  `tokenpulse service install` refuses to install a unit from a test run, so it
  can never bake the test data directory or ports into a production service.

## Build and distribution

- One statically linked binary per platform, built without cgo: darwin x64 and
  arm64, linux x64 and arm64 (glibc not required). The npm packages and
  launcher keep their names and download the same archive names.
- Formatting is `gofmt`; `go vet` and `go test ./...` are the checks. CI builds
  the web app, then the binary, and runs the checks on macOS (the only place
  macOS-specific code is compiled before a release) and on Linux.
- A plain `go build` needs no Node: the embedded web directory always holds at
  least a committed placeholder page saying the frontend was not built, which
  the web build replaces (`remote.md` "Web app").
- The version has one source in the Go code. The npm packaging script checks a
  release tag against it, and the launcher's install-from-source hint names
  the Go install path.

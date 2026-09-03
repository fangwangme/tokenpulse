# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.7] - 2026-09-03

### Fixed
- **Antigravity conversations no longer invalidate their own cache on every
  refresh**: the change detector took the newest mtime across `.db`, `.db-wal`
  and `.db-shm`, but `-shm` is SQLite's WAL shared-memory index and *readers*
  write it — opening a conversation read-only moves its mtime. Parsing a
  conversation therefore guaranteed it looked changed on the next refresh, a
  closed loop. Measured against the real cache (564 cached sessions), 263
  sessions re-synced every cycle and all 263 were stale solely because of
  `-shm`: for every one of them the stored timestamp was already at or past
  `max(.db, .db-wal)`. The same poisoned timestamp fed the language-server skip
  check, so those sessions were re-fetched over RPC as well. On a steady-state
  refresh this dominated the 77 s total, with the Antigravity parse phase alone
  taking 43.7 s. `-shm` is now excluded; `-wal` still catches writes that land
  only in the write-ahead log, which is what it was added for.

### Changed
- **Settings now configures quota providers only**: the provider rows were a
  hardcoded list that was neither a quota list nor a usage list — it offered
  `gemini`, which has had no quota fetcher since the fetcher was removed, and
  omitted `opencode` and `pi`. Worse, one keypress mutated two different
  things: the persisted quota config *and* the session-only usage source
  filter, so toggling a row silently hid usage rows too. The rows are now
  generated from the quota registry (claude, codex, copilot, antigravity), the
  toggle writes only the quota config, and the block is labelled as such. The
  usage view's own source filter (`s`) is unaffected, and nothing here
  decides what usage is parsed.
- **The Settings list now scrolls to the selected row**: the tab rendered its
  lines into a body of `Min(10)` rows between 10 rows of dashboard chrome and
  never scrolled, so on an 80x30 terminal the bottom rows fell outside the
  visible area while the keyboard could still select and toggle them. The list
  now scrolls to keep the cursor on screen, which also stops the next added
  setting from silently pushing a row off the edge.
- **`config enable` / `config disable` now reject ids with no quota fetcher**:
  they used to write the key and print success for anything, including a typo
  or `gemini`, while quota resolution ignored it — configured-looking and inert
  forever. They now fail with the list of configurable quota providers.
- **A stale provider key in `config.toml` no longer costs a lookup**: quota
  provider resolution is intersected with the registry, so a leftover
  `[providers.gemini]` from an older install no longer opens the quota cache
  every refresh. Existing config files are left untouched — the key is ignored,
  not removed. `gemini` is also gone from the default config. Gemini CLI
  **usage** parsing and its historical data are unchanged; usage never read
  this map.

## [0.5.6] - 2026-09-02

### Fixed
- **Antigravity usage is no longer re-dated onto the day it was synced**: two
  passes write the same rows, and the language-server pass ran second. Its
  timestamp chain ended in "now", so every generation the server reported
  without a `createdAt` was stamped with the instant the sync ran — overwriting
  the exact time the local pass had already recovered from the conversation
  database. One refresh moved eight days of history onto a single afternoon
  (4,919 rows and 530M tokens on one millisecond, across 60 sessions), inflating
  that day's reported usage roughly fourfold while the days it drained read
  zero. The language-server pass now joins the same conversation database by
  `responseId` and carries the generation's real time; the session's start time
  ranks below that join, and a generation nothing can date is dropped rather
  than filed under today.
- **The local pass no longer dates a generation by its file's mtime**: a
  conversation file's last-write time is not the time of any generation inside
  it, and it fell back to "now" when the file had no mtime at all. A generation
  whose time cannot be joined from `steps` is now skipped and counted in the
  sync log.
- **A parser-version bump now clears what it replaces**: usage rows a session
  left behind at an older parser version are removed once it is re-parsed, so a
  mis-dated row cannot survive the bump meant to correct it. Bumped the parser
  version to `antigravity-v5`, which is what re-derives the already mis-dated
  rows from the local conversation databases on the next sync.

## [0.5.5] - 2026-09-02

### Added
- **Antigravity IDE Session Sync & Discovery**: Added multi-root discovery across `~/.gemini/antigravity`, `~/.gemini/antigravity-ide`, and `~/.gemini/antigravity-cli`, along with IDE Language Server process candidate detection (`antigravity-ide`).

### Changed
- **Antigravity Session Deduplication & Incremental Sync**:
  - Unified sessions existing across Desktop and IDE into a canonical Desktop work item with prioritized candidate connection routing (`[Desktop, IDE]`), avoiding redundant queries and duplicate counting.
  - Logical message deduplication by `(session_id, message_key)` ensures multi-client artifact overlaps emit single unified messages without token inflation.
  - Preserved cached session metadata and usage when Language Server connections are inactive.
  - Bumped parser version to `antigravity-v4` for clean migration in the usage ledger.

### Fixed
- **Antigravity usage ledger no longer accumulates duplicate rows**: the ledger's
  primary key includes the runtime client, so an incremental refresh that saw a
  message through a different Antigravity runtime than the previous refresh
  stored it a second time. Reported totals were unaffected — the daily rollup
  already collapsed those rows — but the ledger grew a stale row per message and
  relied on that safety net. Ingest now enforces one row per logical message for
  Antigravity and prunes rows left behind by earlier refreshes.
- **Desktop/IDE overlap now collapses regardless of which app is running**:
  conversation-root discovery was gated on having a live Language Server for that
  root, so the 3-root inventory (and therefore the Desktop+IDE merge) silently
  degraded to whichever apps happened to be open. All three roots are now always
  scanned, and the merge runs after every connection has been listed instead of
  depending on process enumeration order.
- **Kept the widest observation when runtimes disagree**: Desktop and IDE can
  report different token counts for the same response, and the runtime holding a
  stale copy always reports fewer. Deduplication now prefers the larger totals
  and uses runtime priority only to break exact ties, matching what the
  aggregation layer already reported.
- **Stale cache rows from a changed canonical runtime**: re-syncing a session
  only cleared usage rows for its current canonical client, leaving the previous
  client's rows in the Antigravity cache. Every runtime the work item covers is
  now cleared before rewriting.
- **Change detection no longer under-syncs**: the incremental "unchanged" check
  compared against local file mtime only while storing the merged local/RPC
  timestamp, so a session could be skipped indefinitely once an RPC listing
  reported a newer timestamp.
- Electron renderer/GPU/utility children of the Antigravity app are no longer
  probed as Language Server candidates, cutting an `lsof` and heartbeat request
  per helper process on every refresh.
- **A refresh can no longer erase usage it cannot re-read.** Re-parsing a session
  cleared all of its ledger rows and re-inserted whatever the current transcript
  still held. Agents delete their own transcripts on a retention timer, so a
  session can legitimately survive on disk with only part of its history left —
  and at that point the ledger is the only record of the rest. Refreshes now only
  re-derive rows written by an older parser version; recorded usage is a fact
  that a later refresh may correct but may not silently drop.
- **Restored and migrated session files are discovered again.** Incremental
  refresh selected files by modification time, but restoring a backup or moving
  to another machine rewrites files with their *original* mtime — so transcripts
  could land on disk already older than the window and never be read. Discovery
  now uses the later of modification and inode-change time, and the window widened
  from 1 to 7 days. On the machine this was found on, a system migration had
  silently dropped 152 messages (23.9M tokens); they are recovered on the next
  refresh.

### Changed
- **Release asset names** now use readable platform and architecture labels
  (`darwin-x64`, `darwin-arm64`, `linux-x64-gnu`, and `linux-arm64-gnu`)
  instead of exposing Rust target triples such as `x86_64-unknown-linux-gnu`.

## [0.5.4] - 2026-08-23

### Fixed
- **Antigravity token usage is recorded at all**: the usage ledger held zero
  `antigravity` rows. Tokens were only ever fetched over a language-server RPC
  that never connects on Linux, while the token counts sitting in every local
  conversation database went unread. They are now parsed directly, so usage
  arrives whether or not Antigravity is running. On the development machine the
  first run recovered ~21,000 generations going back to 2026-05-20. This happens
  automatically on upgrade — no `--rebuild-all`, no flag.
- **Antigravity costs no longer double-count reasoning tokens**: Antigravity
  reports total output with the thinking tokens already inside it, and TokenPulse
  then billed reasoning again on top. Anyone whose Antigravity data came from a
  running language server (in practice, macOS) has been overcharged for every
  thinking model. Existing rows are corrected in place on first launch, so
  reported Antigravity cost will drop.
- **Antigravity Desktop and CLI are told apart**: conversations under
  `~/.gemini/antigravity/` were skipped entirely, and the ones that were read got
  labelled as CLI regardless of origin. Both runtimes are now scanned and
  labelled correctly, which is also what stops a session that exists in both
  places from being counted twice.
- **Antigravity session titles and workspace paths survive a resync**: a local
  rescan overwrote the details only a running language server can supply,
  and nothing could restore them afterwards.
- **A corrupt Antigravity conversation file can no longer abort the sync**: a
  malformed record is skipped, as was always intended, instead of overflowing on
  an oversized value.

## [0.5.3] - 2026-08-15

### Changed
- **Half-Screen TUI Layout Optimization**:
  - **Activity Heatmap View**: Kept cells fixed at 2-char width (`██`) and made columns responsive (`display_cols = (grid_width / 2).min(total_weeks)`) showing recent weeks ending at today. Aligned month labels, grid border, and footer date range strictly within rendered bounds.
  - **Quota View**: Balanced vertical spacing within provider snapshot cards with breathing room between account details, rate limit windows, and stats. Optimized card row height distribution to prevent empty bottom voids.
  - **Keeper View**: Wrapped agent cards into a responsive multi-row grid on width-constrained displays (~90-110 cols) with adaptive height allocation and mouse click selection support.

## [0.5.2] - 2026-08-15

### Added
- **Quota Recovery Notification System**: Alerts you when an exhausted quota window recovers (`used_percent < 100%`).
  - **4 Notification Levels**, every one of which plays the alert sound — they differ only in how far the visual notification travels:
    - `off`: nothing at all.
    - `in_app`: sound + ambient emerald background pulse, perimeter glow, and a bottom-right toast card with a countdown bar.
    - `terminal`: adds a terminal bell and an OSC 9 desktop notification.
    - `system`: adds a macOS Notification Center banner.
  - **Audible by default**: sound plays through `afplay`, not the terminal bell — most terminals ship with the audible bell turned off (Ghostty defaults to `bell-features = no-audio`), which makes a `\x07`-based chime silent on a stock setup. The built-in chime is normalised to roughly 11 dB above macOS `Ping.aiff`.
  - **Configurable sound**: `notification_sound` accepts `chime` (built in), `none`, or any name under `/System/Library/Sounds`. The TUI Settings row plays each sound as you cycle it.
  - **`tokenpulse config test-notification`**: fires a sample alert through the real code path, so the feature can be verified without waiting for a quota window to actually reset.
  - **Edge-Triggered State Tracking**: Tracks exhausted rate windows across Claude, Codex, Gemini, Antigravity, and Copilot. Windows that recover in the same refresh are announced together, so a batch reset produces one sound and one banner rather than a burst.
  - **TUI Settings & CLI Support**: Cycle `notification_level` and `notification_sound` in the TUI Settings tab, or configure via `tokenpulse config set notification_level=<level>`.

## [0.5.1] - 2026-08-15


### Added
- **Keeper Dashboard Tab**: Added interactive session keeper & scheduled heartbeat dashboard for managing automated wakeup and synchronization across Claude Code, Codex, and Google Antigravity.
  - **5h Daily Wakeup**: Automated morning wakeup timer (default 10:30) to initiate session cooldown timers early.
  - **Weekly Auto-Sync**: Automated quota synchronization timed precisely 1 minute after weekly reset to maximize resource rollover.
  - **Live Execution Stream**: Multi-line live heartbeat logs showing executed CLI commands, models, prompts, responses, duration, and status with smooth mouse wheel scrolling.
  - **Single-Key Toggles & Manual Test**: `[1/d]` to toggle 5h keeper, `[2/w]` to toggle weekly sync, and `[p]` for instant test ping with in-flight concurrency locking.
  - **Opt-In by Design**: The engine is disabled by default and is turned on from the Settings tab, since running it spends real quota on every scheduled ping.
  - **Automated Configuration Migration**: Transparently detects and upgrades the exact keeper commands shipped by earlier builds to optimal headless, non-interactive flags (`--no-session-persistence`, `--skip-git-repo-check`, `--ephemeral`, `haiku`, `gpt-5.6-luna`), leaving hand-written commands untouched.
  - **Persistent Trigger History**: Last-fired timestamps are stored in `keeper_state.json` next to the config, so restarting the TUI no longer re-fires a scheduled ping, and a missed daily wakeup is only caught up within two hours of its configured time.
  - **Robust Provider & Reset Extraction**: Accurately extracts weekly reset timestamps from all provider formats including Antigravity's `Gemini (7d)` and `Claude (7d)` rate windows.

- **Observation History Database**: `tokenpulse.db` gained append-only history alongside the existing quota cache, which only ever kept one overwritten row per provider.
  - `quota_observations`: one row per rate window per poll (provider, timestamps, plan, account, window label, model family, full-precision used percent, reset time, period), giving an evenly spaced time series for later analysis.
  - **Model family dimension**: `RateWindow` now carries the model family a window meters, so Antigravity's separate Gemini and Claude quotas — and Claude's per-family Opus/Sonnet/Fable weekly windows — can be grouped without parsing display labels. Providers with a single pooled quota (Codex) and per-request-type quotas (Copilot) leave it empty.
  - `quota_credit_observations`: credit balances for providers that report them.
  - `quota_fetch_failures`: failed polls with the provider attributed, so recurring auth or network problems stay visible after the status bar clears.
  - `keeper_executions`: every ping with its trigger, model, prompt, command, duration, exit code and output. The Keeper panel now seeds itself from the database on launch and shows the newest 50, while the database keeps everything.
  - The database is now WAL-mode with a `user_version` migration path, matching `usage.db`.
- **npm Distribution**: `npm install -g @fangwangme/tokenpulse` installs a prebuilt binary with no Rust toolchain required. A launcher package resolves one of four per-platform binary packages through `optionalDependencies` (macOS x64/arm64, Linux x64/arm64). The release workflow builds, assembles, and publishes them from the `v*` tag; re-running a failed publish skips whatever already reached the registry. The unscoped name `tokenpulse` belongs to an unrelated npm package, hence the scope.
- **Release notes from the CHANGELOG**: the GitHub Release body is now the curated `CHANGELOG.md` section for that version rather than an auto-generated list of PR titles, falling back to generated notes when a version has no section.
- **Linux release binaries**: the release build matrix gained `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, built on `ubuntu-22.04` so they need only glibc 2.35.
- **File Logging**: tracing output goes to a daily rolling file in `~/.local/share/tokenpulse/log/`. Previously logging was only initialised when `RUST_LOG` was set and wrote to stdout, which is unusable under the TUI — a misbehaving background task left no evidence at all. Level is overridable via `TOKENPULSE_LOG` or `RUST_LOG`.

### Fixed
- The release workflow now builds both macOS targets on `macos-14`, cross-compiling the x86_64 one. It previously asked for the Intel `macos-13` runner, which GitHub no longer allocates: the job stayed queued for the 24-hour limit and was then cancelled, blocking the jobs that publish. No release had ever completed as a result.
- The README's `cargo install tokenpulse` never worked — the crate is not published on crates.io. Install instructions now cover npm and `cargo install --git`.
- `tokenpulse config show` now prints the Keeper section, and `config set keeper_engine=true|false` toggles the master switch, so the CLI matches what the Settings tab already exposed.
- The Keeper's next-trigger time no longer shows `--:--` on daylight-saving transition days. Resolving a wall-clock time returned nothing both when an hour is skipped and when it repeats.
- The log directory is resolved with `dirs::home_dir()` rather than `$HOME`, which is routinely unset on Windows and in slim containers and would have put the log under the working directory.
- Keeper pings no longer panic when an agent replies with non-ASCII text. The output snippet was truncated on a byte offset, which splits multi-byte characters; a panicking ping task never reported back, leaving that agent stuck on "Ping running..." and blocked from every later ping.
- Keeper output is sanitized before display: carriage returns, tabs, ANSI escapes and other control characters from agent CLIs are stripped instead of being written straight into the terminal, and multi-line replies are collapsed onto one line.
- Prompts and models are shell-escaped when substituted into a keeper command template, so a value containing a quote can no longer break out of the template and run as a command.
- A keeper ping that hits its 45s timeout now kills the CLI it started instead of leaving it running detached.

## [0.5.0] - 2026-08-15

Tagged but never published: the release workflow could not complete, so no
GitHub Release or npm package exists for this version. Everything intended for
it ships in 0.5.1.

## [0.4.11] - 2026-07-27

### Fixed
- Antigravity sub-agent usage reported as `gemini-3.6-flash-tiered` is now recorded under the base `gemini-3-6-flash` model instead of a separate tiered model, so it is no longer split from the base model or left at zero cost. A trailing `-tiered` routing suffix is stripped by Antigravity cache normalization, the shared model canonicalizer, and pricing lookup, while `tiered` elsewhere in a model id is preserved.
- The `refresh_quota` setting is now honored by the plain-text and `--json` outputs, which previously fetched every enabled provider's quota on each run regardless of the setting. When it is off they contact no quota API and show only unexpired cached snapshots, matching the TUI.

## [0.4.10] - 2026-07-13

### Fixed
- Claude quota authentication now prefers the current-user macOS Keychain credential, falls back only on explicit credential failures, losslessly persists OAuth rotations to their original source without narrowing scopes, and restarts once when a newer concurrent Claude Code login appears during a quota request.
- Codex quota windows are now labeled from each returned window's duration, so a weekly-only primary window is shown as `Weekly (7d)` without synthesizing a five-hour window.

## [0.4.9] - 2026-07-08

### Changed
- **TUI Layout & Alignments**:
  - Right-aligned numerical columns (Tokens, Cost, Input, Output, Cache, Msgs) in the Daily Breakdown view.
  - Added explicit spacers between columns in the Daily Breakdown view and doubled the spacer width between the Messages and 7-day Trend columns.
  - Increased `cost_width` from 8 to 10 in the Models and Overview views, ensuring large cost figures do not overflow and shift columns.
  - Restructured the Agent / Model Cost detail sidebar to use compact, fixed-width name padding (width 24), bringing the Cost column closer to the names and keeping cost values vertically aligned.
  - Aligned inline stats (T/I/O/CR/CW) in the Agent / Model Cost breakdown vertically by right-aligning values to a fixed width of 8.
- **TUI Value Formatting**:
  - Unified token metrics formatting to consistently display exactly two decimal places and a space before suffixes (e.g., `1.87 B`, `79.89 M`, `961.50 K`).
  - Ensured base values below 1,000 and zero format as floats with two decimal places and spacer padding (e.g., `719.00  `, `0.00  `), guaranteeing decimal point alignment when right-aligned.
  - Formatted costs over $1,000.00 to show commas and two decimal places (e.g., `"$1,499.42"`, `"$1,250.00"`).

## [0.4.8] - 2026-07-08

### Fixed
- Antigravity model alias normalization: fixed database normalization logic that stripped performance tiers (`-medium`, `-high`, `-low`, `-thinking`) from model IDs in the local database cache, preventing historical records (such as `gemini-3.5-flash-medium`) from being incorrectly consolidated.
- Quota progress bar: fixed visual layout rendering in `widgets/gauge.rs` that introduced visual gaps/holes in the progress bar when the expected progress marker was positioned inside the filled area of the bar under both `Used` and `Remaining` display modes.

## [0.4.7] - 2026-07-07

### Added
- Claude Code auto-refresh: support automatic OAuth access token refresh when expired or on 401 Unauthorized API responses, with persistence back to credentials file and macOS Keychain.
- Antigravity offline support: support falling back to Google Cloud Code API when Language Servers are closed or unresponsive, leveraging Google OAuth token refresh with client credentials and macOS Keychain storage.

## [0.4.6] - 2026-07-06

### Fixed
- Fixed Antigravity model mapping when using Gemini 3.5 Flash (Medium): when the Language Server returns `gemini-default` as the `responseModel` routing alias, the parser now falls back to `model` (e.g. `MODEL_PLACEHOLDER_M20`) and extracts the custom `modelDisplayName` to normalize it into `gemini-3.5-flash-medium`, preserving the model attributes correctly.

## [0.4.5] - 2026-07-04

### Added
- Auto theme now follows the OS appearance at runtime: when the theme preference is `auto`, each refresh (auto-refresh or manual `r`) re-detects the system light/dark setting on macOS and swaps the active theme if it changed, without a dedicated polling loop. The runtime probe intentionally skips the OSC11 terminal query to avoid colliding with the TUI's stdin reader; startup detection is unchanged.

## [0.4.4] - 2026-07-04

### Added
- Claude Code quota now surfaces a dedicated `Fable (7d)` rate-limit window when the provider reports per-model weekly limits scoped to the Fable model family, alongside the existing Session, Weekly, Sonnet, and Opus windows.
- Codex quota now fetches available manual rate-limit reset credits and shows each credit's local expiry in CLI and TUI quota output.
- Settings now shows the TokenPulse package version as read-only information.

### Changed
- CLI reset-credit output uses a `Banked reset` / `Expiration time` table with local timestamps.
- CLI and TUI reset-credit rows show the earliest expiring credits first and use compact per-credit expiry lines to avoid redundant summary text.
- Refreshed dashboard screenshots to match the current TUI.

## [0.4.3] - 2026-06-21

### Added
- `refresh_quota` configuration setting (default `true`) to enable / disable quota balance refreshes. When `false`, the TUI skips quota fetches on startup, auto-refresh, and manual `r`; the shared auto-refresh timer resets once the usage refresh finishes. Exposed via `config show`, `config set refresh_quota=true|false`, and the TUI Settings page.

## [0.4.2] - 2026-06-16

### Changed
- Models page: right-align trailing metrics columns (Tokens, Cost, Input, Output, Cache R/W, %, Msgs) for better readability, and insert explicit 1-character spacers between columns to prevent text overlap.
- Models page: dynamically allocate remaining width between Model and Agent columns. When available space is tight (<32 chars), guarantee a minimum width of 22 to fully display names like `Gemini 3.1 Pro Preview`. When space is ample, divide the space in a 2:3 ratio, letting the Agent column grow up to 40 characters so long agent lists are not truncated.

### Fixed
- Overview chart: replace discrete integer bar width steps with average floating point width stretching. The bars are dynamically stretched to cover the entire chart area width, eliminating the blank space on the right side of the chart.

## [0.4.1] - 2026-06-14

### Added
- Overview chart X axis: a few evenly spaced date ticks (always including the oldest and newest day) drawn in the previously-blank bottom row and centered on the actual bar columns, so dates stay aligned with their bars even when days are aggregated into buckets and the bars occupy less than the full chart width.

### Changed
- Display quota percentages as whole numbers (e.g. `used 42%`) on the gauge bar and the used/remaining detail line, reverting the two-decimal display introduced in 0.4.0 (the extra precision wasted horizontal space without adding meaning).
- Removed the per-card "Fetched <time>" footer on the Quota page; the footer status line already reports the unified refresh time.
- The Overview chart legend now lists only the provider colors — the first/last dates moved onto the new X axis.

### Fixed
- Unified the runtime auto-refresh timer so every page shows the same countdown. Usage and quota tracked separate `Instant`s that reset independently as each finished; because the local usage scan completes far faster than the networked quota fetch, the two drifted apart and the Quota page and the other pages showed different "Auto-refresh in" values. They now share one timer that triggers both scans together and only resets once both have finished (the 0.4.0 change had unified only the config setting, not the runtime timers).

## [0.4.0] - 2026-06-13

### Added
- Antigravity quota now reports both the 5-hour and weekly limits for each model group via the Language Server's `RetrieveUserQuotaSummary` endpoint, surfaced as `Gemini (5h)`/`Gemini (7d)` and `Claude (5h)`/`Claude (7d)` rate windows.
- Split the aggregated `Cache` column into separate `Cache R` (read) and `Cache W` (write) columns in the TUI Daily, Models, and Activity drill-down views (abbreviated `CR`/`CW` when the terminal is narrow), and added `cache_read_tokens`/`cache_write_tokens` to `ModelSummary`.
- Logged a warning when a model has no pricing catalog entry after the lazy-refresh attempt, making missing liteLLM/OpenRouter/models.dev entries easy to spot.

### Changed
- Rewrote the Antigravity quota fetcher around the modern multi-window endpoint, removing the legacy pool-name matching and reset-duration heuristics (and their tests).
- Fetch provider quotas in parallel (one Tokio task per provider) and parse provider usage concurrently, so a slow or blocking provider no longer stalls the others.
- Merged the separate quota/usage auto-refresh intervals into a single `auto_refresh_secs` setting (default 5 minutes); legacy `quota_auto_refresh_secs` is migrated automatically.
- Display quota percentages with two decimal places in the gauges and summaries.
- Redesigned the quota gauge: a pure progress bar on top (no inline number or time) with a detail line directly beneath it (`<countdown> used X.XX% remaining Y.YY% <pace>`) and a blank row between consecutive windows; the reset countdown and used/remaining figures are colored by the remaining balance and the pace indicator keeps its own color (omitted once the window is exhausted). Compact cards still collapse to a single-line bar.
- Raised the Quota Overview per-provider window cap from 3 to 4 so all four Antigravity windows are visible.
- Laid out the Activity selected-day overview as a consistent two-column grid, moving the session count onto its own row.
- Replaced the single aggregated `cache_tokens` column in the daily CSV export with explicit `cache_read_tokens` and `cache_write_tokens` columns.
- Rendered the TUI quota "Fetched" timestamp in local time instead of UTC.
- Lifted the `--no-tui` summary caps: the console summary now prints all models and up to a year (365 days) of daily totals.
- Remapped the TUI "Today" shortcut from `T` to `n` to avoid clashing with the lowercase `t` (sort by tokens) shortcut.
- Toned down the footer shortcut hints by dropping the highlighted background badge in favor of subtle colored key text.
- Stopped surfacing Claude Code extra-credit usage in the CLI/TUI quota output.

### Fixed
- Fixed the Daily tab `n` ("jump to today") shortcut selecting the wrong row: it located today's index in date order while the table was sorted by the active column (default cost ↓), so the highlight and scroll jumped to an unrelated row. The renderer and the shortcut now share one `sorted_daily_rows` order, so `n` lands on today's actual displayed row.
- Raised the Antigravity Language Server request timeout from 3s to 20s (matching the other quota providers); the `RetrieveUserQuotaSummary` call is a real backend round-trip, and the tight 3s budget made it fail intermittently.

## [0.3.3] - 2026-06-12

### Fixed
- Fixed model pricing reload issue by implementing on-demand lazy refresh for missing or zero-priced models.
- Rate-limited the on-demand refresh: it is skipped when the pricing cache was fetched within the last hour, so unknown or misparsed model ids cannot trigger repeated network fetches across reloads.
- Filtered pseudo-model ids (routing aliases like `auto-gemini-3`/`gemini-default`, internal features like `codex-auto-review`, and the `unknown` parser fallback) out of the on-demand refresh trigger and the zero-cost repair loop, so misparsed model ids can no longer keep either mechanism busy forever.
- Prevented database daily snapshot pollution by only saving snapshots with valid, non-zero pricing.
- Implemented mtime-based cache invalidation for the in-memory pricing cache, ensuring long-running processes (like the TUI) automatically detect updates on disk.
- Integrated pricing cache clearing and lazy-refresh state resetting during TUI reload operations.

## [0.3.2] - 2026-06-04

### Changed
- Replaced manual, one-off model mapping values in `explicit_model_alias` with dynamic, pattern-based canonicalization rules for Minimax, Kimi (Moonshot), DeepSeek, Qwen, Claude, Gemini, and GPT (OpenAI) models to automatically support future version releases.
- Retained strict explicit mappings for specific Gemini 3.0/3.1 preview models to prevent version-crossing pricing mismatches.
- Filtered out models with a total token count of 0 in the models list summary (`load_model_summaries`), matching the display behavior of the Daily detail and Activity logs.

---

## [0.3.1] - 2026-05-27

### Added
- Added `scan_antigravity` configuration setting (default `true`) to allow toggling active Antigravity session scanning and aliases synchronization while retaining historical Antigravity usage charts.
- Added `TOKENPULSE_CONFIG_PATH` environment variable override in `ConfigManager` to allow redirecting the configuration file location for test isolation and clean environments.
- Added input, output, and cache token breakdown columns in the TUI Models view, aligning colors and styling with the Daily breakdown view.

### Changed
- Inlined the Year Heatmap legend bar directly into the widget's footer, reclaiming vertical space in the TUI Activity view.
- Consolidated Antigravity model alias resolution, utilizing a new `format_normalization_fallback` function to handle diverse model naming shapes gracefully.
- Improved the TUI reload function to load and evaluate the configuration dynamically on every refresh/reload rather than using stale startup arguments.
- Refined TUI footer shortcut layouts using pill badges and cleaned up redundant help key prompts.
- Enabled dynamic layout for the Quota page to render pace status details under gauges when the terminal height permits.

### Fixed
- Fixed unit test suite pollution by ensuring tests do not load or mutate the host user's local `config.toml` file under `~/.local`.
- Resolved TUI freezing and executor starvation by isolating blocking subprocess queries and database writes inside `spawn_blocking`.
- Configured a SQLite `busy_timeout` of 5 seconds on all database connections to prevent write-lock conflicts during concurrent TUI auto-refresh cycles.

---

## [0.3.0] - 2026-05-26

### Added
- Added interactive Settings tab in the TUI, allowing runtime toggles of display modes, auto-refresh intervals, active theme preferences, and enabled providers.
- Unified command line entry points under the default `tokenpulse` command (previously split into separate usage/quota subcommands).
- Integrated background auto-refreshing for both quota and usage states in the TUI.

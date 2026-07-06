# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

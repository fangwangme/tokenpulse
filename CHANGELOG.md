# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

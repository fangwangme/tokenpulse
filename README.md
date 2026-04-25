# TokenPulse

TokenPulse is a Rust CLI for inspecting coding-agent quota and historical token usage from local machine data.

It has two core commands:

- `quota`: fetch remaining quota from supported providers on demand
- `usage`: parse local histories into a SQLite ledger and show a TUI or plain-text summary

## Current Coverage

Usage parsing currently supports:

- Claude Code
- Codex
- OpenCode
- Gemini CLI
- PI
- GitHub Copilot CLI

Quota fetching currently supports:

- Claude Code
- Codex
- Gemini CLI
- GitHub Copilot
- Antigravity

Notes:

- usage coverage is strongest today for Claude Code, Codex, OpenCode, and Copilot
- Gemini usage is still provisional
- Antigravity historical usage is not complete yet

## Features

- ledger-backed usage history stored in local SQLite
- per-day pricing snapshots so historical cost does not silently drift
- quota overview plus per-provider detail tabs
- usage dashboard with `Overview`, `Models`, `Daily`, and `Activity`
- usage `--json` output for scripts
- company-aware model coloring and agent/provider separation
- mouse-selectable activity heatmap drill-down
- plain-text mode for scripting and remote shells

## Install

Requirements:

- Rust toolchain
- local agent/session data on the same machine

Build the workspace:

```bash
cargo build --workspace
```

Run the CLI:

```bash
cargo run -p tokenpulse-cli -- --help
```

## Quick Start

Initialize config:

```bash
tokenpulse init
```

Check quota:

```bash
tokenpulse quota
tokenpulse quota -p claude
tokenpulse quota --no-tui
```

Inspect usage:

```bash
tokenpulse usage
tokenpulse usage --tui
tokenpulse usage --no-tui
tokenpulse usage --json
tokenpulse usage --since 2026-04-01
tokenpulse usage -p claude,codex,copilot
tokenpulse usage --refresh-days 2026-04-01:2026-04-09
tokenpulse usage --refresh-pricing
tokenpulse usage --rebuild-all
```

## Data Model

TokenPulse tracks two different concepts:

- `Agent`: the client tool you used, such as `Claude Code`, `Codex`, `OpenCode`, `Gemini CLI`, or `Copilot CLI`
- `Provider`: the backend/model company, such as `Anthropic`, `OpenAI`, `Google`, or `Copilot`

The usage dashboard keeps those separate so the same model family can be attributed across multiple agents.

## Local Storage

TokenPulse stores local state under standard cache/config locations:

- config: `~/.config/tokenpulse/config.toml`
- usage ledger: platform cache dir, typically `~/Library/Caches/tokenpulse/usage.sqlite3` on macOS
- pricing cache: `~/.cache/tokenpulse/pricing.json`

## Project Structure

```text
tokenpulse-core/   core parsing, pricing, quota, and ledger logic
tokenpulse-cli/    CLI entrypoints and TUI
docs/              design and module documentation
```

## Development

Run formatting and tests:

```bash
cargo fmt --all
cargo test --workspace
```

Primary design notes live in [docs/DESIGN.md](docs/DESIGN.md).

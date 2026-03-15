# Usage Module - Detailed Design

## Overview

Parse local session files from coding agents, calculate token usage and costs, display in fancy TUI dashboard.

## Architecture

```
usage/
├── mod.rs          # UnifiedMessage, TokenBreakdown, aggregation functions
├── scanner.rs      # parallel file discovery (walkdir + rayon)
├── claude.rs       # Claude Code JSONL parser
├── codex.rs        # Codex JSONL parser
├── opencode.rs     # OpenCode SQLite parser
└── pi.rs           # PI JSONL parser
```

## Data Model

```rust
pub struct TokenBreakdown {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
}

impl TokenBreakdown {
    pub fn total(&self) -> i64 {
        self.input + self.output + self.cache_read + self.cache_write + self.reasoning
    }
}

pub struct UnifiedMessage {
    pub client: String,         // "claude", "codex", "opencode", "pi"
    pub model_id: String,       // "claude-opus-4", "o3"
    pub provider_id: String,    // "anthropic", "openai"
    pub session_id: String,
    pub timestamp: i64,         // Unix ms
    pub date: String,           // "YYYY-MM-DD"
    pub tokens: TokenBreakdown,
    pub cost: f64,
}
```

## Session Parsers

### SessionParser Trait

```rust
pub trait SessionParser: Send + Sync {
    fn provider_name(&self) -> &str;
    fn session_paths(&self) -> Vec<PathBuf>;
    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>>;
}
```

### Claude Code Parser
- **Path:** `~/.claude/projects/**/*.jsonl`
- **Format:** One JSON object per line
- **Key fields:**
  - `type: "assistant"` → contains usage data
  - `message.model` → model ID
  - `message.usage.input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`
- **Dedup:** Composite key `messageId:requestId`
- **Reference:** tokscale `sessions/claudecode.rs`

### Codex Parser
- **Path:** `~/.codex/sessions/*.jsonl` or `$CODEX_HOME/sessions/`
- **Format:** JSONL with stateful model tracking
- **Key fields:** model changes tracked across lines, token deltas accumulated
- **Reference:** tokscale `sessions/codex.rs`

### OpenCode Parser
- **Path:** `~/.local/share/opencode/opencode.db`
- **Format:** SQLite database
- **Query:** `SELECT id, session_id, data FROM message`
- **Data field:** JSON-serialized message with model, tokens
- **Fallback:** Legacy JSON files if DB not found
- **Reference:** tokscale `sessions/opencode.rs`

### PI Parser
- **Path:** `~/.pi/agent/sessions/**/*.jsonl`
- **Format:** JSONL with header entry + message entries
- **Reference:** tokscale `sessions/pi.rs`

## Scanner

Parallel file discovery using `walkdir` + `rayon`:

1. For each provider, get session paths
2. Walk directories in parallel
3. Fast filter: check file mtime against `--since` date
4. Parse matching files in parallel with rayon
5. Collect and sort by timestamp

## Aggregation Functions

```rust
// Group messages by date
pub fn group_by_date(msgs: &[UnifiedMessage]) -> BTreeMap<String, Vec<&UnifiedMessage>>;

// Group by provider
pub fn group_by_provider(msgs: &[UnifiedMessage]) -> HashMap<String, Vec<&UnifiedMessage>>;

// Group by model
pub fn group_by_model(msgs: &[UnifiedMessage]) -> HashMap<String, Vec<&UnifiedMessage>>;

// Daily cost summary
pub struct DailySummary {
    pub date: String,
    pub total_cost: f64,
    pub total_tokens: i64,
    pub by_provider: HashMap<String, f64>,
}

// Overall summary
pub struct UsageSummary {
    pub total_cost: f64,
    pub total_tokens: i64,
    pub active_days: usize,
    pub avg_daily_cost: f64,
    pub max_daily_cost: f64,
    pub by_provider: Vec<ProviderSummary>,
    pub by_model: Vec<ModelSummary>,
}
```

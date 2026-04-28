# tokscale 项目分析

> 项目地址: https://github.com/junhoyeo/tokscale
> 本地路径: /tmp/tokscale

## 1. 项目概览

Rust CLI 工具 + TUI 仪表盘，追踪 14+ 个 AI 编码代理的 token 使用量和费用。重点在**使用量追踪** (usage tracking)，而非余量查询 (quota)。

**Workspace 结构:**
- `tokscale-core`: 核心库 — 解析、扫描、聚合 session 数据
- `tokscale-cli`: CLI 二进制 — TUI (ratatui) + 表格输出

**性能优化:**
- SIMD 加速 JSON 解析 (simd-json)
- rayon 并行文件扫描 + 并行聚合
- 磁盘缓存定价数据 (LiteLLM, OpenRouter)
- XDG 兼容缓存目录

## 2. 支持的客户端 (14个)

| # | 客户端 | 数据格式 | 存储路径 |
|---|--------|----------|---------|
| 0 | OpenCode | SQLite + Legacy JSON | `~/.local/share/opencode/` |
| 1 | Claude | JSONL | `~/.claude/projects/` |
| 2 | Codex | JSONL (有状态) | `~/.codex/sessions/` |
| 3 | Cursor | CSV | `~/.config/tokscale/cursor-cache/` |
| 4 | Gemini | JSON | 待确认 |
| 5 | Amp | JSON | 待确认 |
| 6 | Droid | Settings JSON | 待确认 |
| 7 | OpenClaw | JSONL | 待确认 |
| 8 | Pi | JSONL | 待确认 |
| 9 | Kimi | wire.jsonl | 待确认 |
| 10 | Qwen | JSONL | 待确认 |
| 11 | RooCode | ui_messages.json | VSCode Extension |
| 12 | KiloCode | ui_messages.json | VSCode Extension |
| 13 | Mux | session-usage.json | 待确认 |

## 3. 核心数据模型

### 统一消息 (最重要的中间结构)

```rust
UnifiedMessage {
    client: String,              // "claude", "codex", etc.
    model_id: String,            // "claude-3-5-sonnet"
    provider_id: String,         // "anthropic", "openai"
    session_id: String,
    timestamp: i64,              // Unix ms
    date: String,                // "YYYY-MM-DD"
    tokens: TokenBreakdown {
        input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
        reasoning: i64,          // 扩展推理 token
    },
    cost: f64,                   // USD
    agent: Option<String>,       // 代理名称
    dedup_key: Option<String>,   // 去重键
}
```

### 聚合结构

```rust
DailyContribution {
    date: String,
    totals: DailyTotals,        // 汇总 token/cost/消息数
    intensity: u8,               // 0-4 (热力图)
    token_breakdown: TokenBreakdown,
    clients: Vec<ClientContribution>,
}

GraphResult {
    meta: GraphMeta,             // 版本, 日期范围, 处理时间
    summary: DataSummary,        // 总计和统计
    years: Vec<YearSummary>,
    contributions: Vec<DailyContribution>,
}
```

## 4. Claude Code 使用量解析

**存储:** `~/.claude/projects/` 下的 JSONL 文件

**数据结构:**
```rust
ClaudeEntry {
    entry_type: String,           // "assistant" 才处理
    timestamp: Option<String>,    // RFC3339
    message: Option<ClaudeMessage> {
        model: Option<String>,
        usage: Option<ClaudeUsage> {
            input_tokens: Option<i64>,
            output_tokens: Option<i64>,
            cache_read_input_tokens: Option<i64>,
            cache_creation_input_tokens: Option<i64>,
        },
        id: Option<String>,      // 去重用
    },
    requestId: Option<String>,   // 去重用
}
```

**去重策略:** `(messageId:requestId)` 复合键

**Headless 支持:** 替代 JSON 格式, 事件流:
- `message_start` / `message_delta` / `message_stop`
- 有状态的跨事件 token 累积

## 5. Codex 使用量解析

**存储:** `~/.codex/sessions/` + `~/.codex/archived_sessions/` (JSONL)

**有状态解析** (关键复杂度):
```rust
CodexEntry {
    entry_type: String,           // "session_meta", "turn_context", "event_msg"
    payload: Option<CodexPayload> {
        payload_type: Option<String>, // "token_count"
        model: Option<String>,
        model_provider: Option<String>,
        agent_nickname: Option<String>,
        info: Option<CodexInfo> {
            last_token_usage: ...,    // 主增量源
            total_token_usage: ...,   // 验证基线
        },
    },
}
```

**Token 计算策略:**
- 使用 `last_token_usage` 作为主增量 (非累积总量)
- `total_token_usage` 仅用于去重/单调性检查
- 检测过时回归 (totals 乱序到达)
- 过滤零 token 快照防止膨胀
- Cache token 钳位防止膨胀

## 6. 定价与费用计算

**定价数据源 (优先级):**
1. Cursor 覆盖 (硬编码)
2. LiteLLM 数据 (主要, 异步获取)
3. OpenRouter 数据 (补充)
4. 磁盘缓存 (`~/.cache/tokscale/`)

**费用公式:**
```
cost = (input × input_price) + (output × output_price)
     + (cache_read × cache_read_price) + (cache_write × cache_write_price)
     + (reasoning × reasoning_price)
```

**模型名标准化:**
- 小写化
- 去除日期后缀 (`claude-3-5-sonnet-20250101` → `claude-3-5-sonnet`)
- 点号转短横 (`claude.3.5` → `claude-3-5`)

## 7. 解析管道

```
Scanner (并行目录遍历)
    → walkdir + rayon par_bridge
    → 按 pattern 匹配 session 文件
    → ScanResult (每客户端索引向量)

Parser (SIMD JSON)
    → simd_json 加速解析
    → JSONL 逐行解析 + 错误恢复

Session Parsers (每客户端专属)
    → par_iter() → flat_map() → collect()
    → Vec<UnifiedMessage>

Aggregator (并行 Map-Reduce)
    → par_iter().fold().reduce()
    → DailyContribution (按日期排序)
```

## 8. CLI 界面

**主模式:**
```bash
tokscale              # 交互式 TUI (默认)
tokscale models       # 模型用量报告
tokscale monthly      # 月度分解
tokscale wrapped      # 年度总结
```

**过滤:**
- 客户端: `--claude`, `--codex`, `--cursor`, etc.
- 日期: `--today`, `--week`, `--month`, `--since`, `--until`, `--year`
- 分组: `--group-by model|client,model|client,provider,model`

**输出格式:** TUI (ratatui) / Table (comfy-table) / JSON

## 9. 关键依赖

| 依赖 | 用途 |
|------|------|
| rayon | 并行文件扫描 + 聚合 |
| simd-json | SIMD 加速 JSON 解析 |
| walkdir | 递归目录遍历 |
| rusqlite | SQLite (OpenCode) |
| ratatui + crossterm | TUI 框架 |
| comfy-table | 表格输出 |
| reqwest + tokio | 异步 HTTP (定价获取) |
| chrono | 时间戳解析 |
| clap | CLI 参数 |

## 10. 对 TokenPulse 的参考价值

| 方面 | 参考价值 |
|------|---------|
| **Claude 使用量解析** | ⭐⭐⭐ JSONL 解析 + 去重 + headless 支持 |
| **Codex 使用量解析** | ⭐⭐⭐ 有状态解析 + 增量计算 + 回归检测 |
| **统一数据模型** | ⭐⭐⭐ UnifiedMessage + TokenBreakdown 设计 |
| **并行处理架构** | ⭐⭐ rayon fold/reduce 模式 |
| **定价计算** | ⭐⭐ 多数据源 + 缓存 + 模型名标准化 |
| **TUI 界面** | ⭐⭐ ratatui 热力图 + 日历视图 |
| **Quota 查询** | ❌ 不涉及 — 纯使用量追踪 |

> **注意:** tokscale 专注于**历史使用量追踪**，不做**实时余量查询**。TokenPulse 的 quota 模块应参考 CodexBar + OpenUsage。

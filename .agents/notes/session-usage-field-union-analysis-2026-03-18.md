# Session Usage 字段并集与指标设计

日期：2026-03-18

## 目标

你现在要的不是“先做一个最小 parser”，而是先把几个参考项目里已经证明有用的字段全部摸出来，做一个并集，然后分层处理：

- 第一层：统计必须字段
- 第二层：高价值上下文字段
- 第三层：原始冗余字段备份

这样后面无论是做每天 token 花费、按模型统计、按 agent 统计、调试缓存错误，还是修 parser，都不会因为一开始字段收得太少而返工。

---

## 1. 这次对比涉及的项目

### 当前项目

- `tokenpulse`

### 参考项目

- `tokscale`
- `CodexBar`
- `openusage`
- `antigravity-trajectory-extractor`

其中和 session usage 字段直接最相关的是：

- `tokenpulse`
- `tokscale`
- `CodexBar`

`openusage` 对 Claude/Codex usage 主要是调用 `ccusage`，本身不是底层 parser。

`antigravity-trajectory-extractor` 主要提供 Antigravity 的本地发现/解码路径，不是 token usage parser，但它补充了本地 session identity / workspace / trajectory 相关字段。

---

## 2. 目前 tokenpulse 的统一结构过于精简

当前 `tokenpulse` 的 `UnifiedMessage` 只有：

- `client`
- `model_id`
- `provider_id`
- `session_id`
- `timestamp`
- `date`
- `tokens`
  - `input`
  - `output`
  - `cache_read`
  - `cache_write`
  - `reasoning`
- `cost`

这能做最基本统计，但不够支撑后续需求，因为缺少：

- 去重键
- agent / mode / source
- 原始事件类型
- workspace / cwd / project
- cost 来源
- parser 版本
- 原始字段备份

所以如果现在就做缓存，很容易把信息压扁，后面难以纠错。

---

## 3. 各项目里实际出现过的有用字段

下面不是“理论上可能有”，而是这次代码里明确读过、用过、或者证明值得保留的字段。

## 3.1 通用身份字段

这些字段在多个项目里都出现，应该纳入统一层。

- `client`
  - 例：`claude` / `codex` / `opencode` / `pi` / `gemini` / `amp` / `qwen` / `kimi`
- `provider_id`
  - 例：`anthropic` / `openai` / `google` / `moonshot` / `qwen`
- `model_id`
- `session_id`
- `timestamp_ms`
- `date`
- `source_path`
- `source_format`
  - 例：`jsonl` / `json` / `sqlite` / `protobuf` / `ls_rpc`

## 3.2 去重 / 事件定位字段

这些字段不一定用于最终展示，但非常重要，应该尽量保留。

- `message_id`
  - Claude、Amp、OpenCode、Kimi 等都有
- `request_id`
  - Claude 明确使用
- `parent_id`
  - Pi 有
- `dedup_key`
  - `tokscale` 已经显式保留
- `event_type`
  - 例：
    - Claude: `assistant` / `message_start` / `message_delta` / `message_stop`
    - Codex: `session_meta` / `turn_context` / `event_msg`
    - Kimi: `StatusUpdate`
    - RooCode: `api_req_started`
- `payload_type`
  - Codex 的 `token_count`

## 3.3 token 与成本字段

这是统计核心，必须统一成标准口径。

- `input_tokens`
- `output_tokens`
- `cache_read_tokens`
- `cache_write_tokens`
- `reasoning_tokens`
- `tool_tokens`
  - Gemini/Mux 有出现；即使暂时不入总账，也建议备份
- `total_tokens_reported`
  - 一些来源会给总 token，但不一定可靠
- `cost_usd_reported`
  - 有些来源直接给 cost
- `cost_usd_calculated`
  - 我们自己算的 cost
- `cost_source`
  - `reported`
  - `calculated`
  - `calculated_frozen_daily_snapshot`
- `pricing_version`
- `pricing_day`

## 3.4 上下文字段

这些字段对“按 agent/工作区/项目统计”和调试很有价值。

- `agent`
  - `tokscale` 已保留
  - OpenCode / Codex / RooCode / Kimi 周边都可能有
- `mode`
  - OpenCode 明确有
- `role`
  - assistant / user / system
- `source`
  - 例：Codex `exec`
- `model_provider`
  - Codex `session_meta` 里有
- `workspace_path`
  - Antigravity / Pi / Claude 未来都可能需要
- `cwd`
  - Pi 已出现
- `project_hash`
  - Gemini 有
- `thread_id`
  - Amp 类结构里可对应 `thread.id`

## 3.5 时间相关辅助字段

这些字段对日聚合和增量扫描很关键。

- `created_at`
- `completed_at`
- `last_updated_at`
- `file_mtime_ms`
- `provider_lock_timestamp`
- `reset_time`
  - quota 侧更常见，但某些本地数据也可能出现

## 3.6 供应商特有但值得保留的字段

### Claude

- `requestId`
- `message.id`
- `message.model`
- `message.usage.input_tokens`
- `message.usage.output_tokens`
- `message.usage.cache_read_input_tokens`
- `message.usage.cache_creation_input_tokens`
- Vertex 识别相关 metadata

### Codex

- `payload.info.last_token_usage`
- `payload.info.total_token_usage`
- `payload.model_provider`
- `payload.agent_nickname`
- `payload.model_info.slug`
- headless 路径中的：
  - `usage.input_tokens`
  - `usage.output_tokens`
  - `usage.cached_input_tokens`
  - `usage.cache_read_input_tokens`
  - `usage.prompt_tokens`
  - `usage.completion_tokens`

### OpenCode

- `id`
- `sessionID`
- `modelID`
- `providerID`
- `cost`
- `tokens.input`
- `tokens.output`
- `tokens.reasoning`
- `tokens.cache.read`
- `tokens.cache.write`
- `time.created`
- `time.completed`
- `agent`
- `mode`

### Pi

- `id`
- `parentId`
- `cwd`
- `message.provider`
- `usage.totalTokens`

### Gemini

- `sessionId`
- `projectHash`
- `startTime`
- `lastUpdated`
- `stats`
- `tokens.cached`
- `tokens.thoughts`
- `tokens.tool`
- `tokens.total`

### Amp

- `credits`
- `usageLedger.events`
- `operationType`
- `messageId`
- `thread.id`
- `thread.created`

### RooCode / KiloCode

- `say`
- `text`
- `apiProtocol`
- `cost`
- `tokensIn`
- `tokensOut`
- `cacheReads`
- `cacheWrites`
- 从历史文件中抽出的：
  - `model`
  - `slug`
  - `name`

### OpenClaw

- `model_change`
- `modelId`
- `provider`
- `usage.cost.total`
- `sessionFile`

### Droid / Factory

- `providerLock`
- `providerLockTimestamp`
- `thinkingTokens`
- `tokenUsage`

### Mux

- `byModel`
- `lastRequest`
- 各 bucket 的：
  - `tokens`
  - `cost_usd`

### Qwen

- `usageMetadata.promptTokenCount`
- `usageMetadata.candidatesTokenCount`
- `usageMetadata.thoughtsTokenCount`
- `usageMetadata.cachedContentTokenCount`

### Kimi

- `token_usage.input_other`
- `token_usage.output`
- `token_usage.input_cache_read`
- `token_usage.input_cache_creation`
- `message_id`

### Antigravity

虽然这次不是 token usage parser，但从本地可读出：

- `cascade_id`
- `trajectory_id`
- `workspace_id`
- `workspace_paths`
- `title`
- `last_step_index`
- `last_modified`
- `generator_metadata`

如果以后做 Antigravity session usage / session history 联动，这些 identity 字段值得保留。

---

## 4. 建议的字段模型

不要把所有字段都塞进一个扁平结构。

建议拆成 4 层。

## 4.1 Layer A: 统计主表必备字段

这是所有来源都要尽量归一化出来的字段。

- `record_id`
- `client`
- `provider_id`
- `model_id`
- `session_id`
- `timestamp_ms`
- `date`
- `input_tokens`
- `output_tokens`
- `cache_read_tokens`
- `cache_write_tokens`
- `reasoning_tokens`
- `cost_usd`
- `cost_source`
- `pricing_version`
- `pricing_day`

这是做 daily aggregation 的核心层。

## 4.2 Layer B: 分析增强字段

这些字段默认也建议落库，因为对后续统计有价值。

- `agent`
- `mode`
- `role`
- `workspace_path`
- `cwd`
- `project_id_or_hash`
- `message_id`
- `request_id`
- `parent_id`
- `dedup_key`
- `event_type`
- `payload_type`
- `source_path`
- `source_format`
- `source_file_mtime_ms`

## 4.3 Layer C: 来源原始关键字段镜像

建议新增一个 `raw_key_fields_json`，只保留原始来源里最关键但暂时没归一化的字段。

例如：

- Claude:
  - `message.id`
  - `requestId`
- Codex:
  - `last_token_usage`
  - `total_token_usage`
  - `model_provider`
  - `agent_nickname`
- OpenCode:
  - `mode`
  - `time.completed`
- Gemini:
  - `projectHash`
  - `tool`
  - `total`

这个字段不是为了最终查询，而是为了以后修 parser 和查账。

## 4.4 Layer D: 原始事件备份

建议再加一个 `raw_event_json` 或 `raw_event_ref`：

- 小记录可直接存 JSON
- 大记录只存摘要和原始文件偏移

不要默认把所有大 payload 全量复制一遍，否则缓存会爆。

所以更好的方案是：

- 存 `source_path`
- 存 `source_offset` / `row_id` / `message_id`
- 存 `raw_event_excerpt_json`

只在必要时能回放，不强制每条都存全量原文。

---

## 5. 建议的“有用字段并集”

如果现在要先做一个并集，我建议先定成下面这组。

### 5.1 核心并集

- `client`
- `provider_id`
- `model_id`
- `session_id`
- `timestamp_ms`
- `date`
- `input_tokens`
- `output_tokens`
- `cache_read_tokens`
- `cache_write_tokens`
- `reasoning_tokens`
- `cost_usd`

### 5.2 身份与去重并集

- `message_id`
- `request_id`
- `parent_id`
- `dedup_key`
- `event_type`
- `payload_type`

### 5.3 上下文并集

- `agent`
- `mode`
- `role`
- `workspace_path`
- `cwd`
- `project_hash`
- `thread_id`
- `source`
- `model_provider`

### 5.4 审计并集

- `source_path`
- `source_format`
- `source_offset`
- `source_row_id`
- `source_file_mtime_ms`
- `parser_version`
- `pricing_version`
- `pricing_day`
- `cost_source`
- `raw_key_fields_json`

---

## 6. 统计指标建议

你问“统计的话都需要哪些指标”，我建议分成 3 组。

## 6.1 基础指标

- `daily_total_tokens`
- `daily_input_tokens`
- `daily_output_tokens`
- `daily_cache_read_tokens`
- `daily_cache_write_tokens`
- `daily_reasoning_tokens`
- `daily_total_cost_usd`
- `daily_session_count`
- `daily_message_count`
- `daily_active_models`
- `daily_active_clients`

## 6.2 结构性指标

- `input_output_ratio`
- `cache_hit_ratio`
  - `cache_read / (input + cache_read)`
- `reasoning_ratio`
  - `reasoning / (output + reasoning)`
- `avg_cost_per_1k_tokens`
- `avg_cost_per_message`
- `top_model_share`
- `top_provider_share`

## 6.3 调试 / 质量指标

- `dedup_drop_count`
- `missing_model_count`
- `missing_provider_count`
- `missing_timestamp_count`
- `zero_cost_count`
- `reported_cost_vs_calculated_cost_delta`
- `reparsed_days_count`
- `incremental_scan_hit_ratio`

这第三组非常重要，因为缓存体系上线以后，最先暴露问题的不是业务指标，而是“今天为什么比昨天少了 20%”这种质量问题。

---

## 7. 解析策略建议

你说“JS 文件或者类似的，冗余信息多”，所以建议不是“全字段硬编码映射”，而是：

## 7.1 先做标准化主结构

统一输出：

- 身份
- 时间
- token breakdown
- cost

## 7.2 再做 provider-specific extra bag

每个 provider 允许输出：

- `extra.identity`
- `extra.usage`
- `extra.context`
- `extra.raw_keys`

例如：

- Claude:
  - `extra.identity.request_id`
- Codex:
  - `extra.usage.last_token_usage`
  - `extra.usage.total_token_usage`
- OpenCode:
  - `extra.context.agent`
  - `extra.context.mode`

## 7.3 最后再做日级聚合缓存

不要把 day cache 建在“过于压缩的消息结构”上。

建议顺序：

1. 先存标准化 message record
2. 再按天聚合
3. 再把价格快照绑定到 day/model/provider

这样出错时可以回放，不会只能看最终日汇总。

---

## 8. 备份建议

“备份一下”我建议不是备份整个源文件，而是备份 3 样东西：

### 8.1 标准化消息表

这是主账本。

### 8.2 原始关键字段镜像

`raw_key_fields_json`

只放关键补充字段，不放整块冗余内容。

### 8.3 可回放定位信息

- `source_path`
- `source_offset`
- `source_row_id`
- `message_id`

这样以后需要人工复盘时，可以精确跳回原始来源。

---

## 9. 对 tokenpulse 的直接建议

如果接下来在 `tokenpulse` 落地，我建议把当前 `UnifiedMessage` 升级为两层结构：

### 9.1 统一主结构

- `client`
- `provider_id`
- `model_id`
- `session_id`
- `timestamp_ms`
- `date`
- `token_breakdown`
- `cost`
- `cost_source`
- `pricing_day`
- `pricing_version`

### 9.2 扩展结构

- `message_id`
- `request_id`
- `parent_id`
- `dedup_key`
- `agent`
- `mode`
- `role`
- `workspace_path`
- `cwd`
- `project_hash`
- `event_type`
- `payload_type`
- `source_path`
- `source_format`
- `source_offset`
- `raw_key_fields_json`

这样做有三个好处：

- 先满足统计
- 不丢调试能力
- 后续扩 provider 不需要再改一轮底层表结构

---

## 10. 最终结论

这次看下来，真正应该先解析并保留的，不只是 token 五元组。

最少要同时保住 4 类信息：

- 统计值
- 身份与去重
- 上下文
- 审计与回放线索

如果只保留：

- `model`
- `provider`
- `date`
- `tokens`
- `cost`

那么短期能跑，长期一定会卡在：

- 去重修不了
- parser 升级没法回放
- agent / workspace 维度做不了
- 历史缓存错了难以 override 校正

所以建议现在就先按“字段并集 + 分层存储”设计，而不是继续维持当前的极简 `UnifiedMessage`。


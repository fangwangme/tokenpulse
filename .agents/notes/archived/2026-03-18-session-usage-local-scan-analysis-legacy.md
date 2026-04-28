# Session Usage 本地扫描方案分析

日期：2026-03-18

## 结论

可以。`session usage` 这块完全可以设计成只扫描本地数据，不请求任何远程接口。

从这次对比看：

- `tokenpulse` 的 usage 本身已经主要是本地扫描。
- `tokscale` 和 `CodexBar` 的 session usage 也是本地扫描。
- `openusage` 自己不实现扫描器，但 Claude/Codex 的 token usage 也是通过本地 `ccusage` CLI 扫 session 文件得出的，不是调 provider 远端 usage API。

真正会引入远端请求的，主要是两类：

- `quota` / 当前套餐剩余额度查询
- `pricing` / 模型价格表拉取与刷新

如果你的目标是“每天 token 花费统计”和“历史价格固定”，那么 usage 完全可以本地化，pricing 也应该改成本地快照化，而不是每次按最新远端价格重算历史。

## 1. tokenpulse 当前实现

### 1.1 usage 数据来源

`tokenpulse` 当前 usage 统计主要来自本地文件/本地数据库：

- Claude: `~/.claude/projects/**/*.jsonl`
- Codex: `~/.codex/sessions/**/*.jsonl` 和 `CODEX_HOME/sessions/**/*.jsonl`
- OpenCode: `~/.local/share/opencode/opencode.db`
- Pi: 本地 jsonl

对应代码：

- `tokenpulse-core/src/usage/claude.rs`
- `tokenpulse-core/src/usage/codex.rs`
- `tokenpulse-core/src/usage/opencode.rs`
- `tokenpulse-core/src/usage/scanner.rs`

### 1.2 当前问题

#### 扫描过滤过粗

`scanner::discover_files()` 是按文件 `mtime >= since` 过滤。

这会有两个问题：

- 如果老文件里后来追加了新消息，会被扫到整文件，导致历史重复解析成本高。
- 如果文件日期结构本来就很清楚，仍然递归全目录，浪费 IO。

#### Codex 解析规则偏简化

当前 `tokenpulse` 的 Codex 解析逻辑比较脆弱：

- 只认 `model` / `tokens` / `session` / `message|response` / `end|complete`
- 用累计 token 直到结束事件才吐一条消息
- 没处理 `last_token_usage` / `total_token_usage`
- 没处理 headless 格式
- 没处理 reasoning token
- 没处理归档目录

这会导致和真实使用记录存在偏差。

#### OpenCode 没有 since 过滤

`OpenCodeSessionParser::parse_sessions()` 里直接扫整个 SQLite，没有按时间裁剪。

#### 历史价格会漂移

`tokenpulse-core/src/pricing/litellm.rs` 每 24 小时从 LiteLLM 拉一次最新价格，之后再用当前价格重算历史消息 cost。

结果是：

- 同一段历史，今天算和明天算可能不一样
- 模型价格改了以后，历史账单会漂移

这正是你现在不满意的点。

## 2. tokscale 的做法

`tokscale` 是这几个项目里最接近你目标的参考实现。

### 2.1 扫描范围控制比 tokenpulse 好

核心在：

- `crates/tokscale-core/src/scanner.rs`

特点：

- 按 client 预定义路径扫描，不是盲扫
- Codex 支持：
  - `~/.codex/sessions`
  - `~/.codex/archived_sessions`
  - headless 目录
- OpenCode 同时支持：
  - SQLite 新格式
  - legacy JSON 旧格式
- 对有明确文件名模式的 client 使用 pattern 匹配

这已经比 `tokenpulse` 当前的“单根目录递归 + mtime 过滤”稳很多。

### 2.2 OpenCode 做了双源去重和 migration cache

核心在：

- `crates/tokscale-core/src/sessions/opencode.rs`
- `crates/tokscale-core/src/lib.rs`

特点：

- 先读 SQLite
- 再读 legacy JSON
- 以 `message id` / `dedup_key` 去重
- 额外记录 `opencode-migration.json`，当确认 JSON 已全部迁移到 SQLite 后，可以减少对旧目录的重复扫描

这类“按源去重 + 迁移状态缓存”很适合你说的“按天缓冲、跳过历史”思路。

### 2.3 Claude 解析更完整

核心在：

- `crates/tokscale-core/src/sessions/claudecode.rs`

比 `tokenpulse` 多的能力：

- 同时支持标准 JSONL 和 headless JSON/流式输出
- 用 `message.id + requestId` 去重
- 对 headless 流事件：
  - `message_start`
  - `message_delta`
  - `message_stop`
  做状态聚合
- 对 usage 字段取 `max`，避免 cumulative 流日志重复累计

这能显著降低“统计结果每次不一样”的概率。

### 2.4 Codex 解析明显更成熟

核心在：

- `crates/tokscale-core/src/sessions/codex.rs`

关键规则：

- 主要消费 `event_msg` 里的 `token_count`
- 优先使用 `last_token_usage` 作为本次增量
- `total_token_usage` 只用于建立基线和回归检测
- 如果 total 回退，看起来像 stale regression，则跳过，避免重复计数
- 支持 `session_meta` 中的 provider / agent
- 支持 headless 格式中常见的 `usage` 嵌套路径
- 把 cached token 从 input 中拆出来，避免总量膨胀

这套逻辑比 `tokenpulse` 当前实现更接近真实使用量。

### 2.5 价格仍然不是历史冻结

虽然 `tokscale` 的 usage 扫描是本地的，但 pricing 还是动态来源：

- LiteLLM
- OpenRouter
- 少量 Cursor overrides

核心在：

- `crates/tokscale-core/src/pricing/mod.rs`
- `crates/tokscale-core/src/pricing/lookup.rs`

结论：

- 它解决了“模型匹配”和“多 provider 价格 lookup”问题
- 但没有解决“历史价格冻结”
- 重新跑一遍历史，cost 仍可能随价格表变化

所以 `tokscale` 更适合参考“扫描规则”和“解析稳定性”，不适合直接照搬 pricing 策略。

## 3. CodexBar 的做法

`CodexBar` 里 vendored 的 `CostUsage` 非常值得参考，因为它已经做了“按文件增量缓存”。

核心文件：

- `Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift`
- `Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+Claude.swift`
- `Sources/CodexBarCore/Vendored/CostUsage/CostUsageCache.swift`
- `Sources/CodexBarCore/Vendored/CostUsage/CostUsagePricing.swift`

### 3.1 它最有价值的是增量缓存

缓存结构里记录：

- `lastScanUnixMs`
- 每个文件的：
  - `mtimeUnixMs`
  - `size`
  - `days`
  - `parsedBytes`
  - `lastModel`
  - `lastTotals`
  - `sessionId`

这意味着：

- 文件没变就直接跳过
- 文件只是追加了内容，就从 `parsedBytes` 开始增量解析
- 不需要每次重扫整个历史

这正是你想要的“按天做缓冲，忽略历史”的基础设施。

### 3.2 Codex 扫描会利用日期结构

CodexBar 对 Codex 的文件发现做了两层优化：

- 优先扫 `sessions/YYYY/MM/DD/*.jsonl` 这种日期分区目录
- 再扫 flat 目录里文件名带日期的 `jsonl`

而且只扫：

- `since - 1 day`
- 到 `until + 1 day`

这样做的原因是给跨时区/边界日期留缓冲，但总体仍然大幅缩小了扫描范围。

这是目前最接近你说的“如果路径本身有明显日期结构，就忽略历史数据”的实现。

### 3.3 Claude 也是增量解析

Claude 这边：

- 始终 walk 目录树
- 但每个文件用 mtime/size + `parsedBytes` 做增量

注意它还专门修掉了一个问题：

- 不能只看 root directory 的 mtime
- 因为子目录下文件变化不一定会反映到根目录 mtime

这个经验对 `tokenpulse` 很重要。

### 3.4 价格是本地硬编码表，历史稳定

`CostUsagePricing.swift` 直接内置了 Claude / Codex 的价格表。

优点：

- 历史结果稳定
- 不依赖远端
- 每次重算不会变

缺点：

- 需要手工更新
- 覆盖面有限

如果你的优先级是“历史账单固定”，这反而是正确方向。

## 4. openusage 的做法

`openusage` 需要分两块看。

### 4.1 quota/probe 是远端

Claude / Codex / Antigravity 的 quota 展示，确实会请求 provider 的远端或本地 LS。

这部分不是 session usage。

### 4.2 Claude/Codex 的 token usage 是本地 ccusage

核心在：

- `plugins/claude/plugin.js`
- `plugins/codex/plugin.js`
- `src-tauri/src/plugin_engine/host_api.rs`
- `docs/plugins/api.md`

关键点：

- 插件通过 `ctx.host.ccusage.query(...)`
- 宿主实际调用的是：
  - `ccusage@18.0.10`
  - `@ccusage/codex@18.0.10`
- CLI 从本地 session 文件计算 daily usage
- `openusage` 只做展示层：
  - Today
  - Yesterday
  - Last 30 Days

所以对你的问题，`openusage` 给出的结论其实也是：

- session usage 可以本地扫描
- quota 才需要远端

## 5. antigravity-trajectory-extractor 的启发

仓库：

- `/private/tmp/antigravity-trajectory-extractor`

核心文件：

- `src/antigravity_trajectory/extractor.py`

### 5.1 它的原则是 cache for discovery, live RPC for content

它不是做 token usage 的，但它给了我们 Antigravity 本地解密/发现路径：

- 本地 state DB:
  - `~/Library/Application Support/Antigravity/User/globalStorage/state.vscdb`
- 本地会话 cache:
  - `~/.gemini/antigravity/conversations/*.pb`
- 运行中的本地 `language_server`

### 5.2 它如何“解”

#### 读取 state.vscdb

从 SQLite 里读：

- `antigravityUnifiedStateSync.trajectorySummaries`

然后：

- base64 decode
- 自己实现最小 protobuf wire parser
- 从 bytes 里提取 `cascade_id`、workspace、title、timestamp

#### 读取 conversations/*.pb

它不尝试完整离线还原 transcript，而是：

- 从 `.pb` 文件名中拿候选 `cascade_id`
- 再用本地运行中的 language server RPC 去验证和取内容

#### 调本地 RPC，不走远端

它通过：

- `ps` 找 `language_server`
- 解析进程参数里的：
  - `workspace_id`
  - `csrf_token`
  - `extension_server_port`
- `lsof` 找监听端口
- 对本地 `127.0.0.1` 端口调用：
  - `GetAllCascadeTrajectories`
  - `GetCascadeTrajectory`
  - `GetCascadeTrajectorySteps`
  - `GetCascadeTrajectoryGeneratorMetadata`

这是“本地解码，不打 provider 远端”的典型实现。

### 5.3 对 tokenpulse 的意义

如果以后 `tokenpulse` 要补 Antigravity 的 session usage / trajectory 侧能力，可以优先考虑：

- 本地 SQLite + 本地 protobuf cache 发现候选 session
- 必要时只调本地 language server
- 不依赖云端 API

## 6. 对 tokenpulse 的建议

### 6.1 usage 和 quota 必须彻底分层

建议明确约束：

- `usage` 模块：只读本地
- `quota` 模块：允许远端 / 本地 LS

这样可以从产品层避免“看 usage 时偷偷打远端”。

### 6.2 引入本地增量索引，而不是只靠 since + mtime

建议新增 usage cache/index，至少记录：

- provider
- file path
- file id
- mtime
- size
- parsed_bytes
- last_model
- last_totals
- session_id
- day aggregates
- pricing_version

最合适的是 SQLite，本地 JSON 也可。

### 6.3 扫描优先利用日期结构

建议顺序：

1. 如果目录天然带 `YYYY/MM/DD` 分区，按日期范围只扫目标日附近目录
2. 如果文件名带日期，先按文件名裁剪
3. 只有在无日期结构时，才退回全目录 walk
4. 对已扫描文件用 `parsed_bytes` 增量续扫

这个思路基本就是 `CodexBar` 的做法。

### 6.4 历史价格必须冻结

建议不要再用“当前 LiteLLM 价格重算全部历史”。

更合理的做法有两种：

#### 方案 A：内置价格快照

- 把价格表 vendored 到仓库
- 每次发布时手动更新
- 历史统计绑定到某个 `pricing_version`

优点：

- 稳定
- 可复现
- 完全本地

#### 方案 B：本地价格仓快照化

- 允许手动执行一次 `pricing refresh`
- 把结果保存成本地带版本号的 snapshot
- usage 统计写入时记录当时 `pricing_version`
- 以后重跑旧天数据时，继续用旧 snapshot

优点：

- 兼顾更新能力和历史稳定性

如果你的要求是“不要请求远程数据”，那默认应采用方案 A，或者方案 B 但关闭自动 refresh。

### 6.5 优先参考关系

如果要落地到 `tokenpulse`，建议参考顺序：

1. `tokscale`
   - 学扫描入口、Codex/Claude/OpenCode 解析规则
2. `CodexBar`
   - 学按文件增量缓存、日期裁剪、历史稳定 cost
3. `openusage`
   - 学展示口径和“usage 本地、quota 远端”的分层
4. `antigravity-trajectory-extractor`
   - 学 Antigravity 本地 SQLite/protobuf/LS 解码路线

## 7. 直接回答你的问题

可以，而且应该这么做。

更具体一点：

- `session usage` 不需要远端请求
- 完全可以只扫描本地 session 数据
- 统计速度和稳定性主要取决于：
  - 是否做按文件增量缓存
  - 是否利用日期目录/文件名裁剪扫描范围
  - 是否用稳定的增量 token 规则
  - 是否冻结历史价格

所以 `tokenpulse` 下一步最合理的方向不是“再换一个远端接口”，而是：

- 把 usage 做成纯本地扫描
- 把价格做成版本化本地快照
- 把扫描做成按天/按文件的增量缓存


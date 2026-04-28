# OpenUsage Quota 查询分析

## 参考项目

本项目参考了以下三个开源项目的实现：

1. **openusage** - https://github.com/robinebers/openusage
   - TypeScript/Tauri 项目，主要参考其 quota 获取逻辑和无感知认证策略
   - 插件化架构，每个 provider 是独立的 JavaScript 插件

2. **CodexBar** - https://github.com/steipete/CodexBar
   - Swift macOS 菜单栏应用，参考 Codex quota 获取

3. **tokscale** - https://github.com/junhoyeo/tokscale
   - Rust 项目，参考 token 使用量追踪实现

## 架构概览

OpenUsage 采用**插件化架构**，每个 provider 是一个独立的 JavaScript 插件：

```
plugins/
├── claude/
│   ├── plugin.json   # 元数据定义
│   ├── plugin.js     # 探测逻辑
│   └── icon.svg      # 图标
├── codex/
├── gemini/
└── ...
```

## 核心组件

### 1. Plugin Manifest (plugin.json)

```json
{
  "schemaVersion": 1,
  "id": "claude",
  "name": "Claude",
  "version": "0.0.1",
  "entry": "plugin.js",
  "icon": "icon.svg",
  "brandColor": "#DE7356",
  "links": [...],
  "lines": [
    { "type": "progress", "label": "Session", "scope": "overview", "primaryOrder": 1 },
    { "type": "progress", "label": "Weekly", "scope": "overview" },
    { "type": "text", "label": "Today", "scope": "detail" }
  ]
}
```

### 2. Host API (Rust 提供给 JS 的接口)

从 `host_api.rs` 提取的核心 API：

```javascript
// 文件系统
ctx.host.fs.exists(path)
ctx.host.fs.readText(path)
ctx.host.fs.writeText(path, content)
ctx.host.fs.listDir(path)

// 环境变量 (白名单限制)
ctx.host.env.get("CODEX_HOME")
ctx.host.env.get("ZAI_API_KEY")
// 等...

// HTTP 请求
ctx.host.http.request({ url, method, headers, bodyText, timeoutMs })

// Keychain (macOS)
ctx.host.keychain.readGenericPassword(service)
ctx.host.keychain.writeGenericPassword(service, value)

// SQLite 查询
ctx.host.sqlite.query(dbPath, sql)

// ccusage 工具调用
ctx.host.ccusage.query({ since, until, provider, homePath })

// Language Server 发现
ctx.host.ls.discover({ processName, markers, csrfFlag, portFlag })
```

### 3. Helper 函数 (注入到 JS 上下文)

```javascript
// Line builders
ctx.line.text({ label, value, color?, subtitle? })
ctx.line.progress({ label, used, limit, format, resetsAt?, periodDurationMs?, color? })
ctx.line.badge({ label, text, color?, subtitle? })

// Formatters
ctx.fmt.planLabel(value)       // "pro" -> "Pro"
ctx.fmt.resetIn(secondsUntil)  // "2h 30m"
ctx.fmt.dollars(cents)         // 1234 -> 12.34
ctx.fmt.date(unixMs)           // "Mar 15"

// Utilities
ctx.util.tryParseJson(text)
ctx.util.safeJsonParse(text)   // { ok: true, value } | { ok: false }
ctx.util.request(opts)
ctx.util.requestJson(opts)     // { resp, json }
ctx.util.isAuthStatus(status)  // 401 || 403
ctx.util.retryOnceOnAuth({ request, refresh })
ctx.util.parseDateMs(value)
ctx.util.toIso(value)
ctx.util.needsRefreshByExpiry({ nowMs, expiresAtMs, bufferMs })

// Base64
ctx.base64.decode(str)
ctx.base64.encode(str)

// JWT
ctx.jwt.decodePayload(token)   // 解析 JWT payload
```

## 无感知认证策略

### Claude 的做法

1. **优先读取文件**: `~/.claude/.credentials.json`
2. **Keychain 回退**: macOS keychain service `Claude Code-credentials`
3. **自动刷新 Token**: 
   - 检查 `expiresAt` 是否即将过期 (提前 5 分钟)
   - 调用 refresh API 获取新 token
   - 保存更新后的 credentials

### Codex 的做法

从 `codex/plugin.js` 分析：
- 读取 `~/.codex/config.json` 获取 tokens
- 使用 access token 调用 quota API
- 401 时尝试刷新 token

### Gemini 的做法

1. **读取 credentials**: `~/.gemini/oauth_creds.json`
2. **读取 OAuth client**: 从 gemini-cli-core 的 oauth2.js 提取 client_id/client_secret
3. **刷新 Token**: 调用 `https://oauth2.googleapis.com/token`

### Antigravity 的做法

1. **读取 SQLite**: `~/Library/Application Support/Antigravity/User/globalStorage/state.vscdb`
2. **解析 protobuf**: 从 `jetskiStateSync.agentManagerInitState` 提取 tokens
3. **Cloud Code API**: 调用 Google Cloud Code API 获取 quota

## Quota API 端点

| Provider | API URL |
|----------|---------|
| Claude | `https://api.anthropic.com/api/oauth/usage` |
| Codex | `https://chatgpt.com/backend-api/wham/usage` |
| Gemini | `https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota` |
| Antigravity | `https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels` |

## TokenPulse 实现状态 (2026-03-15)

### 已完成的 quota fetchers

| Provider | 文件 | 状态 |
|----------|------|------|
| Claude | `quota/claude.rs` | ✅ 完成 |
| Codex | `quota/codex.rs` | ✅ 完成 |
| Gemini | `quota/gemini.rs` | ✅ 完成 |
| Antigravity | `quota/antigravity.rs` | ✅ 完成 |

### 已完成的 auth 模块

| Provider | 文件 | 状态 |
|----------|------|------|
| Claude | `auth/claude.rs` | ✅ 完成 |
| Codex | `auth/codex.rs` | ✅ 完成 |
| Gemini | `auth/gemini.rs` | ✅ 完成 |
| Antigravity | `auth/antigravity.rs` | ✅ 完成 (含 protobuf 解析) |

### 凭证文件路径

| Provider | 默认路径 |
|----------|---------|
| Claude | `~/.claude/.credentials.json` |
| Codex | `~/.config/codex/auth.json` 或 `~/.codex/auth.json` |
| Gemini | `~/.gemini/oauth_creds.json` |
| Antigravity | `~/Library/Application Support/Antigravity/User/globalStorage/state.vscdb` |

## 参考文件

- `/tmp/openusage/plugins/claude/plugin.js` - Claude quota 探测
- `/tmp/openusage/plugins/codex/plugin.js` - Codex quota 探测
- `/tmp/openusage/plugins/gemini/plugin.js` - Gemini quota 探测
- `/tmp/openusage/plugins/antigravity/plugin.js` - Antigravity quota 探测
- `/tmp/openusage/src-tauri/src/plugin_engine/host_api.rs` - Host API 实现

## UI 设计参考

### 进度条与使用速度计算

openusage 提供了使用速度的计算逻辑 (`pace-status.ts`):

```javascript
// 计算使用速度状态
// ahead: 用量低于预期 (projectedUsage <= limit * 0.8)
// on-track: 正常 (projectedUsage <= limit)
// behind: 用量超出预期 (projectedUsage > limit)

// 计算方式:
const usageRate = used / elapsedMs  // 当前使用速率
const projectedUsage = usageRate * periodDurationMs  // 预测周期结束时用量
```

### 进度条显示字段

```javascript
ctx.line.progress({
    label: "Session",
    used: 50,           // 已使用量 (百分比)
    limit: 100,         // 总上限
    format: { kind: "percent" },
    resetsAt: "2024-03-15T12:00:00Z",  // 重置时间
    periodDurationMs: 5 * 60 * 60 * 1000  // 周期时长 (5小时)
})
```

### 模型聚合策略

Antigravity 有多个模型，但只显示三个 pool:
- Gemini Pro: 取所有 Pro 模型中剩余量最少的
- Gemini Flash: 取所有 Flash 模型中剩余量最少的
- Claude: 所有非 Gemini 模型共享一个 quota

```javascript
function poolLabel(normalizedLabel) {
    var lower = normalizedLabel.toLowerCase()
    if (lower.indexOf("gemini") !== -1 && lower.indexOf("pro") !== -1) return "Gemini Pro"
    if (lower.indexOf("gemini") !== -1 && lower.indexOf("flash") !== -1) return "Gemini Flash"
    return "Claude"  // 所有其他模型
}

// 取剩余量最少的作为代表
if (!deduped[pool] || frac < deduped[pool].remainingFraction) {
    deduped[pool] = { label: pool, remainingFraction: frac, resetTime: rtime }
}
```

### Overview vs Detail 显示

`plugin.json` 中定义了 `scope`:
```json
"lines": [
    { "type": "progress", "label": "Session", "scope": "overview" },
    { "type": "progress", "label": "Weekly", "scope": "overview" },
    { "type": "text", "label": "Today", "scope": "detail" }
]
```

- `overview`: 在总览页面显示最重要的指标
- `detail`: 在详情页面显示更多信息

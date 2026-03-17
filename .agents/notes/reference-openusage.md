# OpenUsage 项目分析

> 项目地址: https://github.com/robinebers/openusage
> 本地路径: /tmp/openusage

## 1. 项目概览

macOS 菜单栏应用 (Tauri 2)，追踪多个 AI 编码工具的订阅使用量。

**技术栈:**
- 前端: React 19 + TypeScript + Tailwind CSS + Vite
- 后端: Rust (Tauri 2)
- 插件运行时: QuickJS (JS 嵌入 Rust)
- 存储: SQLite, macOS Keychain, JSON 文件

## 2. 核心架构 — 插件系统

每个 provider 是一个独立的 JS 插件:

```
plugins/{provider}/
├── plugin.json    # 元数据 (id, name, entry, icon, lines定义)
├── plugin.js      # 入口 (定义 globalThis.__openusage_plugin)
├── icon.svg       # 品牌图标
└── plugin.test.js # 测试
```

**插件生命周期:**
1. **发现** (`manifest.rs`): 读取 plugin.json, 验证入口, 编码图标
2. **执行** (`runtime.rs`): QuickJS 实例化 → 注入 Host API → 执行 probe() 函数
3. **输出**: `{ plan?, lines: MetricLine[] }` — 支持 text/progress/badge 三种行类型

## 3. Host API (Rust → JS)

注入为 `__openusage_ctx` 对象:

| API | 功能 |
|-----|------|
| `host.fs.*` | 文件系统 (exists, readText, writeText, listDir) |
| `host.env.get()` | 环境变量 (白名单限制) |
| `host.http.request()` | HTTP 请求 (自动脱敏日志) |
| `host.keychain.*` | macOS Keychain 读写 |
| `host.sqlite.query/exec()` | SQLite 查询 (支持 WAL) |
| `host.ls.discover()` | 语言服务器进程发现 (ps + lsof) |
| `host.ccusage.query()` | ccusage CLI 调用 |
| `util.*` | JSON解析, HTTP请求, 日期转换, JWT解码 |
| `line.*` | 输出行构建器 (text, progress, badge) |
| `fmt.*` | 格式化 (planLabel, resetIn, dollars, date) |

## 4. Provider 实现

### 4.1 Claude

**认证策略:**
- 主: `~/.claude/.credentials.json`
- 备: macOS Keychain (`Claude Code-credentials`)
- Token 自动刷新: 5 分钟缓冲, `platform.claude.com/v1/oauth/token`
- Client ID: `9d1c250a-e61b-44d9-88ed-5944d1962f5e`

**Quota 获取:**
```
GET https://api.anthropic.com/api/oauth/usage
Header: anthropic-beta: oauth-2025-04-20
```

**输出行:** Session (5h) / Weekly (7d) / Sonnet (7d) / 今日/昨日/30天用量

### 4.2 Codex

**认证策略:**
- 主: `~/.config/codex/auth.json` 或 `~/.codex/auth.json` (或 CODEX_HOME)
- 备: Keychain (`Codex Auth`)
- Refresh 触发: 8 天过期

**Quota 获取:**
```
GET https://chatgpt.com/backend-api/wham/usage
Header: Account ID (多账号支持)
```

**输出行:** Session / Weekly / Code Reviews / Credits / 每日用量

### 4.3 Gemini

**认证策略:**
- OAuth2: `~/.gemini/oauth_creds.json`
- Client credentials: 从 gemini-cli-core 包加载
- Refresh: 5 分钟缓冲, `oauth2.googleapis.com/token`

**Quota 获取:**
```
POST https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist
POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota
```

**输出行:** Gemini Pro / Gemini Flash / 账号 / 计划

### 4.4 Antigravity ⭐ (OpenUsage 的优势实现)

**认证策略 (三层):**
1. SQLite: `~/Library/Application Support/Antigravity/User/globalStorage/state.vscdb`
   - 从 `ItemTable` 读取 `antigravityAuthStatus` (API key)
   - 从 `jetskiStateSync.agentManagerInitState` 读取 proto tokens
2. **自实现 protobuf 解析器** (无外部库): varint 解码器 + proto wire format
3. Token 缓存: `{plugin-data-dir}/auth.json`

**Quota 获取 (三级优先):**
1. **Language Server 探测** (首选):
   - 发现 `language_server_macos` 进程
   - 提取 CSRF token + 端口
   - 调用 `GetUserStatus` → `GetCommandModelConfigs` RPC
2. **Cloud Code API** (备用):
   - `POST https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels`
   - `POST https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels`
3. **Token 刷新**: Google OAuth, 缓存 1 小时

**模型聚合策略:**
```javascript
function poolLabel(label) {
    if (label.includes("gemini") && label.includes("pro")) return "Gemini Pro"
    if (label.includes("gemini") && label.includes("flash")) return "Gemini Flash"
    return "Claude"  // 所有非 Gemini 模型
}
// 取每个 pool 中 remainingFraction 最低的作为代表
```

**输出行:** Gemini Pro / Gemini Flash / Claude (progress, percent, reset)

## 5. UI 架构

```
App → AppShell
  ├── PanelHeader
  ├── AppContent
  │   ├── ProviderCard → MetricLineRenderer
  │   │   ├── Text line (label + value)
  │   │   ├── Progress line (进度条 + 速度指示器)
  │   │   └── Badge line (标签 + 彩色徽章)
  │   ├── SettingsPanel
  │   └── DetailsPanel
  └── SideNav (插件列表)
```

**进度条特色功能:**
- 使用速度指示器 (pace indicator): 绿=领先, 黄=正常, 红=落后
- 进度条上的标记点显示预期使用进度
- 用完时间/赤字计算

## 6. 对 TokenPulse 的参考价值

| 方面 | 参考价值 |
|------|---------|
| **Antigravity 认证** | ⭐⭐⭐ protobuf 解析 + 三级认证策略 |
| **Antigravity Quota** | ⭐⭐⭐ LS 探测 + Cloud API 双通道 |
| **模型聚合** | ⭐⭐ pool 分组 + 取最低余量策略 |
| **插件架构** | ⭐ 参考但 TokenPulse 用 Rust 原生实现 |
| **UI 速度指示器** | ⭐⭐ pace 计算逻辑可移植 |

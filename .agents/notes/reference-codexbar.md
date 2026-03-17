# CodexBar 项目分析

> 项目地址: https://github.com/steipete/CodexBar
> 本地路径: /tmp/CodexBar

## 1. 项目概览

macOS 14+ 菜单栏应用 (Swift/SwiftUI)，监控多个 AI/LLM provider 的 token 使用量和配额。以双条形指示器显示 session (5h) 和 weekly 用量。

**模块结构:**
- **CodexBarCore**: Provider 获取/解析/数据模型/本地探测 (RPC, PTY, Web)
- **CodexBar**: 状态管理 (UsageStore, SettingsStore), UI (StatusItem, 菜单, 图标)
- **CodexBarWidget**: WidgetKit 系统小组件
- **CodexBarCLI**: 命令行工具
- **CodexBarMacros**: Swift 编译器插件 (provider 注册)

**技术栈:** Swift 6 + SwiftUI + AppKit + Sparkle + SweetCookieKit

## 2. Provider 实现

### 2.1 Codex (OpenAI) ⭐ (CodexBar 的核心优势)

**四重策略:**

1. **RPC 策略** (首选):
   - 启动 `codex -s read-only -a untrusted app-server` 子进程
   - JSON-RPC 协议: `account/read` (邮箱/计划), `account/rateLimits/read` (用量)
   - 返回: primary (5h) + secondary (weekly) windows + credits
   ```json
   {
     "primary_window": { "used_percent": 45, "reset_at": 1710532800, "limit_window_seconds": 18000 },
     "secondary_window": { "used_percent": 20, "reset_at": 1710873600, "limit_window_seconds": 604800 },
     "credits": { "has_credits": false, "unlimited": false, "balance": "0.50" }
   }
   ```

2. **PTY 策略** (备用): 解析 `codex ... /status` CLI 输出

3. **OAuth 策略**:
   - `GET https://chatgpt.com/backend-api/wham/usage`
   - Bearer token 认证
   - Token 刷新: `POST https://auth.openai.com/oauth/token`

4. **Web Dashboard** (可选): 浏览器 cookie 提取用量详情

**Token 刷新:**
- 端点: `https://auth.openai.com/oauth/token`
- POST: client_id + grant_type=refresh_token + refresh_token + scope
- 错误检测: `refresh_token_expired`, `refresh_token_reused`, `refresh_token_invalidated`
- 刷新后写回磁盘 `CodexOAuthCredentialsStore.save()`

### 2.2 Claude

**智能策略规划器** (`ClaudeSourcePlanner`):

1. **OAuth 策略** (首选):
   - `GET https://api.anthropic.com/api/oauth/usage`
   - Header: `anthropic-beta: oauth-2025-04-20`
   - 凭证来源: Keychain (Claude CLI) 或 `CLAUDE_CODE_API_KEY` 环境变量
   - **智能刷新协调器** (`ClaudeOAuthDelegatedRefreshCoordinator`):
     - 委托给后台 Claude CLI 进程
     - 冷却机制: 默认 5min, 短周期 20sec
     - UserDefaults 持久化冷却状态 (跨应用同步)

2. **Web 策略** (备用): 浏览器 cookie → claude.ai 订阅面板
3. **CLI 策略** (备用): PTY 运行 `claude --verbose`

**响应格式:**
```json
{
  "usage": {
    "sonnet": { "used_tokens": 50000, "remaining_tokens": 450000 },
    "secondary": { ... },
    "tertiary": { ... }
  },
  "budget": { "used": 25.50, "limit": 100.00, "currency_code": "USD" },
  "reset_at": "2026-04-20T00:00:00Z"
}
```

### 2.3 Antigravity ⚠️ (CodexBar 的弱点)

**仅本地 LS 探测** (实验性):

1. `ps -ax` 查找 `language_server_macos` 进程
2. 提取 `--csrf_token`, `--extension_server_port` 等参数
3. `lsof` 查找监听端口
4. gRPC-JSON 调用:
   - `GetUserStatus` (主)
   - `GetCommandModelConfigs` (备)
5. 解析 model configs: label, modelId, quotaInfo (remainingFraction, resetTime)

**问题/限制:**
- 必须 Antigravity 应用正在运行且 LS 已暴露
- **无 Token 刷新机制** (纯本地, 无认证凭证)
- 标记为实验性, 可能需要重启
- 8 秒超时
- **没有 Cloud API 备用方案** — 如果 LS 没运行就获取不到
- **没有 protobuf 解析** — 无法从 SQLite 提取 token 用于 Cloud API 调用

### 2.4 其他 Provider

| Provider | 策略 |
|----------|------|
| Gemini | Gemini CLI OAuth + quota API |
| Cursor | Web cookie + 存储的 session |
| Copilot | GitHub device flow OAuth |
| z.ai | API token → quota 端点 |
| MiniMax | Cookie → HTML 解析 |
| Kimi | JWT cookie → billing API |
| Factory/Droid | Web cookie + WorkOS tokens |
| JetBrains AI | 本地 XML quota 文件 |

## 3. 数据模型

### 核心类型

```swift
// 速率窗口
RateWindow {
    usedPercent: Double          // 使用百分比
    windowMinutes: Int?          // 窗口时长
    resetsAt: Date?              // 重置时间
    resetDescription: String?    // "in 2h 34m"
}

// Provider 快照
UsageSnapshot {
    primary: RateWindow?         // Session (5h)
    secondary: RateWindow?       // Weekly
    tertiary: RateWindow?        // Opus / Flash 等
    providerCost: ProviderCostSnapshot?
    updatedAt: Date
    identity: ProviderIdentitySnapshot?
}

// 费用追踪
ProviderCostSnapshot {
    used: Double
    limit: Double
    currencyCode: String         // "USD"
    period: String?              // "Monthly"
    resetsAt: Date?
}
```

## 4. UI 架构

**菜单栏图标:**
- NSImage 20×18 px, 两条水平条
- 上条: primary (session) 使用率
- 下条: secondary (weekly) 使用率
- 动态颜色: provider 品牌色
- 过期状态: 变暗
- 加载动画: KnightRider/pulse @ 12 FPS

**菜单内容 (SwiftUI in NSMenuItem):**
- Provider 名 + 邮箱
- Primary/Secondary/Tertiary 百分比 + 重置倒计时
- 使用速度 (pace)
- Credits 余额
- 费用分解
- 账号信息 (计划, 组织)

**刷新策略:** 1m / 2m / 5m (默认) / 15m / 30m / 手动

## 5. 认证策略对比

| Provider | 主存储 | 备选 | 刷新触发 | 刷新端点 |
|----------|--------|------|----------|----------|
| Claude | Keychain + 环境变量 | CLI 委托 | 过期前 5min | platform.claude.com |
| Codex | `~/.codex/auth.json` | Keychain | 8 天 | auth.openai.com |
| Antigravity | LS 进程探测 | 无 | 无 | 无 |
| Gemini | Gemini CLI OAuth | gcloud ADC | 过期前 | googleapis.com |

## 6. 对 TokenPulse 的参考价值

| 方面 | 参考价值 |
|------|---------|
| **Codex 多策略** | ⭐⭐⭐ RPC + PTY + OAuth + Web 四重策略 |
| **Codex Token 刷新** | ⭐⭐⭐ 完整的错误检测和重试逻辑 |
| **Claude OAuth 刷新** | ⭐⭐⭐ 委托协调器 + 冷却机制 |
| **数据模型** | ⭐⭐⭐ RateWindow/UsageSnapshot 清晰的三层窗口模型 |
| **Antigravity** | ⚠️ 仅 LS 探测, 无 Cloud API 备选, 无 protobuf — 需参考 OpenUsage |
| **多 Provider 架构** | ⭐⭐ 策略规划器模式值得借鉴 |
| **UI 设计** | ⭐⭐ 双条图标 + 品牌色 + 动画细节 |

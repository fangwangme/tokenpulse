# Usage + Quota 整合及 Settings 重构需求评审报告

> **Date:** 2026-05-26
> **Scope:** tokenpulse-cli TUI — Usage/Quota 视图整合、Settings 页面、Auto Refresh
> **Target Branch:** `feat-usage-settings-panel`
> **Model:** Kimi K2.6

---

## 1. 项目架构回顾

### 1.1 工作空间结构
```
tokenpulse/
├── tokenpulse-core/          # 核心库（SQLite、Parser、Quota Fetcher、Config）
│   ├── src/config/mod.rs       # Config / DisplayConfig / ThemePreference / QuotaDisplayMode
│   ├── src/quota/              # 配额抓取（claude, codex, copilot, antigravity）
│   └── src/usage/              # 使用量存储与聚合
└── tokenpulse-cli/            # 终端应用
    ├── src/main.rs             # CLI 入口（Commands::Usage / Commands::Quota）
    ├── src/commands/
    │   ├── usage.rs            # usage 命令实现（TUI / text / json / csv）
    │   └── quota.rs            # quota 命令实现（TUI / text）
    └── src/tui/
        ├── theme.rs            # Theme / ThemeMode 定义
        ├── views/
        │   ├── usage.rs        # Usage TUI（4 个 tab：Overview / Models / Daily / Heatmap）
        │   └── quota.rs        # Quota TUI（Overview + provider tabs + Settings）
        └── widgets/            # 图表、热力图等
```

### 1.2 TUI 技术栈
- **ratatui 0.29** + **crossterm 0.28**（支持 mouse capture）
- 交互模型：`event::poll(100ms)` 轮询 + `match key.code` 分发
- 每次循环 `terminal.draw(|f| ...)` 重绘

### 1.3 当前两个独立 TUI 的关键差异

| 维度 | Usage TUI (`views/usage.rs`) | Quota TUI (`views/quota.rs`) |
|------|------------------------------|------------------------------|
| **页面结构** | 4 个 page（enum `UsagePage`） | 动态 tab（Overview + N provider + Settings） |
| **刷新机制** | `r` 手动 reload（无 auto） | `a` 切换 auto-refresh（0/1/2/5/10/15 min） |
| **Theme 切换** | `b` 键（轮询 Auto→Dark→Light） | `b` 键（同上） |
| **Setting 页** | ❌ 无 | ✅ 有（display_mode / show_account / theme / auto_refresh / providers） |
| **Footer 信息** | 显示 data range + last refreshed | 显示 auto-refresh 倒计时 + provider 数量 |
| **Tab 导航** | ✅ 已支持 `←→` / `Tab` / `h/l` / 鼠标点击 | ✅ 已支持 `←→` / `Tab` / `h/l`，❌ 鼠标点击未实现 |

---

## 2. 需求拆解与评审

### 2.1 需求总览（按优先级）

1. **Quota/Balance 嵌入 Usage** — 新增 "Quota" tab，平时看汇总，指定 `--provider` 时才进入详情
2. **Usage 新增 Settings 页面** — 支持 theme 切换、auto-refresh 配置
3. **Auto Refresh 分开配置** — Quota 和 Usage 拥有各自独立的 auto-refresh 间隔选项
4. **交互增强** — 所有入口均支持左右键切换 tab + 鼠标点击 tab
5. **Setting 合并/过滤逻辑** — `tokenpulse usage` 只展示 Usage 的 setting，`tokenpulse quota` 只展示 Quota 的 setting；未来统一入口（不带子命令）时合并展示两边的 setting

### 2.2 逐项评审

#### 需求 1：Quota 嵌入 Usage 作为新 Tab

**可行性：✅ 高**

当前 `UsagePage` 是固定 4 个 tab 的 enum：
```rust
enum UsagePage {
    Overview,
    Models,
    Daily,
    Heatmap,
}
```

需要：
- 新增 `Quota` 或 `Balance` 变体
- 在该 tab 内渲染 quota 概览（类似 Quota TUI 的 `render_overview`，但不渲染 per-provider 详情 card）
- 当 CLI 传入了 `--provider` 时，允许进入该 provider 的详情 view

**技术要点：**
- `commands/usage.rs` 中 `run()` 函数已接收 `provider: Option<String>`，该值需要透传进 TUI state
- 需要在 `UsageState` 中新增字段标记是否处于 "quota detail mode"（当指定了 provider 时）
- Quota 数据获取：需要把 `commands/quota.rs` 中的 `build_quota_fetchers` + `refresh_quota_results` 逻辑复用或抽取到 core 层
- **注意**：Quota TUI 当前是阻塞式 `tokio::task::block_in_place(|| runtime.block_on(...))` 模式，Usage TUI 的 `reload` closure 也是阻塞式的，可直接复用相同 pattern

**建议实现路径：**
1. 在 `tokenpulse-core` 中新增 `quota::fetch_all_sync()` 或类似的 helper，供 Usage TUI 调用
2. `UsageState` 新增 `quota_results: Vec<Result<QuotaSnapshot>>` 和 `selected_quota_provider: Option<String>`
3. Quota tab 的 render：
   - 无 `--provider`：类似 Quota TUI 的 overview grid（compact 模式），但用 Usage TUI 的 theme 风格
   - 有 `--provider`：渲染单个 provider card（复用 Quota TUI 的 `render_snapshot_card`）

#### 需求 2：Usage 新增 Settings 页面

**可行性：✅ 高**

新增 `UsagePage::Settings` 变体。Settings 页需要支持：
- **Theme 切换**：当前 `b` 键全局已支持，需把这个功能也放入 Settings UI（类似 Quota TUI 的 settings 行）
- **Auto Refresh 间隔**：见需求 3
- **（可选）Provider 启用/禁用**：复用 Quota TUI 的 provider toggle 逻辑

**技术要点：**
- `UsageState` 需新增 `settings_row: usize` 用于列表导航
- 复用 `ConfigManager` 读写 config（路径：`~/.local/share/tokenpulse/config.toml`）
- Settings 页的 render 风格应和 Quota TUI 的 `render_settings` 保持一致，但内容不同（Usage 不需要 quota_display_mode）

#### 需求 3：Auto Refresh 分开配置（Quota vs Usage）

**可行性：✅ 高**

当前 Quota TUI 已支持 auto-refresh，间隔定义：
```rust
const AUTO_REFRESH_INTERVALS: &[u32] = &[0, 60, 120, 300, 600, 900]; // 秒
```
对应：off / 1m / 2m / 5m / 10m / 15m

用户要求 **Quota 和 Usage 拥有各自独立的可选间隔**，且默认均为 5 分钟：

**Quota Auto Refresh 选项**（频率可更高）：
| 选项 | 秒数 | 说明 |
|------|------|------|
| off  | 0    | 关闭 |
| 1m   | 60   | 1 分钟 |
| 2m   | 120  | 2 分钟 |
| 5m   | 300  | 5 分钟（默认） |
| 10m  | 600  | 10 分钟 |
| 15m  | 900  | 15 分钟 |

**Usage Auto Refresh 选项**（频率不宜过高，reload 有磁盘 I/O）：
| 选项 | 秒数 | 说明 |
|------|------|------|
| off  | 0    | 关闭 |
| 5m   | 300  | 5 分钟（默认） |
| 10m  | 600  | 10 分钟 |
| 15m  | 900  | 15 分钟 |
| 30m  | 1800 | 30 分钟 |

**技术要点：**
- `DisplayConfig` 新增 `usage_auto_refresh_secs` 字段，与现有的 `quota_auto_refresh_secs` 并存
- 在 Usage TUI 的事件循环中（当前是 `event::poll(100ms)`），每次 tick 检查 elapsed：
  ```rust
  if auto_secs > 0 && last_refresh.elapsed().as_secs() >= auto_secs as u64 {
      // trigger reload()
  }
  ```
- **注意**：Usage 的 reload 是 `reload()` closure（重新解析所有 provider session 文件），比 Quota 的 API 请求慢得多。建议 reload 期间禁用 auto-refresh timer，防止重叠触发。

#### 需求 4：交互增强 — 左右键切换 + 鼠标点击 Tab

**可行性：✅ 高**

当前 **Usage TUI** 已完全支持：
- `←/→` / `h/l`：切换 tab ✅
- `Tab` / `Shift+Tab`：切换 tab ✅
- 鼠标点击 tab：在 `render_tabs` 区域通过 `MouseEventKind::Down(MouseButton::Left)` + 坐标命中检测实现 ✅（已在 Heatmap/Daily 等 tab 上有实现）

当前 **Quota TUI** 已支持：
- `←/→` / `h/l`：切换 tab ✅
- `Tab` / `Shift+Tab`：切换 tab ✅
- **鼠标点击 tab：❌ 未实现**

Quota TUI 的 tab 区域是动态生成的（`quota_tab_titles()`），需要为每个 tab 计算点击区域。

**实现建议：**
1. 在 Quota TUI 的 `run()` 事件循环中，增加 `Event::Mouse(mouse)` 分支处理
2. 复用 Usage TUI 已有的点击检测逻辑（`rect_contains(tab_area, ...)` + `Tabs` 的宽度分配计算）
3. 额外注意：Quota TUI 的 tab 是动态的（有 N 个 provider + Overview + Settings），点击检测需基于 `tab_titles.len()` 动态计算每个 tab 的宽度
4. **未来统一 Dashboard**：同样需支持鼠标点击，建议在 `render_tabs()` helper 中统一实现坐标映射函数

#### 需求 5：Setting 合并/过滤逻辑（按子命令隔离）

**可行性：✅ 高**

用户原话：
> "如果不加参数的话，就是把setting的项目合并一下，但是加了参数的话，就是只展示自己的setting"

这里的"参数"指 `usage` / `quota` 这两个子命令。正确理解如下：

**场景 A：`tokenpulse usage`**
- Usage TUI 的 Setting 页面只展示 **Usage 相关的 setting**
- 例如：theme、usage auto-refresh interval、provider filter（用于 usage 数据来源）
- **不包含**：quota_display_mode、show_account、quota auto-refresh 等 Quota 特有配置

**场景 B：`tokenpulse quota`**
- Quota TUI 的 Setting 页面只展示 **Quota 相关的 setting**
- 例如：theme、quota_display_mode、show_account、quota auto-refresh interval、provider 启用/禁用
- **不包含**：usage auto-refresh 等 Usage 特有配置

**场景 C：未来统一入口（如 `tokenpulse` 不带子命令）**
- Setting 页面**合并展示**两端的配置：
  - 通用：theme
  - Usage 特有：usage auto-refresh
  - Quota 特有：display mode、show account、quota auto-refresh、provider toggles

**技术要点：**
- `DisplayConfig` 需要新增 `usage_auto_refresh_secs` 字段，与现有的 `quota_auto_refresh_secs` 并存
- Usage TUI 的 Settings render 函数只读取/展示：
  - `theme`
  - `usage_auto_refresh_secs`
  - `enabled_sources`（用于过滤 usage 数据）
- Quota TUI 的 Settings render 函数保持现状，只读取/展示 Quota 相关配置
- 两边共用同一个 `ConfigManager`，读写同一个 `config.toml`，只是各自关注不同的字段子集

---

## 3. 代码变更范围预估

### 3.1 修改文件清单

| 文件 | 变更类型 | 变更内容 |
|------|---------|---------|
| `tokenpulse-core/src/config/mod.rs` | 新增 | `usage_auto_refresh_secs` 字段 + 默认值常量 |
| `tokenpulse-cli/src/tui/views/usage.rs` | 大幅修改 | 新增 `Quota`/`Settings` page，新增 auto-refresh 逻辑 |
| `tokenpulse-cli/src/tui/views/quota.rs` | 修改 | 增加鼠标点击 tab 支持 |
| `tokenpulse-cli/src/commands/usage.rs` | 修改 | 透传 `provider` 给 TUI；预加载 quota 数据 |
| `tokenpulse-cli/src/commands/quota.rs` | 可选抽取 | 将 `refresh_quota_results` 等抽取为公共函数 |
| `tokenpulse-cli/src/tui/widgets/` | 可选新增 | 抽取公共 `render_tabs_with_click()` 或类似组件 |

### 3.2 复杂度评估

- **视图层复杂度：中等偏高**
  - `usage.rs` 当前约 4200 行，是 Quota TUI（~800 行）的 5 倍
  - 新增两个 tab 的 render 函数和事件处理会增加 ~800-1200 行
  - 建议将新增的 Quota/Settings render 逻辑拆分为独立模块：`views/usage/quota_tab.rs`、`views/usage/settings_tab.rs`

- **状态管理复杂度：中等**
  - `UsageState` 膨胀问题：当前已有 ~20 个字段
  - 建议新增 `QuotaSubState` 和 `SettingsSubState` 嵌套 struct

- **数据流复杂度：低**
  - Quota 数据获取已有成熟代码，主要是复用问题

- **交互复杂度：低**
  - 鼠标点击 tab 逻辑在 Usage TUI 已有成熟实现，只需复用到 Quota TUI

---

## 4. 优化建议

### 4.1 架构层面

1. **提取公共 TUI 组件**
   - `render_settings()` 在 Quota 和 Usage 中高度相似，建议抽象为 `widgets/settings_panel.rs`
   - `theme_status_label()` 和 theme 切换逻辑 100% 可复用
   - **新增**：抽取 `render_clickable_tabs()` 公共函数，统一处理 Tabs Widget 的鼠标命中检测

2. **Config 字段命名统一**
   - 当前：`quota_auto_refresh_secs`、`quota_display_mode`
   - 建议新增：`usage_auto_refresh_secs`
   - 未来若有 central dashboard 可统一为 `display.auto_refresh_secs`（全局）+ `display.auto_refresh_scope: Vec<String>`

3. **Quota 数据预加载策略**
   - 当前 Quota TUI 是在 `run()` 时传入已获取的 `results`
   - 若 Usage TUI 嵌入 Quota tab，应在进入该 tab 时 **懒加载**（on-demand fetch），避免启动时阻塞
   - 可在 `UsageState` 中增加 `quota_loaded: bool` + `quota_results: Option<Vec<...>>`

### 4.2 交互层面

1. **快捷键冲突检查**
   - `s` 键：Usage 当前是 "source filter"，Quota Settings 中是 "show account"。若 Settings 成为独立 tab，需避免 tab 内快捷键与全局冲突。
   - `a` 键：Quota 中是 auto-refresh toggle，若 Usage 也引入 `a` 键，需保持一致。
   - 建议：所有 setting 相关操作仅在 Settings tab 内生效，其他 tab 保持原快捷键。

2. **Auto-refresh UX**
   - Usage 的 reload 可能耗时数秒（解析 session 文件），auto-refresh 时应显示 "Refreshing..." 状态，避免用户误以为卡住。
   - 建议 reload 期间禁用 auto-refresh timer，防止重叠触发。

3. **鼠标点击 Tab 的引入**
   - Quota TUI 引入鼠标点击后，需确保 `MouseEventKind::Down` 事件不会与现有的 mouse capture 冲突
   - 建议在所有 TUI entry 的 `enable_raw_mode()` 后确认已启用 `EnableMouseCapture`（Usage 已启用，Quota 可能缺失）

### 4.3 性能层面

1. **Usage auto-refresh 的成本**
   - 每次 reload 会重新扫描所有 provider 的 session 文件（磁盘 I/O）
   - 默认 5 分钟对少量 provider 是合理的，但对多 provider 用户（7 个全部启用）可能造成明显卡顿
   - 建议：在 footer 中显示下一次 auto-refresh 的倒计时，让用户感知到刷新成本

2. **内存占用**
   - 新增 Quota snapshot 存储到 UsageState 会增加内存，但 QuotaSnapshot 体积小（每个 provider 几十字节），可忽略

---

## 5. 潜在风险

| # | 风险 | 影响 | 缓解措施 |
|---|------|------|---------|
| 1 | `usage.rs` 文件过大（4.2k → 5k+ 行），维护困难 | 高 | 拆分子模块：`views/usage/{mod,overview,models,daily,heatmap,quota,settings}.rs` |
| 2 | `UsageState` 字段过多 | 中 | 使用 struct 嵌套：`quota_state: QuotaState`, `settings_state: SettingsState` |
| 3 | Quota 数据懒加载失败后的错误状态处理 | 低 | 在 UI 上显示 "Failed to load quota data (press r to retry)" |
| 4 | `--provider` 过滤与 setting 显示的交互歧义 | 中 | 明确文档：filter 仅影响展示，不影响可编辑性（用户仍可 toggle 其他 provider） |
| 5 | Auto-refresh 间隔与 Quota TUI 不一致导致用户困惑 | 低 | Footer 明确标注当前 auto-refresh 配置及剩余倒计时 |
| 6 | Quota TUI 启用 mouse capture 可能与用户终端不兼容 | 低 | 测试常见终端（iTerm2、Alacritty、Windows Terminal），确保 EnableMouseCapture 不会破坏现有交互 |

---

## 6. 实施优先级建议

```
Phase 1（基础框架）：
  ├── 新增 UsagePage::Quota tab（仅展示 overview，无详情）
  ├── 复用 Quota TUI 的 overview render 逻辑
  └── 支持懒加载 quota 数据

Phase 2（Settings + Auto Refresh）：
  ├── 新增 UsagePage::Settings tab
  ├── 支持 theme 切换
  ├── 新增分开的 auto-refresh 配置（Usage: 5/10/15/30m，Quota: 1/2/5/10/15m）
  └── 默认均设为 5m

Phase 3（交互增强）：
  ├── Quota TUI 增加鼠标点击 tab 支持
  ├── Usage TUI 确保 Setting/Quota tab 的鼠标点击正常
  └── 统一 Dashboard 入口支持左右键 + 鼠标点击

Phase 4（可选优化）：
  ├── 在 Quota tab 支持进入 provider 详情（当指定 --provider 时）
  ├── 提取公共 settings render 组件
  └── 文件拆分重构
```

---

## 7. 结论

该需求总体**可行度高**，核心工作量集中在 `views/usage.rs` 的扩展和重构。Auto Refresh 的分开配置和鼠标点击 tab 是比较独立的模块，可以并行开发。

最大的挑战不是功能本身，而是 `usage.rs` 当前已经很大（~4200 行），新增 tab 和 state 之前应先做适度的模块化拆分，否则会严重影响后续维护。

建议按 Phase 分步实施，每步保持可编译、可运行、可测试，避免一次性大包大揽导致 regression。

# Usage + Quota TUI 整合及 Settings 页面重构可行性评审报告

> **Date:** 2026-05-26
> **Scope:** tokenpulse-cli TUI — Usage/Quota 视图深度整合、Settings 页面统一、Auto Refresh 支持、鼠标点击支持、热力图窗口精简
> **Target Branch:** `feat-usage-settings-panel`
> **Author Model:** Gemini 3.5 Flash (High)

---

## 1. 核心需求背景与变更建议

用户希望对 TokenPulse CLI 现有的 `usage` 与 `quota` 双入口设计进行重构与统一。当前，Usage TUI 和 Quota TUI 是两个独立启动的交互式窗口。本重构方案将把两者合并，以 Usage 视图为主体。

### 1.1 核心需求列表

1. **去掉切换参数，默认结合 Usage 与 Quota**
   - 移除多余的切换参数，统一由主命令启动进入包含 Quota 与 Settings 页面的统一 TUI 界面。
   - 在 TUI 中增加 `Quota` Tab 专门用于展示余额/配额。
2. **极简 Quota Tab 展示策略：仅保留 Overview**
   - **完全去除 Provider 详情（Detail）页面**：Quota Tab 将**仅保留 Overview 视图**，不再提供单独进入每个 Provider 内部（如 Claude, Gemini）详细限流和重置时间的子 Tab 页面。
   - **分组展示**：如果模型或配额窗口较多，直接在 Overview 视图内使用分组（Group）的形式呈现。
3. **Usage 内部集成统一 Settings 页面**
   - 在 TUI 中增加一个新的 "Settings" Tab 页面。
   - **主题切换逻辑**：将现有的全局快捷键 `b` 自动切换背景主题（Auto -> Dark -> Light）的功能收拢进 Settings 页面中，在设置项内进行设定。其他页面禁用 `b` 快捷键以防误触。
4. **Auto Refresh（自动刷新）精细配置**
   - **默认值**：Quota 和 Usage 自动刷新默认值均设为 **5 分钟**。
   - **Quota 刷新间隔**：支持 1 分钟、2 分钟、5 分钟（以及 10 分钟、15 分钟、关闭(Off)）。
   - **Usage 刷新间隔**：最少为 5 分钟，最大为 30 分钟，支持 5 分钟、10 分钟、15 分钟、30 分钟以及关闭(Off)。
5. **支持鼠标点击切换 Tab**
   - 顶部的 Tab 栏除了现有的键盘左右键切换外，必须**完整支持鼠标左键点击切换**。在加入 `Quota` 和 `Settings` Tab 后，所有 6 个 Tab 均需流畅支持鼠标点击交互。
6. **热力图（Activity）视图进一步精简（新增）**
   - **删除 `w window` 切换功能**：完全移除原有的热力图窗口切换功能及 `w` 键映射。
   - **固定展示窗口**：热力图展示窗口固定为过去的一年（365天 / 52周），不再提供 26 周、52 周等多种展示范围的切换入口，使底层数据计算和 UI 渲染逻辑更加简洁。
7. **Settings 项目过滤逻辑**
   - **无特定参数时**：Settings 页面展示合并后的设置。
   - **有特定参数 `--provider` 时**：仅展示该 Provider 自身的设置，过滤掉其他 Provider 开关。

---

## 2. 系统架构与技术实现方案

### 2.1 TUI 页面状态与路由定义

目前 `views/usage.rs` 的 `UsagePage` 枚举为：
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsagePage {
    Overview,
    Models,
    Daily,
    Heatmap,
}
```

**改造后方案**：
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsagePage {
    Overview,
    Models,
    Daily,
    Heatmap,
    Quota,
    Settings,
}
```

---

### 2.2 热力图精简实现方案

在目前的 `views/usage.rs` 中，热力图状态包含 `heatmap_window`：
```rust
struct UsageState {
    // ...
    heatmap_window: HeatmapWindow,
    // ...
}
```
并且在事件处理中响应 `w` 键以循环切换：
```rust
KeyCode::Char('w') => {
    state.heatmap_window = state.heatmap_window.next();
}
```

**改造后方案**：
1. **删除 `HeatmapWindow` 枚举**。
2. 在 `UsageState` 中删除 `heatmap_window` 字段。
3. 原本所有使用 `state.heatmap_window` 的地方（如数据截取、日历渲染），**统一硬编码/固定使用 365 天** 范围的数据。
4. 在 `render_footer` 中删除 `w window` 的按键提示和响应逻辑。

这可以直接减少热力图渲染组件约 100 行复杂的范围计算代码，对于提升 `views/usage/heatmap.rs` 的可读性有很大帮助。

---

### 2.3 鼠标点击交互支持 (Mouse Event Handling)

- 确保 `UsagePage::all()` 包含全部 6 个页面（Overview, Models, Daily, Heatmap, Quota, Settings）。
- 这样，原有的鼠标事件循环在计算 `page.title()` 时，会自动将新增的 `Quota` 和 `Settings` Tab 纳入点击判定范围，无需编写额外的定位代码，实现天然的鼠标点击支持。

---

### 2.4 拆分重构 `views/usage.rs` 文件

为了避免代码过度臃肿，我们将执行拆分重构：
- 新建文件夹 `tui/views/usage/`
- 将原有的巨型文件分拆为：
  - `mod.rs`（页面控制路由、TUI 状态管理和键盘鼠标事件循环处理）
  - `overview.rs`（渲染 Overview 页面）
  - `models.rs`（渲染 Models 页面）
  - `daily.rs`（渲染 Daily 页面）
  - `heatmap.rs`（渲染 Activity 热力图页面）
  - `quota.rs`（新：仅渲染 Quota 网格概览卡片，使用 Grouping 展示多配额项）
  - `settings.rs`（渲染设置项列表并接收修改操作）

---

## 3. 技术栈评估：关于 TUI 渲染引擎

项目当前基于 **Ratatui 0.29 + Crossterm 0.28** 构建。针对是否可引入 **OpenTUI** 的评估结论如下：

- **结论**：**不建议引入 OpenTUI**。
- **原因**：
  1. **分发成本**：OpenTUI 生态的核心 bindings 针对 TypeScript/React 家族。引入它需要在 Rust CLI 中嵌入 JS 运行时，或者通过 FFI 编写不成熟的 C ABI 绑定，会破坏 TokenPulse 纯 Rust 独立分发、轻量化秒开的极佳体验。
  2. **生态成熟度**：Ratatui 是 Rust 领域无可争议的 TUI 标准，具备完整的原生的布局系统、日历热力图、表格滚动和自适应机制。

---

## 4. 结论与下一步计划

重构路线已明确锁定：
- 去除 Provider Detail 页，仅保留 Quota Overview Grid 渲染；
- 支持 6 个 Tab 的键盘及鼠标左键点击切换；
- 整合并过滤 Settings 项，配置 Quota (1m-15m) 与 Usage (5m-30m) 双自动刷新默认值；
- **移除热力图视图中的 `w window` 切换功能，将其固定为过去一年范围展示**；
- 实施代码模块化拆分。

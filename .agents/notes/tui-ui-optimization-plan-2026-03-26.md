# TUI UI 优化计划

**日期**: 2026-03-26
**状态**: 待实施

---

## 一、问题总结

通过审查 `views/usage.rs` 和 `views/quota.rs`，发现以下核心问题：

1. **空间浪费** - 固定高度过大、全局 margin 黑边
2. **边框噪音** - 四向边框过多、彩色边框喧宾夺主
3. **数据对齐** - 手动空格对齐不稳定
4. **热度图** - 方块太小、颜色是相对值
5. **Quota 进度条** - 单行显示不整齐、无 pace 配色

---

## 二、优化计划

### P0：核心体验修复

#### 1. Quota 进度条两行布局 + pace 配色

- **文件**: `tokenpulse-cli/src/tui/views/quota.rs`
- **修改内容**:
  - 进度条改为两行显示
  - 第一行：Label（固定10字符宽度） + 百分比 + 重置时间
  - 第二行：纯进度条，填满宽度
  - 进度条颜色根据 pace 状态变化：
    - `ahead`（使用较慢）= 绿色 `#10B981`
    - `on-track`（正常）= 黄色 `#F59E0B`
    - `behind`（使用过快）= 红色 `#EF4444`

#### 2. 热度图双字符方块 + 间隙

- **文件**: `tokenpulse-cli/src/tui/widgets/heatmap.rs`
- **修改内容**:
  - 每个方块占 2 字符宽度（如 `██`）
  - 方块之间留 1 字符间隙
  - 每列实际占 3 字符（2 内容 + 1 间隙）
  - 最小宽度检查从 12 改为 30

#### 3. 热度图阈值配置

- **文件**: `tokenpulse-core/src/config/mod.rs`
- **修改内容**:
  - 添加 `HeatmapThresholds` 配置结构
  - 支持配置文件动态调整档位
  - 默认档位：
    - tier1 = 2000万 (0-2000万，最浅)
    - tier2 = 5000万 (2000万-5000万)
    - tier3 = 1亿 (5000万-1亿)
    - tier4 = 1.5亿 (1亿-1.5亿)
    - tier5 = 2亿 (1.5亿-2亿)
    - > 2亿 (最深)

#### 4. 去掉全局 margin

- **文件**: `usage.rs`, `quota.rs`
- **修改内容**:
  - `Layout::default().margin(1)` 改为 `margin(0)`
  - 让 TUI 全屏铺满，去除黑边

#### 5. 指标卡高度压缩

- **文件**: `tokenpulse-cli/src/tui/views/usage.rs`
- **修改内容**:
  - `Constraint::Length(8)` 改为 `Length(4)`
  - 减少卡片内部空白

#### 6. 边框颜色统一

- **文件**: `tokenpulse-cli/src/tui/theme.rs`
- **修改内容**:
  - 大部分边框使用 `theme.border`（暗灰色）
  - 彩色只用于标题、数据、图表

---

### P1：数据结构 + 功能完善

#### 7. RateWindow 添加 model_id

- **文件**: `tokenpulse-core/src/provider.rs`
- **修改内容**:
  - `RateWindow` 结构添加 `model_id: Option<String>` 字段

#### 8. Claude model 数据统一显示

- **文件**: `tokenpulse-core/src/quota/claude.rs`
- **修改内容**:
  - `seven_day_sonnet`, `seven_day_opus` 等按 model 的数据统一显示
  - 在 Detail 页面展示所有 model 级别的配额

#### 9. Gemini bucket 数据统一显示

- **文件**: `tokenpulse-core/src/quota/gemini.rs`
- **修改内容**:
  - 所有 bucket 数据按 model 统一显示
  - 在 Detail 页面展示各 model 的配额

#### 10. 热度图绝对阈值配色

- **文件**: `tokenpulse-cli/src/tui/widgets/heatmap.rs`
- **修改内容**:
  - 颜色计算从相对值改为绝对阈值
  - 读取配置文件的档位设置

#### 11. Quota 最小高度保证

- **文件**: `tokenpulse-cli/src/tui/views/quota.rs`
- **修改内容**:
  - 多 Provider 时使用 `Constraint::Min(6)`
  - 防止卡片被压缩导致渲染崩溃

---

### P2：代码质量优化

#### 12. 用 block.inner(area) 替代手动计算

- **文件**: 全局
- **修改内容**:
  - 将 `Rect::new(area.x + 1, ...)` 替换为 `block.inner(area)`
  - 更安全、更简洁

#### 13. Header 简化边框

- **文件**: `tokenpulse-cli/src/tui/views/usage.rs`
- **修改内容**:
  - Header 区域只保留 `Borders::BOTTOM`
  - 或完全去掉边框，用背景色区分

#### 14. 大卡片边框简化

- **文件**: `tokenpulse-cli/src/tui/views/usage.rs`
- **修改内容**:
  - 大卡片只保留 `Borders::TOP`
  - 减少视觉噪音

---

## 三、配置文件示例

```toml
# ~/.config/tokenpulse/config.toml

[display]
quota_display_mode = "remaining"
show_empty_providers = false

[display.heatmap_thresholds]
tier1 = 20000000  # 0-2000万
tier2 = 50000000  # 2000万-5000万
tier3 = 100000000 # 5000万-1亿
tier4 = 150000000 # 1亿-1.5亿
tier5 = 200000000 # 1.5亿-2亿
```

---

## 四、预期效果

### Quota 卡片（两行布局 + pace 配色）

```
SESSION 45% | 3h 20m
████████████░░░░░░░░░░░░░░░░░░  (黄色 - 正常)

WEEKLY 75% | 2d 14h
██████████████████░░░░░░░░░░░░  (红色 - 过快)
+15% pace | eta 1d 5h

SONNET 30% | 2d 14h
████████░░░░░░░░░░░░░░░░░░░░░░  (绿色 - 较慢)
5% under pace
```

### 热度图（双字符方块 + 间隙）

```
Activity Grid - Year / Total Tokens

    Jan  Feb  Mar  Apr  ...
S   ··   ██   ··   ##
S   ··   ####  ##  ##
M   ··   ####  ##  ##
T   ██   ####  ██  ##
W   ··   ####  ##  ##
T   ··   ####  ##  ##
F   ··   ##    ··  ##
S   ··   ··    ··  ··

Legend: 0-2KW | 2K-5KW | 5K-1亿 | 1-1.5亿 | 1.5-2亿 | >2亿
        ░░     ▒▒     ▓▓      ████     ██████    ████████
```

---

## 五、实施顺序

1. **第一阶段**: P0 全部完成（核心体验修复）
2. **第二阶段**: P1 全部完成（数据结构完善）
3. **第三阶段**: P2 全部完成（代码质量优化）

---

## 六、Review 意见采纳情况

| Review 建议 | 是否采纳 | 理由 |
|------------|---------|------|
| Length(8) -> Length(4) | ✅ 采纳 | 明显的空间浪费 |
| 去掉全局 margin(1) | ✅ 采纳 | 黑边影响视觉效果 |
| 边框颜色用暗灰 | ✅ 采纳 | 彩色边框喧宾夺主 |
| Quota Constraint::Min(6) | ✅ 采纳 | 防止渲染崩溃 |
| 用 block.inner(area) | ✅ 采纳 | 更安全、更简洁 |
| Header/Tabs 移除框线 | ⚠️ 部分采纳 | Header 简化，Tabs 保留 |
| 卡片只用 Borders::TOP | ⚠️ 部分采纳 | 大卡片简化，小卡片保留 |
| 引入翻页机制 | ❌ 暂不采纳 | 增加复杂度，用 Min(6) 解决 |
| 用 Table widget 对齐 | ❌ 暂不采纳 | 两行布局方案会解决对齐问题 |

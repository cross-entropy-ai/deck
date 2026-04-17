# 响应式布局阈值

## 优先级：🟢 低（可选）
## 复杂度：中
## 价值：低

## 问题

当前布局选择完全由用户手动控制（`l` 键切换 horizontal/vertical）。在极端终端尺寸下会出现问题：

### 终端太窄（< 60 cols）

Horizontal 布局下，sidebar 占 28 列，分隔线 1 列，main pane 只剩 31 列。tmux 内容被严重压缩，几乎无法使用：

```
┌─ sidebar(28) ─┬─ main(31) ──────┐
│ ▌ 1 my-proje  │ $ cargo b       │
│    ~/code/my   │    Compil       │  ← 内容截断严重
│    main        │    ...          │
└────────────────┴─────────────────┘
```

### 终端太矮（< 15 rows）

Session card 占 6 行，header 2 行，footer 2 行。可视区域只能显示不到 1 个完整 card：

```
╭─────────────────────────╮
│  Projects  5            │
│ ▌ 1 my-project          │  ← 只能看到 card 的前 2 行
│    ~/code/my-project    │
│ j/k nav  h/? help  q   │
╰─────────────────────────╯
```

### 终端很宽（> 200 cols）

Sidebar 固定 28 列，main pane 170+ 列，sidebar 显得很窄，浪费了空间。

## 目标

根据终端尺寸自动调整布局，在保持手动控制的同时提供更好的默认行为。

## 设计

### 自动布局切换

```
终端宽度 < 50：  强制 vertical 模式（sidebar 变 tab bar）
终端宽度 50-79：horizontal 模式 + sidebar 自动缩窄到 SIDEBAR_MIN(16)
终端宽度 80+：  horizontal 模式 + sidebar 用用户设置的宽度
终端宽度 160+：  horizontal 模式 + sidebar 可以更宽（显示更多信息）
```

### 自动 compact 切换

```
终端高度 < 20：  强制 compact 模式
终端高度 20-30：compact 或 expanded（用户选择）
终端高度 30+：  expanded 模式
```

### 行为规则

1. **自动调整不覆盖用户的手动选择**
   - 用户按 `l` 切换到 vertical → 保持 vertical，即使终端变宽
   - 只有在用户没有明确选择时，才使用自动布局

2. **自动调整有明确的阈值**
   - 当终端尺寸跨过阈值时才切换，避免频繁跳动
   - 使用 hysteresis（滞后）：宽度 < 50 时自动切换到 vertical，但需要 > 55 才切回 horizontal

3. **强制模式**
   - 某些极端尺寸下强制使用特定布局，即使用户之前选了别的
   - 例如：宽度 < 40 时，horizontal 模式完全无法使用，必须强制 vertical

## 实现方案

### 布局决策函数

```rust
// app.rs

/// 根据终端尺寸推荐布局
fn recommended_layout(width: u16, height: u16, user_preference: LayoutMode) -> LayoutMode {
    // 强制阈值：太窄时必须用 vertical
    if width < 50 {
        return LayoutMode::Vertical;
    }
    
    // 用户偏好优先
    user_preference
}

/// 根据终端尺寸推荐视图模式
fn recommended_view_mode(height: u16, user_preference: ViewMode) -> ViewMode {
    // 强制阈值：太矮时必须用 compact
    if height < 20 {
        return ViewMode::Compact;
    }
    
    // 用户偏好优先
    user_preference
}

/// 根据终端尺寸计算推荐的 sidebar 宽度
fn recommended_sidebar_width(term_width: u16, user_width: u16) -> u16 {
    let min_main = 40u16;  // main pane 最小可用宽度
    let max_sidebar = term_width.saturating_sub(min_main + 1);  // 1 for separator
    
    user_width.clamp(SIDEBAR_MIN, max_sidebar.min(SIDEBAR_MAX))
}
```

### 在 resize 时应用

```rust
fn handle_resize(&mut self, width: u16, height: u16) {
    self.term_width = width;
    self.term_height = height;
    
    // 检查是否需要自动调整布局
    let old_layout = self.layout_mode;
    self.layout_mode = recommended_layout(width, height, self.user_layout_preference);
    
    // 检查是否需要自动调整视图模式
    let old_view = self.view_mode;
    self.view_mode = recommended_view_mode(height, self.user_view_preference);
    
    // 调整 sidebar 宽度（不超过可用空间）
    self.sidebar_width = recommended_sidebar_width(width, self.sidebar_width);
    
    self.resize_pty();
    
    // 可选：如果布局变了，显示 toast 通知
    if old_layout != self.layout_mode {
        self.toast = Some(Toast::success(format!(
            "Layout: {}",
            match self.layout_mode {
                LayoutMode::Horizontal => "horizontal",
                LayoutMode::Vertical => "vertical (auto)",
            }
        )));
    }
}
```

### 区分用户偏好和当前状态

```rust
pub struct App {
    // 用户的偏好（来自配置或手动切换）
    user_layout_preference: LayoutMode,
    user_view_preference: ViewMode,
    
    // 实际使用的值（可能被自动调整覆盖）
    layout_mode: LayoutMode,
    view_mode: ViewMode,
    
    // ...
}
```

当用户按 `l` 切换布局时，同时更新 `user_layout_preference` 和 `layout_mode`：

```rust
KeyCode::Char('l') => {
    self.user_layout_preference = match self.user_layout_preference {
        LayoutMode::Horizontal => LayoutMode::Vertical,
        LayoutMode::Vertical => LayoutMode::Horizontal,
    };
    self.layout_mode = recommended_layout(
        self.term_width,
        self.term_height,
        self.user_layout_preference,
    );
    self.resize_pty();
    self.save_config();
}
```

### Sidebar 宽度自适应

Sidebar 宽度在 resize 时自动 clamp，确保 main pane 至少有 40 列可用：

```rust
fn recommended_sidebar_width(term_width: u16, user_width: u16) -> u16 {
    if term_width < 80 {
        // 窄终端，sidebar 尽量缩小
        let max_sidebar = term_width.saturating_sub(41);  // 留 40 给 main + 1 separator
        user_width.min(max_sidebar).max(SIDEBAR_MIN)
    } else if term_width > 160 {
        // 宽终端，sidebar 可以更宽
        user_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX)
    } else {
        // 正常终端
        user_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX)
    }
}
```

### 可选：在 footer 显示终端尺寸

resize 后短暂显示终端尺寸（类似 tmux 的行为）：

```
╭────────────────── 120×40 ──────────────────╮
│                                            │  ← resize 后显示 2 秒
│                                            │
```

可以用 toast 实现。

## 测试计划

- [ ] 终端宽度 < 50 时自动切换到 vertical
- [ ] 终端宽度从 < 50 resize 到 > 55 时恢复用户偏好
- [ ] 终端高度 < 20 时自动切换到 compact
- [ ] 用户手动切换布局后，resize 不会覆盖用户选择（除非到达强制阈值）
- [ ] sidebar 宽度在 resize 后不超过可用空间
- [ ] main pane 始终至少有 40 列（horizontal 模式）
- [ ] 极小终端（如 30×10）下不 crash
- [ ] 极大终端（如 300×80）下布局正常
- [ ] resize 时不闪烁或跳动
- [ ] 配置保存的是用户偏好，不是自动调整后的值

## 注意事项

- 自动调整应该是**保守的**：只在极端情况下覆盖用户选择
- 阈值的选择很重要——太激进会让用户感觉失控，太保守则没有效果
- Hysteresis 防止在阈值边缘反复跳动
- 这个功能可以分阶段实现：先做强制阈值（最简单），再做自适应宽度

## 相关文件

- `src/app.rs`：handle_resize、布局决策逻辑
- `src/config.rs`：区分用户偏好和实际状态

# Code Review: deck

**Date**: 2026-04-16
**Scope**: Full codebase (~5840 LOC, 14 source files)
**Focus**: 重复逻辑、冗余逻辑、逻辑清晰度

---

## 项目大纲

| 模块 | LOC | 职责 |
|------|-----|------|
| `main.rs` | 103 | 入口，终端初始化 |
| `app.rs` | ~900 | 主循环，PTY 管理，渲染调度 |
| `action.rs` | ~700 | Action 枚举，状态机（apply_action） |
| `input.rs` | 330 | 键盘/鼠标输入映射（从 action.rs 拆出） |
| `state.rs` | ~560 | AppState，数据类型，过滤/排序 |
| `ui.rs` | ~1420 | 所有渲染函数 |
| `config.rs` | 388 | 配置持久化，glob/regex 匹配 |
| `bridge.rs` | 77 | vt100→ratatui 适配 |
| `pty.rs` | 290 | PTY 生命周期管理 |
| `tmux.rs` | 193 | tmux CLI 封装 |
| `git.rs` | 75 | Git 状态解析 |
| `theme.rs` | 125 | 主题色板 |
| `nesting_guard.rs` | 151 | 递归嵌套防护 |
| `instance_guard.rs` | 137 | 单实例锁 |

---

## 修复状态总览

### 已修复

| # | 问题 | 修复方式 |
|---|------|----------|
| 1 | 导航逻辑重复（FocusNext/Prev/ScrollUp/Down） | 提取 `navigate()` helper，四分支合一 |
| 2 | ExcludeEditorConfirm 添加 pattern 代码重复 | 统一为"先验证 regex，再走单一 add 路径" |
| 3 | recursive apply_action 的 SideEffect 合并不安全 | 添加 `SideEffect::merge()`，所有递归调用改用 `fx.merge(...)` |
| 13 | `draw_sidebar` 有 14 个参数 | 引入 `SidebarView` 结构体，签名改为 `(frame, area, &SidebarView, theme)` |

额外改进（非本次 review 提出）：
- `key_to_action` / `mouse_to_action` 从 `action.rs` 拆到独立的 `input.rs`
- `MenuConfirm` 从字符串匹配改为 `MenuAction` 枚举匹配

### 未修复

| # | 问题 | 当前状态 |
|---|------|----------|
| 4 | mouse_to_action 中 PTY offset 计算重复 | 仍在 `input.rs:306-309` 和 `input.rs:317-320` 重复 |
| 5 | draw_sessions expanded/compact 样式计算重复 | 仍在 `ui.rs:209-226` 和 `ui.rs:371-387` 重复 |
| 7 | `is_emphasized = is_focused` 永远相等的别名 | 仍在 `ui.rs:207` 和 `ui.rs:369` |
| 8 | `build_tab_status` 无意义 wrapper | 仍在 `ui.rs:682` |
| 9 | `session_at_col` 构建完整 `Vec<SessionView>` 只用了 name | 仍在 `state.rs:433-450` |
| 10 | Config 的 `layout`/`view_mode` 用 String 而非枚举 | 仍在 `config.rs:16-19` |
| 11 | `~/claude` 默认目录 hardcode 两次 | 仍在 `app.rs:149` 和 `app.rs:447` |
| 12 | `_size: PtySize` 未使用参数 | 仍在 `pty.rs:67` |

### 知悉但不修

| # | 问题 | 决定原因 |
|---|------|----------|
| 6 | render() 每帧 clone（`views_owned` 等） | 当前 session 数量不多，clone 代价在 16ms 帧预算里不是瓶颈；改动需要重构 `App::render` 以解构 self 借用，ROI 不够。未来若支持大量 session 或 `SessionRow` 字段增长，可重新评估。 |

---

## 未修复问题详情

### 4. mouse_to_action 中 PTY offset 重复（input.rs:304-325）

左键点击分支和通用转发分支各自计算了一次完全相同的 offset：

```rust
// input.rs:306-309 (left click)
let b = if state.show_borders { 1u16 } else { 0 };
let (col_off, row_off) = match state.layout_mode {
    LayoutMode::Horizontal => (state.sidebar_width + 1 + b, b),
    LayoutMode::Vertical => (b, state.effective_sidebar_height() + b),
};

// input.rs:317-320 (general forward) — 完全相同
let b = if state.show_borders { 1u16 } else { 0 };
let (col_off, row_off) = match state.layout_mode {
    LayoutMode::Horizontal => (state.sidebar_width + 1 + b, b),
    LayoutMode::Vertical => (b, state.effective_sidebar_height() + b),
};
```

另外，`if self.show_borders { 1u16 } else { 0 }` 这个模式在代码中出现了 **5 次**（state.rs:373, 408, 429 和 input.rs:306, 317）。

**建议**：
1. 在 `AppState` 上加一个方法：`fn pty_origin(&self) -> (u16, u16)` 返回 PTY 区域在屏幕上的起始坐标
2. mouse_to_action 中将 offset 计算提到 `!in_sidebar && !on_separator` 分支的开头，只算一次

---

### 5. draw_sessions expanded/compact 样式计算重复（ui.rs:209-226 vs 371-387）

两个函数逐行对比：

| Expanded (line 209-226) | Compact (line 371-387) |
|---|---|
| `let accent_color = if is_focused { theme.green } else { theme.bg };` | 完全相同 |
| `let accent = if is_focused { "▌" } else { " " };` | 完全相同 |
| `let name_style = ...` | 完全相同 |
| `let index_style = ...` | 完全相同 |
| `let bg = if is_focused { theme.surface } else { theme.bg };` | 完全相同 |

**建议**：提取一个 `SessionStyle` 结构体：

```rust
struct SessionStyle {
    accent_color: Color,
    accent: &'static str,
    name_style: Style,
    index_style: Style,
    bg: Color,
}

fn session_style(theme: &Theme, is_focused: bool) -> SessionStyle { ... }
```

两个函数各自调用 `session_style()`，消除重复的 ~20 行样式计算。

---

### 6. render() 每帧分配问题（app.rs:467-517）— 知悉但不修

当前 render 函数在每个 16ms 帧中做的分配：`views_owned: Vec<SessionRow>`（clone 所有 filtered session）、`settings_view`、`rename_input.clone()`、`context_menu.clone()`、`warning_state.clone()`、`confirm_name.clone()`、`spinner_frame.to_string()`。其中 `views_owned` 最重。

**根本原因**：`terminal.draw(|frame| { ... })` 的闭包需要 `&mut Frame`，同时又要读 `self.state`、`self.parser`、`self.plugin_instances`。因为 `self` 整体被 `&mut` 借用，闭包内无法同时借用 `self` 的多个字段，只能先把 state 数据 clone 出来。

**决定不修**：当前 session 数量不多，clone 代价在 16ms 帧预算里不是瓶颈；修复需要重构 `App::render` 解构 self 借用，ROI 不够。未来若支持大量 session 或 `SessionRow` 字段增长，可重新评估。

---

### 其他小问题

| # | 文件:行 | 问题 | 建议 |
|---|---------|------|------|
| 7 | `ui.rs:207,369` | `is_emphasized = is_focused` — 永远相等的别名 | 直接用 `is_focused`，或用 #5 的 `SessionStyle` 一并消除 |
| 8 | `ui.rs:682-684` | `build_tab_status` 只是 `format_git_status(session, false)` 的 wrapper | 内联调用 |
| 9 | `state.rs:433-450` | `session_at_col` 每次 mouse 事件构建完整 `Vec<SessionView>`，只用了 name | `tab_col_ranges()` 改为接受 `&[&str]` 或 `&[(usize, &str)]` |
| 10 | `config.rs:16,19` | `layout: String` / `view_mode: String` 可用枚举 | 用 `LayoutMode`/`ViewMode` 枚举，避免 `app.rs` 中的字符串匹配 |
| 11 | `app.rs:149,447` | `~/claude` 默认目录 hardcode 了两次 | 提取为常量 |
| 12 | `pty.rs:67` | `_size: PtySize` 未使用参数 | 移除 |

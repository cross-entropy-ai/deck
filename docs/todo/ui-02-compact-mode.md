# 紧凑模式（Compact Cards）

## 优先级：🔴 高
## 复杂度：中
## 价值：高

## 问题

当前每个 session card 固定占 6 行（CARD_HEIGHT = 6）：

```
行1: ▌  1  my-project          ← name
行2:       ~/code/my-project   ← directory
行3:       main                ← branch
行4:       2 ahead, 1 behind  ← git status
行5:       active ⠋            ← activity
行6:                           ← 空行间隔
```

在标准 24 行终端中，去掉 header（2 行）和 footer（2 行），可视区域约 20 行，只能显示 **3 个完整 card**。session 数量一多就需要频繁滚动。

## 目标

提供紧凑模式，每个 session 只占 2 行，一屏可以显示 8-10 个 session。用户可以在 compact / expanded 之间切换。

## 设计

### Expanded 模式（当前，6 行/card）

```
 ▌  1  my-project
       ~/code/my-project
       main
       2 ahead, 1 behind
       active ⠋

 ▌  2  my-app
       ~/code/my-app
       develop
       clean
       idle
```

### Compact 模式（2 行/card）

```
 ▌ 1 my-project  main ↑2 ↓1 ~3  active ⠋
      ~/code/my-project
 ▌ 2 my-app  develop  clean  idle
      ~/code/my-app
 ▌ 3 another-proj  feat/login ↑1 +2  idle 5m
      ~/work/another-proj
```

第一行：名字 + branch + git status 符号 + activity
第二行：目录路径

git status 符号化：
- `↑N` = ahead N
- `↓N` = behind N
- `+N` = staged N
- `~N` = modified N
- `?N` = untracked N
- `✓` = clean

### 超紧凑模式（1 行/card，可选）

```
 ▌ 1 my-project  main ↑2↓1  ~/code/my-project  active
 ▌ 2 my-app  develop ✓  ~/code/my-app  idle
```

所有信息压缩到一行，宽度不够时从右边截断。

## 交互

- 按 `c` 在 compact / expanded 之间切换
- 设置持久化到 config.json（`"view_mode": "compact"` / `"expanded"`）
- 在 vertical tab 模式下不受影响（tab 模式本身就是紧凑的）

## 实现方案

### 新增状态和枚举

```rust
// app.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Expanded,  // 当前的 6 行 card
    Compact,   // 2 行 card
}

pub struct App {
    // ... 现有字段 ...
    view_mode: ViewMode,
}
```

### 修改 CARD_HEIGHT

```rust
// ui.rs
pub const CARD_HEIGHT_EXPANDED: usize = 6;
pub const CARD_HEIGHT_COMPACT: usize = 2;

// card_height 变成参数而不是常量
pub fn card_height(view_mode: ViewMode) -> usize {
    match view_mode {
        ViewMode::Expanded => CARD_HEIGHT_EXPANDED,
        ViewMode::Compact => CARD_HEIGHT_COMPACT,
    }
}
```

### 修改 draw_sessions

```rust
fn draw_sessions(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    tick: usize,
    theme: &Theme,
    view_mode: ViewMode,  // 新增参数
) {
    match view_mode {
        ViewMode::Expanded => draw_sessions_expanded(frame, area, sessions, focused, tick, theme),
        ViewMode::Compact => draw_sessions_compact(frame, area, sessions, focused, tick, theme),
    }
}

fn draw_sessions_compact(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    tick: usize,
    theme: &Theme,
) {
    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let spinner_frame = &SPINNER[tick % SPINNER.len()];

    for (i, session) in sessions.iter().enumerate() {
        let is_focused = i == focused;
        let is_current = session.is_current;
        let bg = if is_focused { theme.surface } else { theme.bg };
        
        // === 第 1 行：accent + index + name + branch + status + activity ===
        let accent_color = if is_current && is_focused {
            theme.green
        } else if is_focused {
            theme.accent
        } else {
            theme.bg
        };
        let accent = if is_current || is_focused { "▌" } else { " " };
        
        let mut spans = vec![
            Span::styled(accent, Style::default().fg(accent_color).bg(bg)),
            Span::styled(format!("{}", i + 1), Style::default().fg(if is_focused { theme.secondary } else { theme.dim }).bg(bg)),
            Span::styled(" ", Style::default().bg(bg)),
        ];
        
        // Name
        let name_style = if is_current && is_focused {
            Style::default().fg(theme.green).add_modifier(Modifier::BOLD)
        } else if is_focused || is_current {
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.secondary)
        };
        spans.push(Span::styled(session.name, name_style.bg(bg)));
        
        // Branch
        if !session.branch.is_empty() {
            let branch_color = if is_focused { theme.pink } else { theme.muted };
            spans.push(Span::styled("  ", Style::default().bg(bg)));
            spans.push(Span::styled(session.branch, Style::default().fg(branch_color).bg(bg)));
        }
        
        // Git status (符号化)
        let status = format_git_status_symbols(session);
        if !status.is_empty() {
            let status_color = if status == "✓" { theme.green } else if is_focused { theme.yellow } else { theme.dim };
            spans.push(Span::styled(" ", Style::default().bg(bg)));
            spans.push(Span::styled(status, Style::default().fg(status_color).bg(bg)));
        }
        
        // Activity
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        if session.idle_seconds < 3 {
            spans.push(Span::styled("active ", Style::default().fg(theme.green).bg(bg)));
            spans.push(Span::styled(spinner_frame, Style::default().fg(theme.green).bg(bg)));
        } else {
            let idle_text = format_idle_time(session.idle_seconds);
            spans.push(Span::styled(idle_text, Style::default().fg(theme.dim).bg(bg)));
        }
        
        lines.push(pad_line(spans, bg, width));
        
        // === 第 2 行：目录 ===
        let dir_display = truncate(&shorten_dir(session.dir), width.saturating_sub(4));
        let dir_color = if is_focused { theme.teal } else { theme.muted };
        lines.push(pad_line(vec![
            Span::styled("   ", Style::default().bg(bg)),
            Span::styled(dir_display, Style::default().fg(dir_color).bg(bg)),
        ], bg, width));
    }

    // ... scroll 和 render 逻辑 ...
}
```

### 新增 git status 符号化函数

```rust
fn format_git_status_symbols(session: &SessionView) -> String {
    let mut parts: Vec<String> = Vec::new();
    if session.ahead > 0 { parts.push(format!("↑{}", session.ahead)); }
    if session.behind > 0 { parts.push(format!("↓{}", session.behind)); }
    if session.staged > 0 { parts.push(format!("+{}", session.staged)); }
    if session.modified > 0 { parts.push(format!("~{}", session.modified)); }
    if session.untracked > 0 { parts.push(format!("?{}", session.untracked)); }
    if parts.is_empty() && !session.branch.is_empty() {
        return "✓".to_string();
    }
    parts.join(" ")
}
```

### 修改 scroll_offset

```rust
fn scroll_offset(focused: usize, visible_height: u16, card_height: usize) -> usize {
    let focused_bottom = (focused + 1) * card_height;
    let visible = visible_height as usize;
    if focused_bottom > visible {
        focused_bottom - visible
    } else {
        0
    }
}
```

### 修改 session_at_row

`session_at_row` 中的 `CARD_HEIGHT` 也需要根据 `view_mode` 动态获取。

### 配置持久化

```rust
// config.rs
pub struct Config {
    // ... 现有字段 ...
    pub view_mode: String,  // "compact" / "expanded"
}
```

### 快捷键

在 `handle_sidebar_key` 中添加：

```rust
KeyCode::Char('c') => {
    self.view_mode = match self.view_mode {
        ViewMode::Expanded => ViewMode::Compact,
        ViewMode::Compact => ViewMode::Expanded,
    };
    self.save_config();
}
```

在 help 面板中添加说明。

## 测试计划

- [ ] 按 `c` 在 compact / expanded 之间切换
- [ ] compact 模式下每个 card 占 2 行
- [ ] compact 模式下 git status 用符号显示
- [ ] compact 模式下滚动正确（scroll_offset 使用正确的 card_height）
- [ ] compact 模式下鼠标点击定位正确（session_at_row 使用正确的 card_height）
- [ ] compact 模式下右键菜单正确
- [ ] 内容超出宽度时正确截断
- [ ] 切换持久化到 config.json
- [ ] 重启后恢复上次的 view_mode
- [ ] vertical tab 模式不受 view_mode 影响

## 依赖

- 与 [ui-05-git-status-symbols](ui-05-git-status-symbols.md) 共享 `format_git_status_symbols` 函数
- 与 [ui-04-idle-time-format](ui-04-idle-time-format.md) 共享 `format_idle_time` 函数

## 相关文件

- `src/app.rs`：状态和快捷键
- `src/ui.rs`：渲染逻辑（主要修改）
- `src/config.rs`：持久化

# 通知 Toast

## 优先级：🟡 中
## 复杂度：低
## 价值：中

## 问题

执行操作后缺少反馈。以下场景用户不确定操作是否成功：

- Kill session → session 从列表消失，但没有确认消息
- Create session → 新 session 出现，但如果列表需要滚动，用户可能没注意到
- Switch session → 主面板切换了，但没有文字确认
- Rename session → 名字变了，但如果不仔细看不一定注意到
- 操作失败 → 用户完全不知道发生了什么

## 目标

操作完成后，在界面上短暂显示一条反馈消息，1.5-2 秒后自动消失。

```
╭─────────────────────────────────────────────────────────╮
│                                                         │
│                    正常的 tmux 内容                       │
│                                                         │
│                                                         │
│                              ╭──────────────────────╮   │
│                              │ Session "foo" killed  │   │
│                              ╰──────────────────────╯   │
╰─────────────────────────────────────────────────────────╯
         toast 出现在右下角，1.5秒后自动消失
```

## 设计

### 位置

Toast 显示在 **main pane 的右下角**，不遮挡 sidebar。

```
┌─ sidebar ─┬─── main pane ──────────────────────────────┐
│            │                                            │
│  sessions  │         tmux terminal output               │
│            │                                            │
│            │                                            │
│            │                    ╭────────────────────╮  │
│            │                    │ Switched to "bar"  │  │
│            │                    ╰────────────────────╯  │
│            │                                            │
└────────────┴────────────────────────────────────────────┘
```

### 样式

- 背景色：`theme.surface`
- 文字颜色：`theme.text`
- 边框：rounded，`theme.dim`
- 自动消失时间：1.5 秒
- 不可交互（不阻塞键盘/鼠标）

### 消息类型

```
成功消息（绿色 accent）：
╭──────────────────────────╮
│ ✓ Session "foo" killed   │
╰──────────────────────────╯

╭──────────────────────────╮
│ ✓ Created "session-3"    │
╰──────────────────────────╯

╭──────────────────────────╮
│ ✓ Renamed to "my-app"   │
╰──────────────────────────╯

错误消息（黄色 accent）：
╭──────────────────────────╮
│ ✗ Failed to kill session │
╰──────────────────────────╯
```

## 实现方案

### 新增数据结构

```rust
// app.rs

#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub is_error: bool,
    pub created_at: Instant,
}

impl Toast {
    const DURATION: Duration = Duration::from_millis(1500);

    fn success(message: String) -> Self {
        Toast {
            message,
            is_error: false,
            created_at: Instant::now(),
        }
    }

    fn error(message: String) -> Self {
        Toast {
            message,
            is_error: true,
            created_at: Instant::now(),
        }
    }

    fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= Self::DURATION
    }
}

pub struct App {
    // ... 现有字段 ...
    toast: Option<Toast>,
}
```

### 触发 toast

在各个操作完成后设置 toast：

```rust
// Kill session
fn do_kill_focused_session(&mut self) {
    // ... 现有逻辑 ...
    tmux::kill_session(&name);
    self.toast = Some(Toast::success(format!("Killed \"{}\"", name)));
    // ...
}

// Create session
fn create_new_session(&mut self) {
    // ...
    if tmux::new_session(&name, &dir).is_some() {
        self.toast = Some(Toast::success(format!("Created \"{}\"", name)));
        // ...
    } else {
        self.toast = Some(Toast::error("Failed to create session".to_string()));
    }
}

// Switch session
fn switch_project(&mut self) {
    // ...
    let session = &self.sessions[session_idx];
    let name = session.name.clone();
    // ... 执行切换 ...
    self.toast = Some(Toast::success(format!("Switched to \"{}\"", name)));
}

// Rename session（如果已实现）
// tmux::rename_session 成功后：
self.toast = Some(Toast::success(format!("Renamed to \"{}\"", new_name)));
```

### 自动过期

在事件循环中检查 toast 是否过期：

```rust
// app.rs — run()

// 在渲染之前检查
if let Some(ref toast) = self.toast {
    if toast.is_expired() {
        self.toast = None;
    }
}
```

### UI 渲染

```rust
// ui.rs

pub fn draw_toast(frame: &mut Frame, main_area: Rect, toast: &Toast, theme: &Theme) {
    let text = &toast.message;
    let width = (text.len() + 4) as u16;  // 2 border + 1 padding each side
    let height = 3u16;  // 1 border + 1 text + 1 border

    // 右下角定位
    let x = main_area.right().saturating_sub(width + 1);
    let y = main_area.bottom().saturating_sub(height + 1);
    let toast_area = Rect::new(x, y, width, height);

    // 清除底层内容
    frame.render_widget(Clear, toast_area);

    // 边框颜色
    let border_color = if toast.is_error { theme.yellow } else { theme.green };
    let text_color = theme.text;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme.surface));
    let inner = block.inner(toast_area);
    frame.render_widget(block, toast_area);

    // 消息文字
    let prefix = if toast.is_error { "✗ " } else { "✓ " };
    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(border_color)),
        Span::styled(text, Style::default().fg(text_color)),
    ]);
    frame.render_widget(
        Paragraph::new(vec![line]).style(Style::default().bg(theme.surface)),
        inner,
    );
}
```

### 在 render() 中调用

```rust
// app.rs — render()

terminal.draw(|frame| {
    // ... 现有渲染逻辑 ...

    // Toast（最后渲染，浮在最上层）
    if let Some(ref toast) = self.toast {
        ui::draw_toast(frame, main_area, toast, theme);
    }
})?;
```

## 测试计划

- [ ] kill session 后显示 toast `✓ Killed "foo"`
- [ ] create session 后显示 toast `✓ Created "session-3"`
- [ ] switch session 后显示 toast `✓ Switched to "bar"`
- [ ] 操作失败时显示 toast `✗ Failed to ...`
- [ ] toast 在 1.5 秒后自动消失
- [ ] 连续操作时新 toast 替换旧 toast
- [ ] toast 不阻塞键盘/鼠标操作
- [ ] toast 位置在 main pane 右下角，不超出边界
- [ ] toast 宽度根据消息长度自适应
- [ ] 消息过长时截断（不超过 main pane 宽度的一半）
- [ ] 在 vertical layout 模式下 toast 位置正确
- [ ] 有 border 和无 border 模式下 toast 渲染正确

## 注意事项

- Toast 使用 `Instant`（单调时钟），不受系统时间调整影响
- Toast 渲染在最后（最上层），使用 `Clear` widget 确保底层内容被清除
- Toast 不需要持久化，重启后消失
- 不要在高频操作（如导航 j/k）上显示 toast，只在有意义的操作上显示

## 相关文件

- `src/app.rs`：Toast 数据结构、触发逻辑、过期检查
- `src/ui.rs`：draw_toast 渲染函数

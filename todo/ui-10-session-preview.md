# Session Preview（画中画）

## 优先级：🟢 低（远期）
## 复杂度：高
## 价值：中

## 问题

在 sidebar 中浏览 session 列表时，用户只能看到 session 的元信息（名字、目录、branch、git 状态）。要确认某个 session 里正在跑什么，必须切换过去才能看到。

这在"我有 8 个 session，想找那个正在跑 build 的"场景下体验很差。

## 目标

在 horizontal 布局下，hover 或 focus 到某个**非当前** session 时，在 main pane 的角落叠加一个小窗口，显示该 session 的终端输出预览。

```
┌─ sidebar ─┬─── main pane (当前 session) ──────────────────┐
│            │                                                │
│ ▌ 1 foo   │  $ cargo build                                 │
│   ~/foo   │     Compiling ...                               │
│   main    │                                                 │
│            │                                                │
│ ▌ 2 bar ← │     ╭── bar preview ──────────────╮            │
│   ~/bar   │     │ $ npm run dev                │            │
│   develop │     │ Server running on :3000      │            │
│            │     │ GET / 200 12ms              │            │
│ ▌ 3 baz   │     ╰────────────────────────────╯            │
│   ~/baz   │                                                │
│            │                                                │
└────────────┴────────────────────────────────────────────────┘
       ↑ 焦点在 bar 上        ↑ main pane 角落显示 bar 的终端预览
```

## 设计

### 预览窗口

- **位置**：main pane 的右下角（或左下角，可配置）
- **大小**：宽度 = main pane 宽度的 40%，高度 = main pane 高度的 30%
- **边框**：rounded border，accent 颜色
- **标题**：在 border 上显示 session 名字
- **内容**：该 session 的终端最后 N 行输出

### 触发条件

- Sidebar focus 模式下
- 焦点不在当前 session 上（当前 session 已经在 main pane 全屏显示了）
- 预览的 session 有活动（idle < 某个阈值，或者任何时候都显示）

### 更新策略

- 预览内容需要实时更新（至少 1 秒刷新一次）
- 如果预览 session 有新输出，预览窗口应该跟随滚动

## 技术挑战

### 挑战 1：为每个 session 维护独立的终端状态

当前架构中，只有一个 PTY（`tmux attach`），连接的是当前 session。要预览其他 session 的终端输出，需要额外的数据源。

**方案 A：`tmux capture-pane`（推荐）**

使用 `tmux capture-pane` 命令抓取其他 session 的屏幕内容：

```rust
// tmux.rs

/// 抓取指定 session 的 pane 内容（最后 N 行）
pub fn capture_pane(session_name: &str, lines: u16) -> Option<String> {
    // capture-pane 的 -p 输出到 stdout，-t 指定目标
    // -S 指定起始行（负数表示从底部往上）
    tmux(&[
        "capture-pane",
        "-p",
        "-t", session_name,
        "-S", &format!("-{}", lines),
    ])
}
```

优点：
- 不需要额外的 PTY
- 不需要维护独立的 vt100 parser
- 实现简单

缺点：
- `capture-pane` 返回纯文本，不包含颜色信息（除非用 `-e` flag）
- 有 escape sequence 版本：`tmux capture-pane -p -e -t <session>` 可以保留颜色
- 需要额外的 ANSI → ratatui 颜色转换

**方案 B：多 PTY**

为每个需要预览的 session 打开一个只读 PTY。过于复杂，不推荐。

**方案 C：`tmux pipe-pane`**

将其他 session 的输出 pipe 到一个文件或 unix socket。也比较复杂。

### 挑战 2：颜色保留

`tmux capture-pane -e` 输出带 ANSI escape 的文本。需要解析这些 escape sequence 并转换为 ratatui Style。

可以复用 `vt100` 库来解析：

```rust
fn parse_captured_pane(text: &str, width: u16, height: u16) -> vt100::Screen {
    let mut parser = vt100::Parser::new(height, width, 0);
    parser.process(text.as_bytes());
    parser.screen().clone()
}
```

然后用已有的 `bridge::render_screen()` 来渲染。

### 挑战 3：性能

`tmux capture-pane` 是一个外部进程调用。如果每 16ms（POLL_MS）调用一次就太频繁了。

- 只在 sidebar focus 模式 + 焦点在非当前 session 时才抓取
- 抓取频率：每 500ms-1s 一次
- 在后台线程做抓取，避免阻塞 UI

## 实现方案

### 新增状态

```rust
// app.rs

pub struct App {
    // ... 现有字段 ...
    
    /// 预览缓存：session 名 → 预览内容
    preview_cache: Option<PreviewCache>,
}

struct PreviewCache {
    session_name: String,
    screen: vt100::Screen,
    last_updated: Instant,
}

const PREVIEW_REFRESH: Duration = Duration::from_millis(500);
const PREVIEW_WIDTH: u16 = 60;
const PREVIEW_HEIGHT: u16 = 15;
```

### 抓取逻辑

```rust
fn update_preview(&mut self) {
    // 只在 sidebar focus + 非当前 session 时更新
    if self.focus_mode != FocusMode::Sidebar {
        self.preview_cache = None;
        return;
    }

    let Some(&session_idx) = self.filtered.get(self.focused) else {
        self.preview_cache = None;
        return;
    };
    let session = &self.sessions[session_idx];
    if session.is_current {
        self.preview_cache = None;
        return;
    }

    // 检查是否需要刷新
    if let Some(ref cache) = self.preview_cache {
        if cache.session_name == session.name && cache.last_updated.elapsed() < PREVIEW_REFRESH {
            return;  // 缓存还新鲜
        }
    }

    // 抓取
    if let Some(text) = tmux::capture_pane_with_escape(&session.name, PREVIEW_HEIGHT) {
        let mut parser = vt100::Parser::new(PREVIEW_HEIGHT, PREVIEW_WIDTH, 0);
        parser.process(text.as_bytes());
        self.preview_cache = Some(PreviewCache {
            session_name: session.name.clone(),
            screen: parser.screen().clone(),
            last_updated: Instant::now(),
        });
    }
}
```

### UI 渲染

```rust
// ui.rs

pub fn draw_preview(
    frame: &mut Frame,
    main_area: Rect,
    screen: &vt100::Screen,
    session_name: &str,
    theme: &Theme,
) {
    // 预览窗口大小
    let pw = (main_area.width * 2 / 5).min(60);
    let ph = (main_area.height * 3 / 10).min(15);

    // 右下角定位
    let px = main_area.right().saturating_sub(pw + 1);
    let py = main_area.bottom().saturating_sub(ph + 1);
    let preview_area = Rect::new(px, py, pw, ph);

    // 清除底层
    frame.render_widget(Clear, preview_area);

    // 边框，标题为 session 名
    let title = format!(" {} ", session_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(Color::Reset));
    let inner = block.inner(preview_area);
    frame.render_widget(block, preview_area);

    // 渲染终端内容
    bridge::render_screen(screen, inner, frame.buffer_mut());
}
```

### 在 render() 中调用

```rust
// app.rs — render()

terminal.draw(|frame| {
    // ... 现有渲染 ...

    // 预览（在 toast 之前，但在主内容之后）
    if let Some(ref cache) = self.preview_cache {
        ui::draw_preview(frame, main_area, &cache.screen, &cache.session_name, theme);
    }

    // Toast（最后）
    // ...
})?;
```

## 测试计划

- [ ] sidebar focus + 焦点在非当前 session → 显示预览
- [ ] sidebar focus + 焦点在当前 session → 不显示预览
- [ ] main focus → 不显示预览
- [ ] 切换焦点时预览内容跟随变化
- [ ] 预览内容每 500ms 刷新
- [ ] 预览窗口位置在 main pane 右下角
- [ ] 预览窗口不超出 main pane 边界
- [ ] 预览窗口有 border 和 session 名标题
- [ ] 预览内容包含颜色（如果使用 `capture-pane -e`）
- [ ] tmux capture-pane 失败时不 crash，只是不显示预览
- [ ] 性能测试：预览不应该导致 UI 卡顿

## 风险和替代方案

### 风险

1. **性能**：频繁调用 `tmux capture-pane` 可能影响 UI 响应性
2. **颜色解析**：`capture-pane -e` 的 ANSI 输出可能有边缘情况
3. **宽字符**：预览窗口宽度与原 session 宽度不一致，可能导致换行错位

### 替代方案：静态预览

不做实时预览，而是在按特定键（如 `p`）时全屏显示该 session 的快照：

```
按 p → 全屏显示选中 session 的终端快照
按 Esc/q → 返回 sidebar
```

这样实现简单，不需要处理实时更新和浮层定位问题。

## 依赖

- 需要 `tmux capture-pane` 命令（tmux 1.8+，几乎所有现代版本都有）
- 颜色版本需要 `capture-pane -e`（tmux 2.0+）
- 复用已有的 `bridge::render_screen()` 函数

## 相关文件

- `src/tmux.rs`：新增 `capture_pane` 函数
- `src/app.rs`：PreviewCache 状态管理
- `src/ui.rs`：draw_preview 渲染
- `src/bridge.rs`：复用 render_screen

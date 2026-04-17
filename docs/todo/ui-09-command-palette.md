# Command Palette

## 优先级：🟢 低（可选）
## 复杂度：高
## 价值：中

## 问题

当前所有操作都通过单字母快捷键触发（`j/k/f/t/b/l/x/q`）。这对熟练用户效率很高，但：

1. **可发现性差** — 新用户不知道有哪些操作可用，需要按 `h/?` 看 help 面板
2. **help 面板是静态的** — 无法搜索，看完需要按键关闭
3. **操作不可搜索** — 用户记得"那个切换边框的操作"但忘了快捷键是 `b`
4. **扩展性受限** — 随着功能增加，可用的单字母越来越少

## 目标

实现类似 VS Code `Ctrl+P` / Sublime Text `Cmd+Shift+P` 的 command palette：

```
╭────────────────────────────────╮
│ > toggle bor█                  │
│                                │
│   Toggle borders           b   │  ← 匹配项高亮
│   Toggle layout            l   │
│   ────────────────────────     │
│   Cycle filter             f   │  ← 不匹配但显示
│   Cycle theme              t   │
│   New session              n   │
│   Kill session             x   │
│   Quit                     q   │
╰────────────────────────────────╯
```

居中浮层，输入文字实时过滤可用命令，Enter 执行选中命令。

## 交互设计

### 打开

- **快捷键**：`Ctrl+p` 或 `:` （在 sidebar focus 模式下）
- 弹出居中浮层，输入框在最上方

### 命令列表

```
命令名               快捷键    描述
────────────────────────────────────
Switch session       Enter     切换到选中 session
New session          n         创建新 session
Kill session         x         终止选中 session
Rename session       r         重命名选中 session
────────────────────
Toggle layout        l         切换水平/垂直布局
Toggle borders       b         切换边框显示
Toggle compact       c         切换紧凑/展开视图
────────────────────
Cycle theme          t         切换主题
Cycle filter         f         切换过滤器
────────────────────
Toggle sidebar       Ctrl+s    显示/隐藏 sidebar
Show help            h/?       显示快捷键帮助
Quit                 q         退出程序
```

### 搜索/过滤

- 输入文字实时过滤命令列表
- 匹配逻辑：命令名的子串匹配（不区分大小写）
- 匹配字符高亮显示
- 第一个匹配项自动选中
- `↑/↓` 在匹配结果间导航

### 执行

- `Enter`：执行选中命令
- `Esc`：关闭 palette，不执行任何操作

### 上下文感知（可选增强）

- 在 Main focus 模式下打开 palette，只显示全局命令
- 在 Sidebar focus 模式下打开 palette，显示 session 相关命令 + 全局命令
- 当前 session 相关的命令优先显示

## 实现方案

### 命令注册表

```rust
// command.rs（新文件）

#[derive(Debug, Clone)]
pub struct Command {
    pub name: &'static str,
    pub shortcut: &'static str,
    pub description: &'static str,
    pub category: CommandCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Session,
    Layout,
    Theme,
    Navigation,
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    SwitchSession,
    NewSession,
    KillSession,
    RenameSession,
    ToggleLayout,
    ToggleBorders,
    ToggleCompact,
    CycleTheme,
    CycleFilter,
    ToggleSidebar,
    ShowHelp,
    Quit,
}

pub const COMMANDS: &[(CommandAction, Command)] = &[
    (CommandAction::SwitchSession, Command {
        name: "Switch session",
        shortcut: "Enter",
        description: "Switch to the focused session",
        category: CommandCategory::Session,
    }),
    (CommandAction::NewSession, Command {
        name: "New session",
        shortcut: "n",
        description: "Create a new tmux session",
        category: CommandCategory::Session,
    }),
    // ... 其他命令 ...
];
```

### Palette 状态

```rust
// app.rs

pub struct App {
    // ... 现有字段 ...
    palette: Option<PaletteState>,
}

struct PaletteState {
    query: String,
    selected: usize,
    /// 匹配的命令 indices（指向 COMMANDS）
    filtered: Vec<usize>,
}

impl PaletteState {
    fn new() -> Self {
        let filtered = (0..COMMANDS.len()).collect();
        Self {
            query: String::new(),
            selected: 0,
            filtered,
        }
    }

    fn update_filter(&mut self) {
        self.filtered = COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, (_, cmd))| {
                if self.query.is_empty() {
                    return true;
                }
                cmd.name.to_lowercase().contains(&self.query.to_lowercase())
            })
            .map(|(i, _)| i)
            .collect();
        if self.selected >= self.filtered.len() {
            self.selected = 0;
        }
    }
}
```

### UI 渲染

```rust
// ui.rs

pub fn draw_command_palette(
    frame: &mut Frame,
    area: Rect,   // 整个终端区域
    query: &str,
    selected: usize,
    filtered_commands: &[(CommandAction, &Command)],
    theme: &Theme,
) {
    // 居中浮层
    let width = 40u16.min(area.width.saturating_sub(4));
    let height = (filtered_commands.len() as u16 + 4).min(area.height.saturating_sub(4));
    let x = (area.width - width) / 2;
    let y = (area.height - height) / 3;  // 偏上 1/3

    let palette_area = Rect::new(x, y, width, height);

    // 清除底层
    frame.render_widget(Clear, palette_area);

    // 边框
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.surface));
    let inner = block.inner(palette_area);
    frame.render_widget(block, palette_area);

    // 输入框（第 1 行）
    let input_area = Rect { height: 1, ..inner };
    let input_line = Line::from(vec![
        Span::styled("> ", Style::default().fg(theme.accent)),
        Span::styled(query, Style::default().fg(theme.text)),
        Span::styled("█", Style::default().fg(theme.accent)),
    ]);
    frame.render_widget(Paragraph::new(vec![input_line]), input_area);

    // 分隔线（第 2 行）
    let sep_area = Rect { y: inner.y + 1, height: 1, ..inner };
    let sep = Line::from(Span::styled(
        "─".repeat(inner.width as usize),
        Style::default().fg(theme.dim),
    ));
    frame.render_widget(Paragraph::new(vec![sep]), sep_area);

    // 命令列表（第 3 行起）
    let list_area = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };

    let inner_w = list_area.width as usize;
    let lines: Vec<Line> = filtered_commands
        .iter()
        .enumerate()
        .take(list_area.height as usize)
        .map(|(i, (_, cmd))| {
            let is_selected = i == selected;
            let bg = if is_selected { theme.accent } else { theme.surface };
            let fg = if is_selected { theme.bg } else { theme.secondary };
            let shortcut_fg = if is_selected { theme.bg } else { theme.dim };

            let shortcut_width = cmd.shortcut.len();
            let name_width = inner_w.saturating_sub(shortcut_width + 3);
            let name = format!(" {:<width$}", cmd.name, width = name_width);
            let shortcut = format!("{:>width$} ", cmd.shortcut, width = shortcut_width);

            Line::from(vec![
                Span::styled(name, Style::default().fg(fg).bg(bg)),
                Span::styled(shortcut, Style::default().fg(shortcut_fg).bg(bg)),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), list_area);
}
```

### 键盘处理

```rust
fn handle_palette_key(&mut self, key: KeyEvent) -> bool {
    let palette = self.palette.as_mut().unwrap();
    match key.code {
        KeyCode::Esc => {
            self.palette = None;
        }
        KeyCode::Enter => {
            if let Some(&cmd_idx) = palette.filtered.get(palette.selected) {
                let action = COMMANDS[cmd_idx].0;
                self.palette = None;
                return self.execute_command(action);
            }
        }
        KeyCode::Up => {
            if palette.selected > 0 {
                palette.selected -= 1;
            }
        }
        KeyCode::Down => {
            if palette.selected + 1 < palette.filtered.len() {
                palette.selected += 1;
            }
        }
        KeyCode::Backspace => {
            palette.query.pop();
            palette.update_filter();
        }
        KeyCode::Char(c) => {
            palette.query.push(c);
            palette.update_filter();
        }
        _ => {}
    }
    false
}

fn execute_command(&mut self, action: CommandAction) -> bool {
    match action {
        CommandAction::Quit => return true,
        CommandAction::ToggleLayout => { /* ... */ }
        CommandAction::ToggleBorders => { /* ... */ }
        // ... 其他命令映射到现有逻辑 ...
    }
    false
}
```

## 测试计划

- [ ] `Ctrl+P` 打开 command palette
- [ ] 显示所有可用命令
- [ ] 输入文字实时过滤命令
- [ ] `↑/↓` 在命令间导航
- [ ] `Enter` 执行选中命令
- [ ] `Esc` 关闭 palette
- [ ] 命令执行后 palette 自动关闭
- [ ] 快捷键右对齐显示
- [ ] 宽度自适应终端大小
- [ ] 搜索结果为空时显示 "No matching commands"
- [ ] palette 打开时不响应其他快捷键
- [ ] palette 在 main focus 和 sidebar focus 下都能打开

## 注意事项

- 这个功能复杂度较高，建议在基础 UI 改进（idle 时间、git 符号、滚动指示器等）完成后再实现
- Command palette 的核心价值是**可发现性**，如果快捷键数量不多（< 15），help 面板可能就够了
- 如果实现了 command palette，可以考虑把 help 面板的内容迁移过来，减少两套展示

## 依赖

- 建议先实现 [ui-06-session-rename](ui-06-session-rename.md)，这样 palette 中才有 rename 命令
- 建议先实现 [ui-02-compact-mode](ui-02-compact-mode.md)，这样 palette 中才有 toggle compact 命令

## 相关文件

- `src/command.rs`：新文件，命令注册表
- `src/app.rs`：palette 状态和键盘处理
- `src/ui.rs`：palette 渲染

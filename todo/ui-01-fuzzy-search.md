# 模糊搜索（Fuzzy Filter）

## 优先级：🔴 高
## 复杂度：中
## 价值：高

## 问题

当前只有 All / Working / Idle 三态过滤（`f` 键循环）。当 session 数量超过 5 个，找到目标 session 需要反复按 `j/k` 滚动。这是 session 数量增长后的第一痛点。

## 目标

按 `/` 进入搜索模式，实时按名字过滤 session 列表。类似 fzf / lazygit 的体验。

## 交互设计

### 进入搜索

```
按 / → sidebar footer 变成搜索输入框：

╭─────────────────────────╮
│  Projects  3            │
│                         │
│  ▌  1  my-project       │  ← 匹配的 session
│       ~/code/my-proj    │
│       main              │
│       clean             │
│       active ⠋          │
│                         │
│  ▌  2  my-app           │  ← 匹配的 session
│       ~/code/my-app     │
│       ...               │
│                         │
│─────────────────────────│
│  / my█                  │  ← 搜索输入框，光标在这里
╰─────────────────────────╯
```

### 搜索过程中

- 每输入一个字符，实时过滤 session 列表
- 匹配逻辑：session name 包含输入的子串（不区分大小写）
- 第一个匹配项自动获得焦点
- `j/k` 或 `↑/↓` 在匹配结果间导航
- `Enter` 确认选中并切换到该 session（同时退出搜索模式）
- `Esc` 取消搜索，恢复原来的过滤状态和焦点位置
- `Backspace` 删除最后一个字符
- 搜索为空时显示所有 session（等价于退出搜索）

### 退出搜索

- `Enter`：确认选中，切换到该 session，回到 Main focus
- `Esc`：取消搜索，恢复之前的 filtered 列表和 focused 位置
- 输入框清空：自动退出搜索模式

### 进阶：模糊匹配（可选）

初版用 substring 匹配即可。后续可以加模糊匹配：
- `mp` 匹配 `my-project`（首字母匹配）
- 匹配的字符高亮显示

## 实现方案

### 新增状态

```rust
// app.rs
pub struct App {
    // ... 现有字段 ...
    
    /// 搜索模式是否激活
    search_active: bool,
    /// 搜索输入内容
    search_query: String,
    /// 进入搜索前保存的焦点位置，用于 Esc 恢复
    search_saved_focused: usize,
    /// 进入搜索前保存的 filter_mode，用于 Esc 恢复
    search_saved_filter: FilterMode,
}
```

### 修改过滤逻辑

```rust
fn recompute_filter(&mut self) {
    self.filtered.clear();
    for (i, session) in self.sessions.iter().enumerate() {
        // 先应用 filter_mode
        let passes_filter = match self.filter_mode {
            FilterMode::All => true,
            FilterMode::Working => session.idle_seconds < 3,
            FilterMode::Idle => session.idle_seconds >= 3,
        };
        
        // 再应用搜索
        let passes_search = if self.search_active && !self.search_query.is_empty() {
            session.name.to_lowercase().contains(&self.search_query.to_lowercase())
        } else {
            true
        };
        
        if passes_filter && passes_search {
            self.filtered.push(i);
        }
    }
    // 搜索模式下自动聚焦第一个匹配项
    if self.search_active && self.focused >= self.filtered.len() {
        self.focused = 0;
    }
}
```

### 修改键盘处理

```rust
fn handle_sidebar_key(&mut self, key: KeyEvent) -> bool {
    // 搜索模式下的键盘处理
    if self.search_active {
        return self.handle_search_key(key);
    }
    
    // ... 现有逻辑 ...
    
    // 新增：/ 进入搜索
    KeyCode::Char('/') => {
        self.search_active = true;
        self.search_query.clear();
        self.search_saved_focused = self.focused;
        self.search_saved_filter = self.filter_mode;
    }
}

fn handle_search_key(&mut self, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            // 取消搜索，恢复状态
            self.search_active = false;
            self.search_query.clear();
            self.focused = self.search_saved_focused;
            self.filter_mode = self.search_saved_filter;
            self.recompute_filter();
        }
        KeyCode::Enter => {
            // 确认选中
            self.search_active = false;
            self.search_query.clear();
            self.switch_project();
            self.focus_mode = FocusMode::Main;
        }
        KeyCode::Backspace => {
            self.search_query.pop();
            self.recompute_filter();
        }
        KeyCode::Char(c) => {
            self.search_query.push(c);
            self.recompute_filter();
        }
        KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
            // 在搜索结果间导航（但 j 在搜索模式下应该输入字符）
            // 只用 Down 键导航，j 作为普通字符输入
        }
        _ => {}
    }
    false
}
```

### 修改 UI

```rust
// ui.rs — 修改 draw_footer

// 搜索模式下 footer 显示搜索框
if search_active {
    let search_line = Line::from(vec![
        Span::styled(" / ", Style::default().fg(theme.accent)),
        Span::styled(&search_query, Style::default().fg(theme.text)),
        Span::styled("█", Style::default().fg(theme.accent)),  // 模拟光标
    ]);
    // 替换 footer 的 hints 行
}
```

### 修改 help 面板

在 help 面板中添加搜索的说明：

```
  /         search sessions
```

## 测试计划

- [ ] 按 `/` 进入搜索模式，footer 显示搜索输入框
- [ ] 输入字符实时过滤 session 列表
- [ ] 大小写不敏感匹配
- [ ] `Backspace` 删除字符，列表实时更新
- [ ] 搜索结果为空时显示 "No matches"
- [ ] `Enter` 确认选中并切换 session
- [ ] `Esc` 取消搜索，恢复原状态（焦点、过滤器）
- [ ] 搜索模式下 `j/k` 的行为（j 应输入字符，↑↓ 导航）
- [ ] 搜索模式下其他快捷键不应触发（比如 `q` 不应退出）
- [ ] 搜索字符串清空后行为正确

## 注意事项

- 搜索模式下 `j`、`k`、`q`、`f` 等应作为普通字符输入，不触发快捷键
- 搜索模式下只有 `↑/↓`（方向键）用于导航
- 搜索输入框需要处理 Unicode 字符（session 名可能包含非 ASCII）
- 搜索时 `filter_mode` 仍然生效（搜索是在当前过滤结果上叠加的）

## 相关文件

- `src/app.rs`：状态管理和键盘处理
- `src/ui.rs`：footer 渲染修改

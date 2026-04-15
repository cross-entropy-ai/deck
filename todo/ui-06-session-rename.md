# 会话重命名

## 优先级：🟡 中
## 复杂度：中
## 价值：中

## 问题

当前没有办法在 sidebar 内重命名 session。用户需要切换到 tmux 主界面执行 `tmux rename-session`。这打断了 sidebar 的操作流。

自动生成的 session 名字（`session-0`, `session-1`）不够直观，用户通常希望改为有意义的名字。

## 目标

按 `r` 进入 inline rename 模式，直接在 sidebar 中编辑 session 名字。

## 交互设计

### 进入 rename

```
正常状态：
 ▌  3  my-project
       ~/code/my-project
       main
       clean
       idle 5m

按 r 后进入 rename 模式：
 ▌  3  █my-project         ← 名字变为可编辑，光标在最前面
       ~/code/my-project       全选状态，输入任何字符替换整个名字
       main
       clean
       idle 5m
```

### 编辑过程

- 进入 rename 时，输入框预填充当前名字，全选状态
- 输入第一个字符时清除原名字（全选替换行为）
- 如果第一个按键是方向键，则进入编辑模式（不清除）
- `←/→` 移动光标
- `Home/End` 跳到开头/结尾
- `Backspace` 删除光标前的字符
- `Delete` 删除光标后的字符
- `Enter` 确认重命名
- `Esc` 取消，恢复原名字

### 简化方案（推荐先实现）

不做光标移动，只支持：
- 进入时清空，从头开始输入
- `Backspace` 删除最后一个字符
- `Enter` 确认
- `Esc` 取消

这样实现简单很多，覆盖 90% 的使用场景。

### 验证

- 名字不能为空
- 名字不能与已有 session 重名
- 名字不能包含 tmux 不允许的字符（`.` 和 `:`）

### 错误处理

```
重名时：
 ▌  3  █existing-name      ← 红色边框或名字变红
       name already exists  ← 提示信息替换目录行

空名字时：
 ▌  3  █                   
       name cannot be empty
```

## 实现方案

### 新增状态

```rust
// app.rs
pub struct App {
    // ... 现有字段 ...
    
    /// 重命名模式是否激活
    renaming: bool,
    /// 重命名输入框的当前内容
    rename_input: String,
    /// 被重命名的 session 的原始名字（用于 Esc 恢复和执行 rename）
    rename_original: String,
    /// 重命名验证错误信息
    rename_error: Option<String>,
}
```

### 新增 tmux rename 命令

```rust
// tmux.rs

/// 重命名一个 session
pub fn rename_session(old_name: &str, new_name: &str) -> bool {
    tmux(&["rename-session", "-t", old_name, new_name]).is_some()
}
```

### 快捷键处理

```rust
// app.rs — handle_sidebar_key

KeyCode::Char('r') => {
    if let Some(&session_idx) = self.filtered.get(self.focused) {
        self.renaming = true;
        self.rename_original = self.sessions[session_idx].name.clone();
        self.rename_input = self.rename_original.clone();
        self.rename_error = None;
    }
}
```

### rename 模式的键盘处理

```rust
fn handle_rename_key(&mut self, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            self.renaming = false;
            self.rename_input.clear();
            self.rename_error = None;
        }
        KeyCode::Enter => {
            let new_name = self.rename_input.trim().to_string();
            
            // 验证
            if new_name.is_empty() {
                self.rename_error = Some("name cannot be empty".to_string());
                return false;
            }
            if new_name.contains('.') || new_name.contains(':') {
                self.rename_error = Some("name cannot contain . or :".to_string());
                return false;
            }
            if new_name != self.rename_original 
                && self.sessions.iter().any(|s| s.name == new_name) {
                self.rename_error = Some("name already exists".to_string());
                return false;
            }
            
            // 执行重命名
            if tmux::rename_session(&self.rename_original, &new_name) {
                // 更新 session_order
                if let Some(pos) = self.session_order.iter().position(|n| n == &self.rename_original) {
                    self.session_order[pos] = new_name;
                }
                self.refresh_sessions();
            }
            
            self.renaming = false;
            self.rename_input.clear();
            self.rename_error = None;
        }
        KeyCode::Backspace => {
            self.rename_input.pop();
            self.rename_error = None;  // 清除错误，用户在修改
        }
        KeyCode::Char(c) => {
            // 第一次输入时，如果内容还是原始名字，清空（全选替换行为）
            if self.rename_input == self.rename_original {
                self.rename_input.clear();
            }
            self.rename_input.push(c);
            self.rename_error = None;
        }
        _ => {}
    }
    false
}
```

### UI 修改

```rust
// ui.rs — 修改 draw_sessions 中的 Row 1

// 当 renaming 且是 focused session 时，显示编辑框
if renaming && is_focused {
    // Row 1: 编辑框
    let input_style = if rename_error.is_some() {
        Style::default().fg(theme.yellow).bg(bg)  // 错误时变黄
    } else {
        Style::default().fg(theme.text).bg(bg).add_modifier(Modifier::UNDERLINED)
    };
    
    lines.push(pad_line(vec![
        Span::styled(accent, Style::default().fg(accent_color).bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
        Span::styled(&rename_input, input_style),
        Span::styled("█", Style::default().fg(theme.accent).bg(bg)),  // 光标
    ], bg, width));
    
    // Row 2: 错误信息（如果有）或目录
    if let Some(ref error) = rename_error {
        lines.push(pad_line(vec![
            Span::styled("      ", Style::default().bg(bg)),
            Span::styled(error, Style::default().fg(theme.yellow).bg(bg)),
        ], bg, width));
    } else {
        // 正常显示目录
        // ...
    }
} else {
    // 正常渲染
    // ...
}
```

### 传参给 UI

```rust
// app.rs — render()

// 需要把 renaming 状态传给 UI
// 方案 1：通过 draw_sidebar 新增参数
// 方案 2：通过一个 RenameState 结构体

pub struct RenameState {
    pub active: bool,
    pub input: String,
    pub error: Option<String>,
}
```

## 测试计划

- [ ] 按 `r` 进入 rename 模式，显示编辑框
- [ ] 输入字符替换原名字（全选行为）
- [ ] `Backspace` 删除字符
- [ ] `Enter` 确认重命名，tmux session 名字更新
- [ ] `Esc` 取消，恢复原名字
- [ ] 空名字时显示错误提示，不执行 rename
- [ ] 重名时显示错误提示，不执行 rename
- [ ] 包含 `.` 或 `:` 时显示错误提示
- [ ] rename 成功后 session_order 更新
- [ ] rename 成功后列表正确刷新
- [ ] 在非 focused session 上不能触发 rename
- [ ] rename 模式下其他快捷键不应触发

## 相关文件

- `src/app.rs`：状态管理和键盘处理
- `src/ui.rs`：编辑框渲染
- `src/tmux.rs`：新增 `rename_session` 函数

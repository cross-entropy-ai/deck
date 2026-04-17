# 分隔线视觉增强

## 状态：已完成（2026-04-15）
## 优先级：🟢 低（顺手做）
## 复杂度：极低
## 价值：低

## 当前实现

- 正常状态：`│` + `theme.dim`
- hover 状态：`┃` + `theme.accent`
- 拖拽状态：`┃` + `theme.green`
- vertical layout 不受影响

## 备注

最终实现采用了方案 A，没有加入中间箭头提示，保持实现简单且信号足够明显。

## 问题

当前的 sidebar 与 main pane 之间的分隔线只有两种状态：

```
正常：│  （theme.dim 颜色）
hover：│  （theme.subtle 颜色，比 dim 稍亮）
```

颜色差异很小，用户很难注意到分隔线是可拖拽的。第一次使用时，用户不会意识到可以拖拽调整 sidebar 宽度。

## 目标

让分隔线的交互性更明显：
1. hover 时有更显著的视觉变化
2. 拖拽时有明确的反馈
3. 光标形状提示（如果终端支持）

## 设计

### 状态变化

```
正常状态：
  │    细线，dim 颜色

hover 状态：
  ┃    粗线，accent 颜色，或者 ⟨⟩ 提示

拖拽状态：
  ┃    粗线，高亮颜色
```

### 方案 A：粗线 + 颜色变化（推荐，最简单）

```rust
// 正常
'│'  fg: theme.dim

// hover
'┃'  fg: theme.accent     // 粗线 + accent 色

// 拖拽
'┃'  fg: theme.green      // 粗线 + 绿色（表示"正在操作"）
```

### 方案 B：双箭头提示

```
正常：│
hover：◀▶  或  ⟨⟩  或  ⇔
```

在 hover 时，分隔线中间的某一行显示左右箭头，提示可拖拽方向。

```
│
│
│
◀▶   ← 只在分隔线中间显示
│
│
│
```

### 方案 C：光标变化 + 粗线

使用 ANSI escape 改变光标形状（部分终端支持）：

```rust
// hover 时发送 cursor shape change
// \x1b[6 q = steady bar cursor
// 这个方案兼容性差，不推荐
```

## 实现方案（方案 A，推荐）

### 修改分隔线渲染

当前代码在 `app.rs` render 函数中：

```rust
// 现有代码
if let Some(gap) = gap_area {
    let sep_fg = if hover_sep { theme.subtle } else { theme.dim };
    for y in gap.y..gap.bottom() {
        if let Some(cell) = frame.buffer_mut().cell_mut((gap.x, y)) {
            cell.set_char('│');
            cell.set_style(ratatui::style::Style::default().fg(sep_fg));
        }
    }
}
```

改为：

```rust
if let Some(gap) = gap_area {
    let (sep_char, sep_fg) = if self.dragging_separator {
        ('┃', theme.green)       // 拖拽中：粗线 + 绿色
    } else if hover_sep {
        ('┃', theme.accent)      // hover：粗线 + accent
    } else {
        ('│', theme.dim)         // 正常：细线 + dim
    };
    for y in gap.y..gap.bottom() {
        if let Some(cell) = frame.buffer_mut().cell_mut((gap.x, y)) {
            cell.set_char(sep_char);
            cell.set_style(ratatui::style::Style::default().fg(sep_fg));
        }
    }
}
```

### 可选：添加拖拽提示（方案 B 的简化版）

在分隔线的中间位置显示一个提示符号：

```rust
if let Some(gap) = gap_area {
    let mid_y = gap.y + gap.height / 2;
    
    for y in gap.y..gap.bottom() {
        if let Some(cell) = frame.buffer_mut().cell_mut((gap.x, y)) {
            if hover_sep && y == mid_y {
                // 中间位置显示拖拽提示
                cell.set_char('⇔');
                cell.set_style(ratatui::style::Style::default().fg(theme.accent));
            } else {
                let (ch, fg) = if self.dragging_separator {
                    ('┃', theme.green)
                } else if hover_sep {
                    ('┃', theme.accent)
                } else {
                    ('│', theme.dim)
                };
                cell.set_char(ch);
                cell.set_style(ratatui::style::Style::default().fg(fg));
            }
        }
    }
}
```

## 测试计划

- [ ] 正常状态显示细线 `│`，dim 颜色
- [ ] 鼠标 hover 到分隔线时变为粗线 `┃`，accent 颜色
- [ ] 拖拽时粗线 `┃`，绿色
- [ ] 鼠标移开后恢复细线
- [ ] 释放鼠标后恢复 hover 或正常状态
- [ ] vertical layout 模式下不显示分隔线（不受影响）
- [ ] 确认 `┃` 和 `⇔` 在常见终端中渲染正确

## 注意事项

- `┃` (U+2503 BOX DRAWINGS HEAVY VERTICAL) 在所有现代终端中都能渲染
- `⇔` (U+21D4 LEFT RIGHT DOUBLE ARROW) 可能在某些字体中宽度不一致，需要测试
- 如果 `⇔` 有问题，可以用 `◆` 或 `●` 代替
- 这个改动只涉及 render 函数中约 10 行代码，非常安全

## 相关文件

- `src/app.rs`：render 函数中的分隔线渲染（约第 362-370 行）

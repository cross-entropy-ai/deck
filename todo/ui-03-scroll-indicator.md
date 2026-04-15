# 滚动指示器

## 优先级：🔴 高
## 复杂度：低
## 价值：中高

## 问题

当前 session 列表超出可视区域时，没有任何视觉提示。用户不知道：
- 上面是否还有更多 session
- 下面是否还有更多 session
- 当前在列表中的位置

唯一的线索是 header 中的总数（如 `Projects 10`），但需要用户自己心算当前看到的是第几个。

## 目标

提供清晰的滚动位置指示，让用户始终知道列表的全局位置。

## 设计方案

### 方案 A：顶部/底部提示文字（推荐）

当 session 列表溢出时，在可视区域的顶部和底部显示提示：

```
╭─────────────────────────╮
│  Projects  10           │
│                         │
│  ▲ 2 more               │  ← 上方有 2 个不可见的 session
│                         │
│  ▌  3  project-c        │
│       ~/code/project-c  │
│       ...               │
│                         │
│  ▌  4  project-d        │
│       ~/code/project-d  │
│       ...               │
│                         │
│  ▼ 6 more               │  ← 下方有 6 个不可见的 session
│                         │
│─────────────────────────│
│  j/k nav  h/? help  q  │
╰─────────────────────────╯
```

- 提示文字用 `theme.dim` 颜色，不抢眼
- 当已经在顶部时，不显示 `▲`
- 当已经在底部时，不显示 `▼`

### 方案 B：Header 中显示位置

```
  Projects  3/10          ← 当前焦点是第 3 个，共 10 个
```

或者：

```
  Projects  10  [3]       ← 方括号内是焦点位置
```

这个方案更简洁，不占用 session 列表的空间。

### 方案 C：右侧滚动条（最完整但复杂度高）

在 sidebar 的最右列画一条细滚动条：

```
╭─────────────────────────╮
│  Projects  10          ░│  ← 滚动条轨道
│                        ░│
│  ▌  3  project-c       █│  ← 滚动条滑块（当前位置）
│       ~/code/project-c █│
│       ...              █│
│                        ░│
│  ▌  4  project-d       ░│
│       ~/code/project-d ░│
│       ...              ░│
│                        ░│
│─────────────────────────│
│  j/k nav  h/? help  q  │
╰─────────────────────────╯
```

使用字符：`░` = 轨道，`█` = 滑块

### 推荐：A + B 组合

同时实现方案 A 和 B——header 显示位置数字，列表边缘显示溢出提示。

## 实现方案

### 计算可见范围

```rust
// ui.rs

struct ScrollInfo {
    /// 列表中第一个可见 session 的 index
    first_visible: usize,
    /// 列表中最后一个可见 session 的 index
    last_visible: usize,
    /// 上方不可见的数量
    above: usize,
    /// 下方不可见的数量
    below: usize,
    /// 总数
    total: usize,
    /// 焦点位置（1-based，用于显示）
    focused_display: usize,
}

fn compute_scroll_info(
    focused: usize,
    total: usize,
    visible_height: u16,
    card_height: usize,
) -> ScrollInfo {
    let visible_cards = (visible_height as usize) / card_height;
    let scroll = scroll_offset(focused, visible_height, card_height);
    let first_visible = scroll / card_height;
    let last_visible = (first_visible + visible_cards).min(total).saturating_sub(1);
    let above = first_visible;
    let below = total.saturating_sub(last_visible + 1);

    ScrollInfo {
        first_visible,
        last_visible,
        above,
        below,
        total,
        focused_display: focused + 1,
    }
}
```

### 修改 draw_header（方案 B）

```rust
fn draw_header(
    frame: &mut Frame,
    area: Rect,
    count: usize,
    theme: &Theme,
    filter_label: &str,
    focused_display: usize,  // 新增
) {
    let position_text = if count > 0 {
        format!(" {}/{}", focused_display, count)
    } else {
        format!(" {}", count)
    };

    let line = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            filter_label.to_string(),
            Style::default().fg(theme.secondary).add_modifier(Modifier::BOLD),
        ),
        Span::styled(position_text, Style::default().fg(theme.muted)),
    ]);
    frame.render_widget(
        Paragraph::new(vec![Line::raw(""), line]).style(Style::default().bg(theme.bg)),
        area,
    );
}
```

### 修改 draw_sessions（方案 A）

在 session 列表渲染前后插入溢出提示：

```rust
fn draw_sessions(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    tick: usize,
    theme: &Theme,
) {
    // ... 现有逻辑 ...

    let scroll_info = compute_scroll_info(focused, sessions.len(), area.height, CARD_HEIGHT);

    // 在渲染 session 列表之后，叠加溢出提示

    // 上方溢出提示
    if scroll_info.above > 0 {
        let hint = format!(" ▲ {} more ", scroll_info.above);
        let hint_line = Line::from(Span::styled(
            hint,
            Style::default().fg(theme.muted).bg(theme.bg),
        ));
        // 渲染在 sessions_area 的第一行
        let hint_area = Rect { height: 1, ..area };
        frame.render_widget(Paragraph::new(vec![hint_line]), hint_area);
    }

    // 下方溢出提示
    if scroll_info.below > 0 {
        let hint = format!(" ▼ {} more ", scroll_info.below);
        let hint_line = Line::from(Span::styled(
            hint,
            Style::default().fg(theme.muted).bg(theme.bg),
        ));
        // 渲染在 sessions_area 的最后一行
        let hint_area = Rect {
            y: area.bottom().saturating_sub(1),
            height: 1,
            ..area
        };
        frame.render_widget(Paragraph::new(vec![hint_line]), hint_area);
    }
}
```

### 溢出提示的淡入效果（可选）

提示文字可以用渐变效果：

```
 ▲ 2 more ─────────────
  ▌  3  project-c         ← 正常亮度
      ~/code/project-c
      ...
  ▌  4  project-d         ← 正常亮度
      ~/code/project-d
      ...
 ▼ 6 more ─────────────
```

最顶部和最底部的可见 session 可以用稍暗的颜色，暗示还有内容。

## 测试计划

- [ ] session 数量 <= 可视区域时，不显示任何溢出提示
- [ ] session 数量 > 可视区域时，正确显示 `▲ N more` 和 `▼ N more`
- [ ] 焦点在第一个 session 时，不显示 `▲`
- [ ] 焦点在最后一个 session 时，不显示 `▼`
- [ ] header 显示正确的 `N/M` 位置信息
- [ ] 滚动时提示数字实时更新
- [ ] compact 模式下滚动信息正确（card_height 不同）
- [ ] 过滤后列表变短，溢出提示正确消失/出现
- [ ] 搜索过滤后溢出提示正确
- [ ] resize 后溢出提示正确

## 相关文件

- `src/ui.rs`：draw_header 和 draw_sessions 修改
- `src/app.rs`：传递 focused 和 view_mode 给 UI

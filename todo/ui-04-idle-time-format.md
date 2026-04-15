# Idle 时间具体化

## 状态：已完成（2026-04-15）
## 优先级：🔴 高（立刻做）
## 复杂度：极低
## 价值：中

## 当前实现

- expanded 卡片在左侧 gutter 展示 idle badge，不额外增加卡片高度
- `idle_seconds < 60` 时不显示具体时长
- `idle_seconds >= 60` 时显示 `1m / 2h / 3d`
- vertical tab 模式使用同一套紧凑时长格式
- 未聚焦卡片使用更浅的颜色，不抢当前卡片的信息权重

## 备注

原始提案是显示 `idle 30s` / `idle 5m` 文本，并额外占一行。最终实现改成了更紧凑的 badge 形式，信息密度更高，也更符合当前卡片布局。

## 问题

当前只显示 `active` 或 `idle`，没有具体的空闲时长。用户无法区分"刚刚还在用"和"几天没碰了"。

```
现状：
  active ⠋     ← 有活动（idle_seconds < 3）
  idle          ← 无活动（idle_seconds >= 3），但多久？不知道
```

`idle_seconds` 字段已经存在，只是没有格式化显示。

## 目标

```
改进后：
  active ⠋      ← 3秒内有活动
  idle 30s       ← 30 秒无活动
  idle 5m        ← 5 分钟无活动
  idle 2h        ← 2 小时无活动
  idle 3d        ← 3 天无活动
```

## 实现

### 新增格式化函数

```rust
// ui.rs（或放到一个独立的 format.rs）

/// 格式化空闲时间为人类可读的字符串
fn format_idle_time(seconds: u64) -> String {
    if seconds < 3 {
        // 不应该走到这里，调用方应该判断
        return "active".to_string();
    }
    if seconds < 60 {
        return format!("idle {}s", seconds);
    }
    if seconds < 3600 {
        let minutes = seconds / 60;
        return format!("idle {}m", minutes);
    }
    if seconds < 86400 {
        let hours = seconds / 3600;
        return format!("idle {}h", hours);
    }
    let days = seconds / 86400;
    format!("idle {}d", days)
}
```

### 修改 draw_sessions 中的 Row 5

```rust
// 现有代码（expanded 模式）
// Row 5: working / idle
let mut row5 = vec![Span::styled("      ", Style::default().bg(bg))];
if session.idle_seconds < 3 {
    row5.push(Span::styled("active ", Style::default().fg(theme.green).bg(bg)));
    row5.push(Span::styled(spinner_frame, Style::default().fg(theme.green).bg(bg)));
} else {
    row5.push(Span::styled("idle", Style::default().fg(theme.dim).bg(bg)));  // ← 改这里
}

// 改为：
if session.idle_seconds < 3 {
    row5.push(Span::styled("active ", Style::default().fg(theme.green).bg(bg)));
    row5.push(Span::styled(spinner_frame, Style::default().fg(theme.green).bg(bg)));
} else {
    let idle_text = format_idle_time(session.idle_seconds);
    row5.push(Span::styled(idle_text, Style::default().fg(theme.dim).bg(bg)));
}
```

### 修改 draw_sidebar_tabs 中的 activity

```rust
// 现有代码（vertical tab 模式）
let activity = if session.idle_seconds < 3 {
    format!("active {}", spinner_frame)
} else {
    "idle".to_string()  // ← 改这里
};

// 改为：
let activity = if session.idle_seconds < 3 {
    format!("active {}", spinner_frame)
} else {
    format_idle_time(session.idle_seconds)
};
```

### 颜色分级（可选增强）

根据空闲时间长短使用不同颜色：

```rust
let idle_color = if session.idle_seconds < 60 {
    theme.subtle     // 刚空闲，浅灰
} else if session.idle_seconds < 3600 {
    theme.muted      // 几分钟，中灰
} else {
    theme.dim        // 很久了，深灰
};
```

这能让用户一眼扫出哪些 session 还"热乎"。

## 测试计划

- [ ] idle_seconds = 5 → 显示 "idle 5s"
- [ ] idle_seconds = 30 → 显示 "idle 30s"
- [ ] idle_seconds = 59 → 显示 "idle 59s"
- [ ] idle_seconds = 60 → 显示 "idle 1m"
- [ ] idle_seconds = 300 → 显示 "idle 5m"
- [ ] idle_seconds = 3599 → 显示 "idle 59m"
- [ ] idle_seconds = 3600 → 显示 "idle 1h"
- [ ] idle_seconds = 86400 → 显示 "idle 1d"
- [ ] idle_seconds = 259200 → 显示 "idle 3d"
- [ ] idle_seconds < 3 → 显示 "active ⠋"（不变）
- [ ] expanded 模式和 vertical tab 模式都正确显示
- [ ] compact 模式（如果已实现）也正确显示

## 注意事项

- 这个改动非常小，可以在 10 分钟内完成
- `format_idle_time` 函数会被 compact 模式和 vertical tab 模式复用
- 不需要秒级精度——显示 "idle 5m" 而非 "idle 5m23s"，保持简洁

## 相关文件

- `src/ui.rs`：draw_sessions 第 216-224 行，draw_sidebar_tabs 第 384-387 行

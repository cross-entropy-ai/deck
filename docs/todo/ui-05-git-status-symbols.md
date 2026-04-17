# Git Status 符号化

## 状态：已完成（2026-04-15）
## 优先级：🔴 高（立刻做）
## 复杂度：低
## 价值：中

## 当前实现

- sidebar 卡片默认使用符号化状态：`↑N ↓N +N ~N ?N`
- clean 状态显示 `✓`
- 宽度不足时自动退化为无空格紧凑格式：`↑2↓1+3~1?2`
- focused / current 卡片保留强调色，未激活卡片改为浅色显示
- vertical tab 模式已同步使用符号化格式

## 备注

原始提案里保留了“宽度足够时显示完整英文”的回退方案。最终实现直接统一为符号格式，只在极窄宽度时去掉空格，避免视觉风格来回跳。

## 问题

当前 git status 使用完整英文单词，在 sidebar 宽度有限时显示不下：

```
现状：
  2 ahead, 1 behind, 3 modified    ← 34 字符，sidebar 宽 28 时直接截断
  
  compact 降级（已有）：
  2a/1b/3m                          ← 不够直觉，需要学习 a=ahead, b=behind
```

已有的 `format_git_status(compact: bool)` 函数在 compact 模式下用 `2a/1b/3m` 这种格式，虽然比完整版短，但可读性差。

## 目标

使用 Unicode 符号替代英文缩写，在保持简洁的同时提升直觉性：

```
改进后：
  ↑2 ↓1 +3 ~1 ?2    ← 符号化，更紧凑也更直觉
  ✓                   ← clean 状态
```

符号对照：
| 符号 | 含义 | 完整英文 |
|------|------|---------|
| `↑N` | ahead N commits | N ahead |
| `↓N` | behind N commits | N behind |
| `+N` | N files staged | N staged |
| `~N` | N files modified | N modified |
| `?N` | N files untracked | N untracked |
| `✓` | working tree clean | clean |

这些符号不需要 Nerd Font，纯 Unicode 即可。

## 设计

### 自适应策略

在不同宽度下选择不同的格式：

```
宽度充裕（> 20 字符可用）：
  2 ahead, 1 behind, 3 modified    ← 完整英文

中等宽度（10-20 字符可用）：
  ↑2 ↓1 +3 ~1 ?2                  ← 符号 + 数字，用空格分隔

极窄宽度（< 10 字符可用）：
  ↑2↓1+3~1?2                      ← 符号 + 数字，无空格
```

### 颜色编码

```rust
// 每个符号可以用不同颜色
↑2    → theme.green     // ahead = 有东西要 push
↓1    → theme.yellow    // behind = 需要 pull
+3    → theme.teal      // staged = 准备好了
~1    → theme.yellow    // modified = 需要关注
?2    → theme.muted     // untracked = 次要信息
✓     → theme.green     // clean = 一切正常
```

## 实现方案

### 新增符号化格式函数

```rust
// ui.rs

/// 符号化 git status
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

### 新增带颜色的 span 构建函数

```rust
/// 构建带颜色的 git status spans
fn build_status_spans_symbols<'a>(
    session: &SessionView,
    bg: Color,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let mut spans: Vec<Span> = Vec::new();

    if session.ahead > 0 {
        if !spans.is_empty() { spans.push(Span::styled(" ", Style::default().bg(bg))); }
        spans.push(Span::styled(
            format!("↑{}", session.ahead),
            Style::default().fg(theme.green).bg(bg),
        ));
    }
    if session.behind > 0 {
        if !spans.is_empty() { spans.push(Span::styled(" ", Style::default().bg(bg))); }
        spans.push(Span::styled(
            format!("↓{}", session.behind),
            Style::default().fg(theme.yellow).bg(bg),
        ));
    }
    if session.staged > 0 {
        if !spans.is_empty() { spans.push(Span::styled(" ", Style::default().bg(bg))); }
        spans.push(Span::styled(
            format!("+{}", session.staged),
            Style::default().fg(theme.teal).bg(bg),
        ));
    }
    if session.modified > 0 {
        if !spans.is_empty() { spans.push(Span::styled(" ", Style::default().bg(bg))); }
        spans.push(Span::styled(
            format!("~{}", session.modified),
            Style::default().fg(theme.yellow).bg(bg),
        ));
    }
    if session.untracked > 0 {
        if !spans.is_empty() { spans.push(Span::styled(" ", Style::default().bg(bg))); }
        spans.push(Span::styled(
            format!("?{}", session.untracked),
            Style::default().fg(theme.muted).bg(bg),
        ));
    }

    if spans.is_empty() && !session.branch.is_empty() {
        spans.push(Span::styled("✓", Style::default().fg(theme.green).bg(bg)));
    }

    spans
}
```

### 修改现有 build_status_spans

替换现有的 `build_status_spans` 或根据可用宽度选择格式：

```rust
fn build_status_spans<'a>(
    session: &SessionView,
    is_focused: bool,
    bg: Color,
    theme: &Theme,
    max_width: usize,
) -> Vec<Span<'a>> {
    // 优先尝试符号化格式
    let symbol_spans = build_status_spans_symbols(session, bg, theme);
    let symbol_width: usize = symbol_spans.iter().map(|s| s.width()).sum();

    if symbol_width <= max_width {
        return symbol_spans;
    }

    // 宽度不够，用无空格紧凑格式
    // （极端情况，一般不会到这里）
    let compact = format_git_status(session, true);
    let dim = if is_focused { theme.subtle } else { theme.dim };
    vec![Span::styled(compact, Style::default().fg(dim).bg(bg))]
}
```

### 修改 build_tab_status（vertical tab 模式）

```rust
fn build_tab_status(session: &SessionView) -> String {
    format_git_status_symbols(session)  // 使用符号格式
}
```

## 测试计划

- [ ] 只有 ahead → 显示 `↑2`（绿色）
- [ ] 只有 behind → 显示 `↓1`（黄色）
- [ ] 只有 staged → 显示 `+3`（teal）
- [ ] 只有 modified → 显示 `~1`（黄色）
- [ ] 只有 untracked → 显示 `?2`（muted）
- [ ] 多个状态组合 → `↑2 ↓1 ~3`（空格分隔，各自颜色）
- [ ] clean 状态 → `✓`（绿色）
- [ ] no git → 不显示（或显示 `—`，保持现有行为）
- [ ] 宽度不够时降级到紧凑格式
- [ ] expanded 模式正确显示
- [ ] compact 模式正确显示
- [ ] vertical tab 模式正确显示
- [ ] 确认 ↑↓+~?✓ 在常见终端字体中渲染正确

## 注意事项

- Unicode 符号 `↑↓+~?✓` 在几乎所有现代终端中都能正确渲染，不需要特殊字体
- `+` 和 `~` 是 ASCII 字符，兼容性最好
- 如果用户终端不支持 Unicode（极少见），`↑↓✓` 可能显示为乱码，可以提供 ASCII fallback 选项
- 这个改动和 compact 模式（ui-02）是独立的，可以先做符号化，再做 compact

## 相关文件

- `src/ui.rs`：build_status_spans（第 625-652 行），format_git_status（第 578-623 行），build_tab_status（第 415-417 行）

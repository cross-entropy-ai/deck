# 主题自动跟随终端

## 优先级：🟢 低（可选）
## 复杂度：中
## 价值：低

## 问题

当前有 6 个主题，其中 5 个 dark（Catppuccin Mocha、Tokyo Night、Gruvbox、Nord、Dracula）和 1 个 light（Catppuccin Latte）。用户需要手动按 `t` 循环切换主题。

如果用户的终端是浅色背景，启动时默认 Catppuccin Mocha（深色主题）会非常刺眼。反之亦然。

## 目标

启动时自动检测终端是 light 还是 dark 模式，选择对应的主题作为默认。

## 检测方法

### 方法 1：OSC 11 查询（最可靠）

发送 `\x1b]11;?\x07` 到终端，终端会回复当前背景颜色：

```
发送：\x1b]11;?\x07
回复：\x1b]11;rgb:1111/1111/1b1b\x07    ← Catppuccin Mocha 的深色背景
```

解析回复中的 RGB 值，计算亮度，判断 light/dark。

**支持情况**：
- ✅ iTerm2, kitty, Alacritty, WezTerm, foot, xterm
- ❌ 部分老版本 Terminal.app（macOS 默认终端）
- ❓ tmux 内的转发支持取决于 tmux 版本和配置

**亮度计算**：

```rust
/// 判断颜色是否为浅色
fn is_light_color(r: u16, g: u16, b: u16) -> bool {
    // 感知亮度公式（ITU-R BT.709）
    // 输入是 16-bit 值（0-65535）
    let luminance = 0.2126 * (r as f64 / 65535.0)
                  + 0.7152 * (g as f64 / 65535.0)
                  + 0.0722 * (b as f64 / 65535.0);
    luminance > 0.5
}
```

### 方法 2：`COLORFGBG` 环境变量

某些终端设置 `COLORFGBG` 环境变量：

```bash
COLORFGBG="15;0"     ← 前景色 15（白），背景色 0（黑）→ dark mode
COLORFGBG="0;15"     ← 前景色 0（黑），背景色 15（白）→ light mode
```

解析背景色的 ANSI 色号：0-6 = dark，7-15 = light（粗略判断）。

**支持情况**：
- ✅ rxvt, urxvt, xterm（部分）
- ❌ 很多现代终端不设置这个变量
- 不够可靠，只作为 fallback

### 方法 3：`TERM_PROGRAM` + 系统外观（macOS 特定）

在 macOS 上，检测系统是否为 dark mode：

```bash
defaults read -g AppleInterfaceStyle 2>/dev/null
# 输出 "Dark" 表示 dark mode，命令失败表示 light mode
```

结合 `TERM_PROGRAM`（判断是否在原生终端中），可以推测主题偏好。

### 推荐策略

优先级从高到低：

1. 用户配置文件中已保存的主题 → 直接使用（最高优先级）
2. OSC 11 查询 → 检测终端背景色
3. `COLORFGBG` 环境变量 → fallback
4. 默认 Catppuccin Mocha → 最终 fallback

## 实现方案

### 检测函数

```rust
// theme.rs 或新文件 detect.rs

use std::io::{self, Read, Write};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMode {
    Light,
    Dark,
    Unknown,
}

/// 尝试检测终端是 light 还是 dark 模式
pub fn detect_terminal_mode() -> TerminalMode {
    // 1. 尝试 OSC 11
    if let Some(mode) = detect_via_osc11() {
        return mode;
    }

    // 2. 尝试 COLORFGBG
    if let Some(mode) = detect_via_colorfgbg() {
        return mode;
    }

    // 3. macOS dark mode 检测
    #[cfg(target_os = "macos")]
    if let Some(mode) = detect_via_macos_appearance() {
        return mode;
    }

    TerminalMode::Unknown
}

fn detect_via_osc11() -> Option<TerminalMode> {
    // 切换到 raw mode
    let mut stdout = io::stdout();
    
    // 发送 OSC 11 查询
    stdout.write_all(b"\x1b]11;?\x07").ok()?;
    stdout.flush().ok()?;
    
    // 读取响应（需要设置超时）
    // 这里需要使用 termios 或 crossterm 的 raw mode
    // 响应格式：\x1b]11;rgb:RRRR/GGGG/BBBB\x07
    
    // 解析 RGB 值
    // 计算亮度
    // 返回 Light 或 Dark
    
    // 注意：这个实现需要小心处理终端 raw mode
    // 如果在 crossterm 的 raw mode 之前调用，需要临时进入 raw mode
    None  // placeholder
}

fn detect_via_colorfgbg() -> Option<TerminalMode> {
    let val = std::env::var("COLORFGBG").ok()?;
    let parts: Vec<&str> = val.split(';').collect();
    let bg_str = parts.last()?;
    let bg: u8 = bg_str.parse().ok()?;
    
    // ANSI 颜色表：0-6 通常是深色，7-15 通常是浅色
    if bg <= 6 || bg == 8 {
        Some(TerminalMode::Dark)
    } else {
        Some(TerminalMode::Light)
    }
}

#[cfg(target_os = "macos")]
fn detect_via_macos_appearance() -> Option<TerminalMode> {
    let output = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .ok()?;
    
    if output.status.success() {
        let style = String::from_utf8_lossy(&output.stdout);
        if style.trim() == "Dark" {
            return Some(TerminalMode::Dark);
        }
    }
    // 命令失败通常意味着 light mode
    Some(TerminalMode::Light)
}
```

### 修改启动逻辑

```rust
// app.rs — App::new()

let cfg = Config::load();

// 如果配置中没有保存主题，尝试自动检测
let theme_index = if !cfg.theme.is_empty() {
    THEMES.iter().position(|t| t.name == cfg.theme).unwrap_or(0)
} else {
    match detect_terminal_mode() {
        TerminalMode::Light => {
            // 找到 Catppuccin Latte（light 主题）
            THEMES.iter().position(|t| t.name == "Catppuccin Latte").unwrap_or(0)
        }
        TerminalMode::Dark | TerminalMode::Unknown => 0,  // 默认 Catppuccin Mocha
    }
};
```

### 标记主题的 light/dark 属性

```rust
// theme.rs

pub struct Theme {
    pub name: &'static str,
    pub is_light: bool,    // 新增字段
    pub bg: Color,
    // ...
}
```

这样在自动检测时可以准确匹配。

## 测试计划

- [ ] 在 dark 终端中启动，自动选择 dark 主题
- [ ] 在 light 终端中启动，自动选择 Catppuccin Latte
- [ ] 已有配置文件时，优先使用配置中的主题
- [ ] `COLORFGBG` 环境变量设置正确时能检测
- [ ] macOS dark mode 检测正确（仅 macOS）
- [ ] 检测失败时 fallback 到默认主题（不 crash）
- [ ] 手动切换主题后保存到配置，下次启动不再自动检测
- [ ] OSC 11 查询超时处理（终端不响应时不卡住）

## 风险

- **OSC 11 查询可能导致 stdin 污染**：如果终端不支持 OSC 11，查询字符可能被 echo 到屏幕上
- **Timing 问题**：OSC 11 查询需要等待终端响应，如果超时可能延迟启动
- **tmux 内的查询**：在 tmux 内运行时，OSC 11 可能返回 tmux 的背景色而非外层终端的

## 注意事项

- OSC 11 查询应该在进入 crossterm raw mode **之前**完成，或者需要特殊处理
- 检测超时应该设置得很短（50-100ms），避免影响启动速度
- 这个功能是锦上添花，检测失败就用默认值，绝不应该 crash

## 相关文件

- `src/theme.rs`：新增 `detect_terminal_mode()` 函数，`is_light` 字段
- `src/app.rs`：启动时调用检测逻辑
- `src/config.rs`：配置优先级判断

# Terminal Diagnostics (Settings 页面 Debug 功能)

日期：2026-04-16
状态：idea

## 动机

部分终端对 control sequence 的支持不完整，用户遇到功能异常（剪贴板不工作、颜色不对等）时难以定位原因。在 deck 的 settings 页面提供一个诊断功能，展示链路图告诉用户在哪一步卡住了。

## 核心链路（以 OSC 52 剪贴板为例）

```
tmux copy-mode → tmux buffer → OSC 52 escape sequence → 终端 (Ghostty) → 系统剪贴板
```

## 检测项

### 被动检测（读配置和环境变量，无风险）

| 检测项 | 方法 | 说明 |
|--------|------|------|
| 终端类型和版本 | `$TERM_PROGRAM` / `$TERM` | 识别 Ghostty / iTerm2 / Alacritty 等 |
| 是否在 tmux 内 | `$TMUX` 环境变量 | 影响 escape sequence 透传 |
| tmux 剪贴板配置 | `tmux show-option -g set-clipboard` | `on` / `external` / `off` |
| tmux 透传配置 | `tmux show-option -g allow-passthrough` | 控制 escape sequence 能否穿透 tmux |
| True color 支持 | `$COLORTERM` | `truecolor` / `24bit` |
| Unicode 支持 | `$LC_ALL` / `$LANG` | UTF-8 locale |

### 主动探测（发 escape sequence，需要处理终端响应）

| 检测项 | 方法 | 难度 |
|--------|------|------|
| OSC 52 端到端 | 写入随机字符串，`pbpaste` 读回比对 | 中等 |
| OSC 52 query | 发 `\e]52;c;?\a`，从 stdin 读响应 | 较难 |
| DA (Device Attributes) | 发 DA query，解析终端能力 | 较难 |

## UX 设计

```
┌─ Terminal Diagnostics ─────────────────────┐
│                                            │
│  Environment                               │
│  ✓ Terminal: Ghostty 1.6.1                 │
│  ✓ Inside tmux 3.5a                        │
│  ✓ $TERM = xterm-ghostty                   │
│                                            │
│  Clipboard (OSC 52)                        │
│  ✓ tmux set-clipboard = on                 │
│  ✓ Write test passed                       │
│  ✗ allow-passthrough = off                 │
│    └→ nested escape sequences won't work   │
│                                            │
│  Color                                     │
│  ✓ True color ($COLORTERM = truecolor)     │
│  ✓ 256 color fallback                      │
│                                            │
│  Unicode                                   │
│  ✓ UTF-8 locale (en_US.UTF-8)             │
│                                            │
│  [Re-run diagnostics]                      │
└────────────────────────────────────────────┘
```

## 实现要点

1. **主动探测必须用户手动触发**（按按钮 "Run diagnostics"），不能自动跑。部分终端对未知 escape sequence 可能打出乱码。

2. **超时机制**：终端不支持某个 query 时不会回应，只能靠超时判定"不支持"。crossterm 的 `poll()` 可以做带超时的等待。

3. **事件循环处理**：deck 是 ratatui 应用，crossterm 事件循环在跑。终端对 escape sequence 的回应会混在键盘事件流里，需要加一个"等待终端响应"的临时模式。

4. **OSC 52 端到端验证**：写一段随机字符串，用 `pbpaste`（macOS）或 `xclip -o` / `xsel -o`（Linux）读回比对。失败了再逐段查原因（tmux 配置？终端配置？透传问题？）。

5. **跨平台剪贴板读取**：macOS 用 `pbpaste`，Linux 用 `xclip -o` 或 `xsel -o`，需要处理工具不存在的情况。

## 实现路径

1. 先做被动检测（全部是读配置，风险低，价值高）
2. 再加 OSC 52 端到端测试（macOS 上用 `pbpaste` 验证，最实际）
3. 后续迭代加 DA query 等更复杂的主动探测

---

# vt100 补全 strikethrough (SGR 9) 支持

日期：2026-04-16
状态：idea

## 现象

在 Ghostty + tmux 中直接执行 `printf '\e[9mstrikethrough text\e[0m\n'` 可以看到删除线。但通过 deck 的 PTY 面板渲染时删除线丢失。

## 原因

两层都缺 strikethrough 支持：

1. **vt100 crate (`patches/vt100/src/attrs.rs`)** — `mode` 位掩码只定义了 bold/dim/italic/underline/inverse 五种属性，没有 strikethrough。解析器收到 `\e[9m` 时直接忽略。
2. **bridge.rs** — 即使 vt100 支持了，bridge 也没有把它映射到 ratatui 的 `Modifier::CROSSED_OUT`。

## 修复方案

### 1. `patches/vt100/src/attrs.rs`

- 添加 `TEXT_MODE_STRIKETHROUGH` 位常量（`0b0010_0000`）
- 添加 `strikethrough()` getter 和 `set_strikethrough()` setter
- 在 `write_escape_code_diff()` 中处理 strikethrough diff

### 2. vt100 SGR 解析器

- 处理 SGR 9（设置 strikethrough）
- 处理 SGR 29（取消 strikethrough）

### 3. `src/bridge.rs`

- 在 modifier 映射段添加：
  ```rust
  if cell.strikethrough() {
      modifier |= Modifier::CROSSED_OUT;
  }
  ```

## 备注

同样的模式可能适用于其他缺失的 SGR 属性（dim 等），可以一并排查补全。

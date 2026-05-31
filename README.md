<div align="center">

# deck

**A tmux session sidebar that lives inside your terminal.**

Browse and switch sessions while the main area stays attached to the current
session's PTY — no full-screen "menu replaces your shell" workflow.

<br>

[![Latest release](https://img.shields.io/github/v/release/cross-entropy-ai/deck?style=flat-square&logo=github&label=release&color=2ea043)](https://github.com/cross-entropy-ai/deck/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/cross-entropy-ai/deck/release.yml?style=flat-square&label=build&logo=githubactions&logoColor=white)](https://github.com/cross-entropy-ai/deck/actions/workflows/release.yml)
[![Downloads](https://img.shields.io/github/downloads/cross-entropy-ai/deck/total?style=flat-square&label=downloads&color=1f6feb)](https://github.com/cross-entropy-ai/deck/releases)
[![Stars](https://img.shields.io/github/stars/cross-entropy-ai/deck?style=flat-square&label=stars&color=e3b341)](https://github.com/cross-entropy-ai/deck/stargazers)
[![Built with Rust](https://img.shields.io/badge/built_with-Rust-dea584?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-8957e5?style=flat-square)](#install)

<br>

[Install](#install) · [Usage](#usage) · [Customize](#customize) · [Plugins](#plugins) · [Develop](#develop)

<br>

<img src="docs/screenshot.png" alt="deck — a tmux session sidebar inside your terminal" width="820">

</div>

---

`deck` is a [ratatui](https://github.com/ratatui-org/ratatui) sidebar UI for tmux — the sidebar lists your sessions, and the main pane stays live.

| ✅ deck **is** | 🚫 deck **is not** |
|---|---|
| A ratatui sidebar that lists, and lets you act on, your tmux sessions | A tmux replacement, a general system terminal app, or a window/pane manager |

| 💪 deck **helps you** | ✋ deck **does not help you** |
|---|---|
| Browse, switch, create, rename, kill, reorder, and filter sessions — and persist themes, layout, keybindings, and plugin commands in `~/.config/deck/config.json` | Install or configure tmux, work safely in arbitrary nested tmux setups, or do what you normally do inside a session |

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/cross-entropy-ai/deck/main/install.sh | sh
```

Or with Homebrew:

```bash
brew install cross-entropy-ai/tap/deck
```

Or download a pre-built binary from [GitHub Releases](https://github.com/cross-entropy-ai/deck/releases).

## Usage

```bash
deck
```

> [!NOTE]
> **Requires `tmux`** installed and available on `PATH`. If there are no sessions, deck tries to create a detached session named `default` so it can start.

deck runs two panes. The **sidebar** lists your tmux sessions with working directory, git branch, and idle time. The **main pane** stays attached to the focused session so the terminal never disappears behind a menu.

Configuration lives in `~/.config/deck/config.json`.

### Focus and navigation

The sidebar and main pane each capture keys in their own mode. Press `Ctrl+S` at any time to toggle focus between them.

With the sidebar focused:

| Key | Action |
|---|---|
| `j` / `k` or `↑` / `↓` | Move cursor |
| `Enter` | Switch tmux to the highlighted session and jump into it |
| `1`–`9` | Jump straight to the Nth visible session |
| `f` | Cycle filter (All / Idle / Working) |
| `x` | Kill the selected session (confirm with `y`) |
| `Alt+↑` / `Alt+↓` | Reorder sessions |
| `r` | Reload `~/.config/deck/config.json` |
| `h` or `?` | Show the full in-app help |
| `q` | Quit |

> [!TIP]
> Click a session to switch, right-click for a context menu (rename, kill, new session), or drag the edge between panes to resize.

### Customize

Most look-and-feel options have in-app toggles while the sidebar is focused:

| Key | Action |
|---|---|
| `s` | Open settings |
| `t` | Theme picker |
| `b` | Toggle pane borders |
| `l` | Horizontal ↔ vertical layout |
| `c` | Expanded ↔ compact session cards |

Anything not exposed as a hotkey lives in `~/.config/deck/config.json` — themes, keybindings, exclude patterns, plugins, update-check mode. Edit the file in your editor, then press `r` in the sidebar: deck re-applies the config without restarting. A parse error surfaces as a red banner with the line/column; the previous state stays in place so you can fix the JSON and press `r` again.

Full keybinding list and rebinding syntax are rendered in-app via `h` / `?`.

#### Plugins

Bind a key to any command and run it inside its own deck pane. Add entries to your config:

```json
{
  "plugins": [
    { "name": "GPU", "command": "nvtop", "key": "g" },
    { "name": "Top", "command": "btop",    "key": "m" }
  ]
}
```

Each plugin shows up in the sidebar; press its key while the sidebar is focused to launch it in the main pane. `Esc` returns to the terminal session.

## Develop

```bash
cargo run
```

For implementation details, see [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Build from source

```bash
cargo build --release
./target/release/deck
```

You still need tmux installed locally to run it.

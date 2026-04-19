# deck

deck wraps your tmux, providing a fast way to switch sessions and manage multiple vibe coding projects from one sidebar.

![screenshot](docs/screenshot.png)

## Core features

- **Session-first navigation**: browse and switch tmux sessions from one persistent sidebar
- **Fast session ops**: create, rename, kill, reorder, and filter sessions in-place
- **Terminal stays live**: the main pane remains attached to the selected session instead of being replaced by a menu screen
- **Keyboard and mouse support**: navigate with keys, click to switch, right-click for context actions

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

Requirements:

- `tmux` installed and available in `PATH`

deck runs two panes. The **sidebar** lists your tmux sessions with working directory, git branch, and idle time. The **main pane** stays attached to the focused session so the terminal never disappears behind a menu.

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

Click a session to switch, right-click for a context menu (rename, kill, new session), or drag the edge between panes to resize.

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

## Build from source

```bash
cargo build --release
./target/release/deck
```

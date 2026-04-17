# deck

A terminal sidebar for browsing and switching tmux sessions. See your sessions, git status, and terminal output in one view.

![screenshot](docs/screenshot.png)

## Features

- **Session sidebar** with git branch, ahead/behind, staged/modified/untracked counts
- **Instant switch** — navigate sessions with j/k or number keys, session switches as you move
- **Filter** sessions by All / Working / Idle
- **Reorder** sessions with Alt+Up/Down
- **Rename, kill, create** sessions without leaving the TUI
- **Mouse support** — click to switch, right-click for context menu, drag to resize sidebar
- **7 built-in themes** — Catppuccin Mocha, Tokyo Night, Gruvbox, Nord, Dracula, Catppuccin Latte, Claude Light
- **Horizontal & vertical** layouts, compact & expanded view modes, optional borders
- **Exclude patterns** — hide sessions by glob (`_*`) or regex (`/^test/`)
- **Tmux theme sync** — deck applies its color scheme to tmux automatically

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/cross-entropy-ai/deck/main/install.sh | sh
```

Or with Homebrew:

```bash
brew tap cross-entropy-ai/tap
brew install deck
```

Or download a pre-built binary from [GitHub Releases](https://github.com/cross-entropy-ai/deck/releases).

## Usage

```bash
deck
```

Requires `tmux` installed and available in `PATH`. Run it inside a tmux session.

### Keybindings

Defaults — all of these are rebindable (see Configuration).

| Key | Action |
|---|---|
| `Ctrl+S` | Toggle focus between sidebar and terminal |
| `j` / `k` | Navigate sessions |
| `1`-`9` | Jump to session by number |
| `Enter` | Switch to session and focus terminal |
| `x` | Kill session (with confirmation) |
| `f` | Cycle filter (All / Working / Idle) |
| `l` | Toggle horizontal/vertical layout |
| `b` | Toggle borders |
| `c` | Toggle compact/expanded view |
| `s` | Open settings |
| `t` | Open theme picker |
| `u` | Install available update (only when the update banner is showing) |
| `Alt+Up/Down` | Reorder sessions |
| `h` / `?` | Help |
| `q` | Quit |

### Configuration

Config is stored at `~/.config/deck/config.json`. On first launch deck writes every option to the file so you can see what's available:

```json
{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28,
  "sidebar_height": 4,
  "view_mode": "expanded",
  "exclude_patterns": ["_*"],
  "plugins": [],
  "update_check": "enabled",
  "keybindings": {
    "focus_next": ["j", "Down"],
    "focus_prev": ["k", "Up"],
    "switch_project": "Enter",
    "kill_session": "x",
    "open_settings": "s"
  }
}
```

Exclude patterns support glob syntax (`_*`, `scratch*`) and regex wrapped in slashes (`/^test-.+$/`).

#### Keybindings

Each entry maps a command name to one key, a list of keys, or `null` to unbind. Keys use vim-style syntax:

- Plain characters: `"j"`, `"?"`
- Named keys: `"Enter"`, `"Esc"`, `"Up"`, `"Space"`, `"F1"`…
- Modifiers: `"C-"` (Ctrl), `"A-"` (Alt), `"S-"` (Shift); combinable: `"C-A-x"`

```json
"keybindings": {
  "kill_session": "X",
  "toggle_help": ["h", "?", "F1"],
  "toggle_borders": null
}
```

Only the commands you list override the defaults; every other command keeps its default key. Open Settings → Keybindings to see the full list of available commands and their current bindings.

#### Automatic updates

deck checks GitHub for a new release on startup and every 24 hours, caching the result at `~/.config/deck/update-cache.json`. When an update is available, an inline banner appears at the bottom of the sidebar — click `upgrade` or press `u` to run `brew upgrade cross-entropy-ai/tap/deck` directly in the right pane. Set `"update_check": "disabled"` to turn it off.

## Build from source

```bash
cargo build --release
./target/release/deck
```

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

### Default keys

| Key | Action |
|---|---|
| `Ctrl+S` | Toggle focus between sidebar and terminal |
| `Enter` | Focus the terminal pane for the selected session |
| `h` / `?` | Help |
| `q` | Quit |

## Build from source

```bash
cargo build --release
./target/release/deck
```

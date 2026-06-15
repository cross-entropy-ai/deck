<div align="center">

# deck

**A tmux session sidebar that lives inside your terminal**

Browse and switch agent sessions easily.

<br>

[![Latest release](https://img.shields.io/github/v/release/cross-entropy-ai/deck?style=flat-square&logo=github&label=release&color=2ea043)](https://github.com/cross-entropy-ai/deck/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/cross-entropy-ai/deck/ci.yml?style=flat-square&label=ci&logo=githubactions&logoColor=white)](https://github.com/cross-entropy-ai/deck/actions/workflows/ci.yml)
[![Downloads](https://img.shields.io/github/downloads/cross-entropy-ai/deck/total?style=flat-square&label=downloads&color=1f6feb)](https://github.com/cross-entropy-ai/deck/releases)
[![Stars](https://img.shields.io/github/stars/cross-entropy-ai/deck?style=flat-square&label=stars&color=e3b341)](https://github.com/cross-entropy-ai/deck/stargazers)
[![Built with Rust](https://img.shields.io/badge/built_with-Rust-dea584?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-8957e5?style=flat-square)](#install)

<br>

[Install](#install) · [Usage](#usage) · [Customize](#customize) · [Develop](#develop)

<br>

<img src="docs/screenshot.png" alt="deck — a tmux session sidebar inside your terminal" width="820">

</div>

---

`deck` is a tmux enhancement tool — the sidebar lists your sessions and agents.

| ✅ deck **is** | 🚫 deck **is not** |
|---|---|
| A ratatui sidebar that lists, and lets you act on, your tmux sessions | A tmux replacement, a general system terminal app, or a window manager |

| 💪 deck **helps you** | ✋ deck **does not help you** |
|---|---|
| Browse, switch, agent sessions | Install or configure tmux, hook your agents |

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

deck runs two panes. The **sidebar** lists your tmux sessions. The **main pane** stays attached to the focused session.

Configuration lives in `~/.config/deck/config.json`.

> [!TIP]
> Click a session to switch, right-click for a context menu (rename, kill, new session), or drag the edge between panes to resize.

## Develop

```bash
cargo run
```

For implementation details, see the Architecture section of [`CLAUDE.md`](CLAUDE.md).

## Build from source

```bash
cargo build --release
./target/release/deck
```

You still need tmux installed locally to run it.

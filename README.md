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
brew install cross-entropy-ai/tap/deck
```

Or with curl:

```bash
curl -fsSL https://raw.githubusercontent.com/cross-entropy-ai/deck/main/install.sh | sh
```

Or download a pre-built binary from [GitHub Releases](https://github.com/cross-entropy-ai/deck/releases).

## Usage

```bash
deck
```

> [!NOTE]
> **Requires `tmux`** installed and available on `PATH`. On startup with no tmux sessions, deck creates a detached bootstrap session named `default`. The New Session picker suggests the next free `session-N` name.

deck runs two panes. The **sidebar** lists your tmux sessions. The **main pane** stays attached to the focused session.

Configuration lives in `~/.config/deck/config.yaml`. Existing deck or tmux-sidebar JSON configuration is migrated on first load.

> [!TIP]
> Click a session to switch, drag a session to reorder it, right-click to rename or close it, or drag the edge between panes to resize. Use the header chevron to collapse or expand the sidebar.

The Agents tab can generate a cross-session summary with either Claude Code or Codex; choose the CLI under **Settings → Agents → Summary agent**.

Deck uses standard Unicode icons by default, so a Nerd Font is not required.
Set `DECK_ICON_STYLE=ascii` for the strictest terminal compatibility, or
`DECK_ICON_STYLE=nerd` to opt into Nerd Font glyphs.

### Remote hosts

Add hosts with `deck remote add <host>` (resolved through `~/.ssh/config`), and they appear as their own sidebar sections. Deck reuses one SSH connection per host and owns those options itself — tune them under **Settings → Remote**, where you can also turn reuse off; saved port forwards stay configured but inactive while it is off, since they run as `ssh -O` commands against that shared socket.

> [!IMPORTANT]
> Deck **forwards your ssh-agent** to every configured remote by default, so shells and coding agents in a remote session can use your local keys without copying a private key there. Anyone with root on that host can use every loaded key for as long as the connection is open. Turn it off per host in `~/.config/deck/config.yaml`:
>
> ```yaml
> remotes:
>   - host: shared-box
>     forward_agent: false
> ```

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

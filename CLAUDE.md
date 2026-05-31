# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

deck is a terminal sidebar TUI for browsing and switching tmux sessions, written in Rust. It uses ratatui for rendering and crossterm for terminal I/O. Requires tmux at runtime.

## Build & Test

```bash
cargo build                    # dev build
cargo build --release          # release build
cargo check                    # type-check without building
cargo test                     # run all tests
cargo test <test_name>         # run a single test
cargo clippy                   # lint
./target/release/deck          # run the release binary (needs tmux)
```

Config is stored at `~/.config/deck/config.json`.

## Architecture

See `docs/ARCHITECTURE.md` for the full diagram. The key layers:

- **Presentation**: `ui.rs` (pure rendering functions, no mutable state) + `bridge.rs` (vt100 screen -> ratatui buffer adapter)
- **Application**: `app.rs` (event loop, state management) + `state.rs` (AppState, enums, constants) + `action.rs` (Action enum for key/mouse -> intent mapping)
- **Data**: `tmux.rs`, `git.rs` (stateless CLI wrappers), `pty.rs` (PTY lifecycle), `config.rs` (JSON persistence)
- **Theme**: `theme.rs` - static `THEMES` array of `Theme` structs with 12 named color slots (bg, surface, dim, muted, subtle, secondary, text, accent, green, teal, yellow, pink)

The rendering path: `app.run()` loop -> build `SessionView` slices (borrowed, not cloned) -> `ui::draw_*()` pure functions -> `bridge::render_screen()` for the PTY pane.

`vt100` is pinned to a long-lived `deck` branch on a fork (`Junyi-99/vt100-rust`) via `[patch.crates-io]` in `Cargo.toml`. See `docs/vt100-fork.md` for what's patched and how to add new fixes.

## Workflow Rules

Development work (bug fix or new feature):

- **Always create a new branch** before making changes. Use `feature/<name>` or `fix/<name>` naming.
- Commit on the branch, push, and open a PR into `main`. Do not push code changes directly to `main`.
- Follow existing commit message style: imperative mood, concise summary line, optional body explaining "why".

Releases are the exception — see below.

## Release

Releases do **not** go through a branch or PR. Tag `main` directly with `vX.Y.Z` and push the tag:

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

GitHub Actions builds binaries and updates the Homebrew tap (`cross-entropy-ai/homebrew-tap`). See `docs/release.md`.

### Release notes

After the release run finishes, update that release's notes to describe what
changed since the previous version (diff the previous tag against this one,
e.g. `git log v0.5.0..v0.5.1`). Write for users, not committers, and group
user-facing changes under headed sections: **New Features**, **Enhancements**,
**Bug Fixes**, and a short **Under the hood** for CI/internal-only changes.

Be careful with `@` and `#` in the notes — GitHub renders them as live
mentions/links:

- Wrap literal `@` text (e.g. the `@host` divider label, file globs, emails)
  in backticks so it doesn't ping a GitHub user.
- Use a bare `#123` only when you intend to link that exact PR/issue; wrap any
  other `#` (e.g. `#tag`, a count like `#3`) in backticks.

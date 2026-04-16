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

A patched `vt100` crate lives in `patches/vt100/` and is applied via `[patch.crates-io]` in Cargo.toml.

## Workflow Rules

- **Always create a new branch** for any feature or fix before making changes. Use `feature/<name>` or `fix/<name>` naming.
- PR into `main`. Do not push directly to `main`.
- Follow existing commit message style: imperative mood, concise summary line, optional body explaining "why".

## Release

Tag with `vX.Y.Z` and push the tag. GitHub Actions builds binaries and updates the Homebrew tap (`cross-entropy-ai/homebrew-tap`). See `docs/release.md`.

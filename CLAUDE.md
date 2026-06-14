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

Config is stored at `~/.config/deck/config.yaml`.

## Architecture

Source is split into five top-level modules under `src/` (plus `main.rs`).
`docs/cleanup-plan.md` has a fuller map of the layering:

- **`ui/`**: pure rendering functions (no mutable state), incl. `ui/sidebar/`
  for the session sidebar, `ui/bridge.rs` (vt100 screen -> ratatui buffer
  adapter), and `ui/theme.rs` (static `THEMES` array of `Theme` structs with
  12 named color slots: bg, surface, dim, muted, subtle, secondary, text,
  accent, green, teal, yellow, pink).
- **`app/`**: the event loop and dispatch (`app/mod.rs`, `app/dispatch.rs`,
  `app/update.rs`, `app/render.rs`, `app/pty.rs`) plus `app/action/` (the
  `Action` enum for key/mouse -> intent mapping, and the reducers).
- **`model/`**: `model/state.rs` (`AppState`, enums, constants),
  `model/config.rs` (YAML persistence), `model/keybindings.rs`.
- **`infra/`**: stateless backends and CLI wrappers — `infra/tmux/`
  (`local.rs` + `remote.rs`), `infra/ssh.rs`, `infra/pty.rs`,
  `infra/agent.rs`, etc. At the crate root these are aliased as
  `crate::tmux` (local) and `crate::remote_tmux` (remote).
- **`session/`**: the `SessionControl` backend trait abstracting local vs
  remote (`session/local.rs`, `session/remote.rs`, `session/executor.rs`).

The rendering path: the `app` loop builds borrowed session slices ->
`ui::draw_*()` pure functions -> `bridge::render_screen()` for the PTY pane.

`vt100` is pinned to a long-lived `deck` branch on a fork (`Junyi-99/vt100-rust`) via `[patch.crates-io]` in `Cargo.toml`. See `docs/vt100-fork.md` for what's patched and how to add new fixes.

### Local vs remote: one type, key by `Option<String>` host

deck talks to a local tmux server and N remote ones, but the **high-level
layers must not branch on local vs remote**. When adding any
per-session/per-host feature:

- **One data type for both.** Local and remote produce the *same* shape
  (e.g. `SessionInfo`, `DetectedAgent`, `AgentTarget`). Never introduce a parallel
  `foo` + `remote_foo` pair with different shapes (e.g. a scalar for
  local and a map for remote) — that leaks the distinction upward.
- **Key by host the way the rest of deck does:** `Option<String>`, where
  `None` = local and `Some(host)` = a remote host (see `KillRequest`,
  `RenameRequest`, `CreateSessionRequest`, `AppState.agents`). One
  store (`HashMap<Option<String>, T>`); absence = "not known yet".
- **Only the data-gathering branches.** `tmux/local.rs` (local) and
  `tmux/remote.rs` (ssh) gather inputs differently, then feed the *same*
  pure logic (`agent::detect_agents`, `tmux_parse::parse_sessions`). The
  renderer consumes `&[&dyn SidebarSession]` and never asks "is this
  remote?". Push the local/remote split as low as it goes.

### Remote ssh commands: mind the remote shell

`remote_tmux` sends commands as ssh argv that the **remote login shell
re-parses** (argv boundaries are lost). Two recurring traps:

- **Shell-special leading characters.** A separator/marker token must not
  start with `=` (zsh *equals-expansion* `=word` → command path — it ate
  a `===…===` probe marker on a zsh host while a bash host was fine) nor
  `-` (echo flag). Use plain `__like_this__`. Quote literal `#` formats as
  `$'#{…}'` so they aren't read as comments; single-quote user values
  (`shell_single_quote`); to pass a literal `;` to *tmux* (not the shell)
  single-quote it, but leave `;` bare when you *want* a shell separator.
- **Test against more than one host.** Remote shells differ (bash vs zsh,
  macOS vs Linux `ps`). A probe that works on one host can silently
  return nothing on another — verify across the configured hosts.

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

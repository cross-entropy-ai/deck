# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

deck is a terminal sidebar TUI for browsing and switching tmux sessions, written in Rust. It uses ratatui for rendering and crossterm for terminal I/O. Requires tmux at runtime.

## Build & Test

```bash
cargo build                    # dev build
cargo build --release          # release build
cargo check                    # type-check without building
cargo test --workspace         # run all workspace tests
cargo test <test_name>         # run a single test
cargo clippy --workspace       # lint all workspace crates
cargo fmt --all -- --check     # check formatting for all workspace crates
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
  (`local.rs` + `remote.rs`), `infra/ssh/` (the ssh client, port-forward
  command builders, listener enumeration, and ssh-specific model types
  under `infra/ssh/model/`), `infra/pty.rs`,
  `infra/agent.rs`, etc. At the crate root these are aliased as
  `crate::tmux` (local) and `crate::remote_tmux` (remote). The one
  stateful exception is `infra/ssh/agent_relay.rs`, which owns a child
  process and its mux threads per container lane. Its container-side half
  is the `agent-relay` workspace crate, a dependency-free static binary
  deck embeds from `assets/agent-relay/` (rebuilt by
  `scripts/build-agent-relay.sh`, never by `cargo build`) and streams into
  the container — see `docs/ssh-agent-forwarding.md` for why a container
  cannot be served the way a host is.
- **`session/`**: the `SessionControl` control-plane trait
  (`session/local.rs`, `session/remote.rs`, `session/executor.rs`); a
  `System` hands one out per lane via `control()`.
- **`system/`**: the `System` extension point (`system/mod.rs`) — deck is a
  shell that mounts `System`s; tmux (local + remote) is the one built-in
  (`system/tmux.rs`). A `System` owns its lanes' sidebar structure
  (`section_for`), discovery (`snapshot`), control plane (`control`), and
  divider-button behavior (`on_button`). `crate::system::for_lane(&lane)`
  resolves a lane's owning system; add a backend = `impl System` + register
  it in the `SYSTEMS` slice.

The rendering path: the `app` loop builds borrowed session slices ->
`ui::draw_*()` pure functions -> `bridge::render_screen()` for the PTY pane.

`vt100` and `crossterm` are each pinned to a long-lived `deck` branch on a fork (`Junyi-99/vt100-rust`, `Junyi-99/crossterm`) via `[patch.crates-io]` in `Cargo.toml`. See `docs/vt100-fork.md` and `docs/crossterm-fork.md` for what's patched and how to add new fixes.

### Lanes and Systems: don't branch on local vs remote (or on which system)

deck is a shell over one or more mounted `System`s; tmux exposes a *lane*
per server (the local one + each remote host). The **high-level layers must
not branch on local vs remote** — or, now, on which system owns a lane.
When adding any per-session/per-lane feature:

- **One data type for both.** Local and remote produce the *same* shape
  (e.g. `SessionInfo`, `DetectedAgent`, `LaneSnapshot`). Never introduce a
  parallel `foo` + `remote_foo` pair with different shapes — that leaks the
  distinction upward.
- **Key in-memory stores by `LaneId`** (`{system}\x1f{lane}`), not by host:
  `AppState.agents`, `collapsed_sections`, the executor's FIFO lanes. Use
  the injected `SystemRegistry` to reach the owning runtime; never match on
  the system id or `lane.lane()` outside a `System` or transport adapter.
  Session, agent, overlay, action, and effect DTOs carry `LaneId` directly.
- **The local/remote (and tmux-specific) split lives inside `TmuxSystem`.**
  `tmux/local.rs` and `tmux/remote.rs` gather inputs differently, then feed
  the *same* pure logic (`agent::detect_agents`, `tmux_parse::parse_sessions`),
  surfaced through `System::snapshot`/`section_for`/`control`. The sidebar
  renderer consumes the System's `SectionDef`/`SectionButton` and never asks
  "is this remote?". Push the split as low as it goes.

### Remote ssh commands: mind the remote shell

`remote_tmux` sends commands as ssh argv that the **remote login shell
re-parses** (argv boundaries are lost). Three recurring traps:

- **Shell-special leading characters.** A separator/marker token must not
  start with `=` (zsh *equals-expansion* `=word` → command path — it ate
  a `===…===` probe marker on a zsh host while a bash host was fine) nor
  `-` (echo flag). Use plain `__like_this__`. Quote literal `#` formats as
  `$'#{…}'` so they aren't read as comments; single-quote user values
  (`shell_single_quote`); to pass a literal `;` to *tmux* (not the shell)
  single-quote it, but leave `;` bare when you *want* a shell separator.
- **Never set a variable with an assignment *prefix*.** `run_ssh` opens every
  command with the `REMOTE_PATH_EXPORT` statement, and new commands must not
  add a `PATH=… cmd` prefix of their own: a prefix reaches one *simple
  command* only, and zsh (the macOS default login shell) *restores* what a
  prefix set as soon as that command returns — even `export`, so
  `PATH=… export PATH=…` is a no-op there and `tmux` a few `;` later is
  `command not found`. bash hides it (POSIX persists assignments before a
  special builtin).
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

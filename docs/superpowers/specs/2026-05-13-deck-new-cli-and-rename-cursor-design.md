# `deck new <name>` CLI and rename cursor keys — Design

**Status**: draft
**Date**: 2026-05-13

## Goal

Two small, independent UX improvements bundled into one design because they share no code and one PR is enough:

1. A `deck new <session-name>` CLI subcommand that creates a fresh tmux session named `<session-name>` rooted at the caller's current working directory, then launches deck with its main pane attached to that session.
2. The in-sidebar rename input gains horizontal cursor movement: Left, Right, Home, End, and forward Delete. Today the cursor only moves implicitly via insert/backspace at the tail.

## Non-goals

- A general `deck` CLI surface (e.g. `deck attach`, `deck list`, `deck kill`). `new` is added because it has a concrete launcher use case; the rest can wait until there's demand.
- Inter-process control of an already-running deck (e.g. "tell the running deck to switch to a new session"). `deck new <name>` follows the same single-instance rule as plain `deck`: a second invocation while one is running still errors out, and `--force` is the existing escape hatch.
- Cursor-key support in the `exclude_editor` input. The same limitation exists there (`src/app/action/keyboard.rs:140-148`), but fixing it is a separate change to keep this PR focused.
- Rebinding the rename input keys via the keybindings config. Rename is a text-input mode, not a sidebar command mode.

## Scope

### 1. `deck new <name>`

**CLI surface**

```
deck new <session-name>           Create a tmux session named <session-name>
                                  in the current directory and attach to it.
                                  Combine with --force to override an existing
                                  deck instance.
```

- `<session-name>` is required, positional, single argument. Spaces inside a single quoted argument are passed to tmux verbatim.
- `--force` / `-f` may appear before or after `new <name>` (`deck --force new foo` and `deck new foo --force` are both accepted). All other existing top-level flags (`--help`, `--version`) are not meaningful with `new` and produce a usage error if combined.
- Exit codes:
  - `0` — success (deck ran and exited normally).
  - `1` — runtime failure: session name already exists, or `tmux new-session` failed for any reason (the tmux error text is included in the deck stderr message).
  - `2` — usage error: missing name, extra positional argument, conflicting flag.

**Behavior**

1. Parse args. If `new` is the first non-flag token, the next token is captured as the session name and `--force` is collected from anywhere on the line. Anything else (missing name, second positional) → exit 2 with a usage hint pointing at `deck --help`.
2. Before doing anything else for `new`, check tmux: list sessions; if a session with the requested name already exists, print `deck: session '<name>' already exists` to stderr and exit 1. Deck does not launch.
3. Otherwise call the existing `tmux::new_session(name, cwd)` helper, which runs `tmux new-session -d -s <name> -c <cwd>` where `<cwd>` is `std::env::current_dir()` resolved to an absolute path. tmux's own failures (invalid name characters, unreadable cwd, etc.) surface as `None` from the helper; deck reports `deck: failed to create session '<name>'` to stderr and exits 1. (tmux's stderr is dropped by the existing helper; surfacing it would require a parallel helper, which is deferred until a real complaint shows up.)
4. On success, proceed into the normal `Run` path with an extra piece of state: an "attach override" carrying `<name>`.

**Wiring the attach override**

- `ParsedArgs` grows a field: `attach_override: Option<String>`. For plain `deck` and `deck --force`, it is `None`. For `deck new <name>`, it is `Some(<name>)`.
- `App::new` signature changes to accept this `Option<String>` and stash it in app state long enough to reach `ensure_attach_target` (passed through as a parameter to `spawn_tmux_pty`, not persisted on `AppState` — it is consumed once at startup).
- `App::ensure_attach_target(nesting_guard, attach_override)` consults the override first:
  - If `Some(name)` and `list_sessions()` contains `name`, return `name` directly.
  - Otherwise fall back to the existing logic (`nesting_guard.preferred_attach_target` → create `session-N` under `~/claude`).
- The override-not-in-list case (rare race: user killed the session between `tmux new-session` and `ensure_attach_target`) intentionally falls through silently rather than erroring; falling back to the default behavior keeps deck launchable.

**Help text update**

`print_help()` in `src/main.rs` gains one line, placed between the existing `deck` and `deck --force` entries:

```
deck new <session>         Create a session named <session> in the current
                           directory and attach to it
```

### 2. Rename input cursor keys

**Keys handled** (added to the rename branch in `src/app/action/keyboard.rs:9-17`):

| Key      | Action               |
| -------- | -------------------- |
| `Left`   | `RenameCursorLeft`   |
| `Right`  | `RenameCursorRight`  |
| `Home`   | `RenameCursorHome`   |
| `End`    | `RenameCursorEnd`    |
| `Delete` | `RenameDelete`       |

Existing keys (`Enter` → confirm, `Esc` → cancel, `Backspace` → delete left, `Char(_)` → insert at cursor) are unchanged.

**New `Action` variants** in `src/app/action/mod.rs`:

```rust
RenameCursorLeft,
RenameCursorRight,
RenameCursorHome,
RenameCursorEnd,
RenameDelete,
```

**Reducer** (`src/app/action/reduce.rs`) handles each by mutating `state.renaming.as_mut()`:

- `RenameCursorLeft`: step `cursor` back by the UTF-8 byte length of the char immediately before the cursor (use `input[..cursor].chars().last().map(char::len_utf8)`; no-op when `cursor == 0`).
- `RenameCursorRight`: step `cursor` forward by the UTF-8 byte length of the char at the cursor (use `input[cursor..].chars().next().map(char::len_utf8)`; no-op when `cursor == input.len()`).
- `RenameCursorHome`: `cursor = 0`.
- `RenameCursorEnd`: `cursor = input.len()`.
- `RenameDelete`: if `cursor < input.len()`, remove the char at `cursor` (compute its UTF-8 byte length, then `input.drain(cursor..cursor+len)`); `cursor` does not move.

All five operations are byte-safe under multi-byte input because the cursor is already tracked as a byte index and steps are taken at char (not grapheme) boundaries — matching how the existing insert/backspace operate.

The render path (`src/ui/overlays.rs:90 draw_rename_input`) already supports rendering the cursor at any position inside the string (it splits `before` / `cursor_char` / `rest`), so no UI changes are needed.

## Architecture

Both features touch the existing CLI / state / action / reduce / render pipeline; no new modules.

```
deck new <name>:
  main.rs
    ├─ parse_args                                  (pure, no tmux calls)
    │    └─ ParsedArgs { force, attach_override: Some(name) }
    └─ run() main flow, after lock + tmux preflight:
         ├─ tmux::list_sessions  (duplicate check)
         ├─ tmux::new_session(name, cwd)
         └─ App::new(.., attach_override)
            └─ spawn_tmux_pty(..., attach_override)
               └─ ensure_attach_target(nesting_guard, attach_override)

rename cursor keys:
  keyboard.rs (rename branch)
    └─ Action::RenameCursor{Left,Right,Home,End} / RenameDelete
       └─ reduce.rs (matches on Action)
          └─ mutates state.renaming.{input, cursor}
             └─ render via existing draw_rename_input
```

## Data flow

- **CLI feature**: the new `attach_override` flows one-way from `parse_args` → `App::new` → `ensure_attach_target`, consumed exactly once at startup. It is not persisted to config, not stored on `AppState` after spawn, and not observable from any later code path.
- **Rename feature**: identical to existing rename actions — keyboard event → action → reduce mutates `state.renaming` → next frame's `draw_rename_input` reads `(input, cursor)` from the state.

## Error handling

| Situation                                               | Handling                                                                                           |
| ------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `deck new` with no name                                 | stderr `deck: 'new' requires a session name.` + `Run \`deck --help\` for usage.` → exit 2          |
| `deck new foo bar`                                      | stderr `deck: unexpected argument 'bar' after \`new foo\`.` → exit 2                                |
| `deck new foo` and session `foo` already exists         | stderr `deck: session 'foo' already exists` → exit 1, deck does not launch                         |
| `deck new foo` and tmux rejects the name (invalid char) | stderr `deck: failed to create session 'foo'` → exit 1 (tmux stderr is not propagated; see Behavior step 3) |
| tmux not installed                                      | existing preflight (`src/main.rs:89`) catches this before the `new`-path block runs; no reorder needed (block goes between the preflight and the "ensure at least one session" step) |
| Override session disappears before attach (race)        | silent fallback to existing `ensure_attach_target` logic; deck launches normally                   |
| Rename cursor at boundary (Left at 0, Right at end, Delete at end) | no-op                                                                                  |

Main flow ordering for the `new` path: the existing sequence in `main.rs` is `parse_args` → SIGTERM handler → `InstanceGuard::acquire` → `tmux -V` preflight → "ensure at least one session" → launch UI. The `new`-path block (duplicate check + `tmux::new_session`) slots in between the `tmux -V` preflight and the "ensure at least one session" step, which becomes a no-op for this path because the session was just created. The instance lock is acquired *before* any tmux mutation, so a failed lock acquisition never leaves an orphan session behind.

## Testing

**Existing tests must keep passing** — no behavior change for `deck` / `deck --force` / `deck hooks ...`.

**New unit tests:**

- `src/main.rs::parse_args` (extracted to a testable shape if not already): four cases
  - `["new", "foo"]` → `Run { force: false, attach_override: Some("foo") }`
  - `["new"]` → `Err(2)`
  - `["new", "foo", "bar"]` → `Err(2)`
  - `["new", "foo", "--force"]` and `["--force", "new", "foo"]` → both yield `Run { force: true, attach_override: Some("foo") }`
- `App::ensure_attach_target`:
  - override `Some(name)` and `list_sessions` contains `name` → returns `name`
  - override `Some(name)` and `list_sessions` does not contain `name` → falls through to existing logic (covered by current tests indirectly; one explicit assertion that fallback runs)
- Rename reduce (`src/app/action/reduce.rs` tests):
  - `RenameCursorLeft` from middle of ASCII string moves back one byte
  - `RenameCursorLeft` at 0 is no-op
  - `RenameCursorRight` from middle of `中文abc` advances by 3 bytes when at a multi-byte char
  - `RenameCursorHome` / `End` move to 0 / `input.len()`
  - `RenameDelete` at middle removes the char at cursor and leaves cursor where it is
  - `RenameDelete` at end is no-op

**Not tested in v1**: end-to-end `deck new <name>` against a real tmux. The tmux-side helper is a one-line wrapper around `tmux new-session -d -s <name> -c <cwd>` and shares its failure surface with existing `tmux::new_session`. Manual smoke test before release: `deck new spec-test`, confirm session exists, confirm deck launches attached to it; then `deck new spec-test` again, confirm the duplicate-name error path.

## Migration / compatibility

- No config schema changes.
- No keybinding changes (rename keys are baked into the input handler, not the keybindings table; they cannot collide with existing user bindings).
- No tmux-server-state changes beyond creating one extra session when `deck new <name>` is used — exactly what the user asked for.

## Out of scope, deferred

- Apply the same cursor keys to the `exclude_editor` input. Same pattern; do it next.
- A `deck new <name> -c <dir>` flag to override cwd. Easy to add; wait until someone asks.
- `deck switch <name>` while a running deck is up (would need IPC). Wait for demand.

# New session: working-dir picker overlay

**Status:** design — not yet implemented
**Date:** 2026-05-18

## Problem

Creating a new tmux session in deck today goes through one of two paths:

- **In-app:** "New session" item in the global right-click menu → `Action::OpenGlobalMenu` reducer arm fires `fx.create_session = true` → `App::create_new_session()` creates a session at the hard-coded `~/claude` path with an auto-generated `session-N` name.
- **CLI:** `deck new <name>` from `App::run_new_subcommand()` in `app/lifecycle.rs`, also hard-coded to `~/claude`.

Users can't pick where the new session lives without dropping out to tmux directly. The hard-coded `~/claude` only fits one workflow.

## Goal

When the user triggers "new session" inside deck's sidebar, present an overlay with:

1. A **text input** for the working directory (editable path).
2. A **directory preview** below it listing the children of the path's effective parent, filtered by the leaf segment as a case-insensitive prefix.

The user can type a path freely, navigate the preview list with arrow keys, descend with Tab, and create the session at the displayed path with Enter. Esc cancels.

Session naming stays auto-generated (`session-N`) — out of scope for this feature.

The CLI path (`deck new <name>`) is unchanged.

## Non-goals

- Fuzzy matching beyond prefix.
- Session name input.
- Recent-dirs / bookmarks list.
- Remote filesystems / non-local paths.
- Renaming or moving directories from the picker.
- Persisting last-picked location across deck restarts.

## Trigger flow

The global menu "New session" item is rewired:

| Before | After |
|---|---|
| `MenuConfirm` reducer arm for `Some("New session")` sets `fx.create_session = true`. | Same arm fires a new action `Action::OpenNewSessionPicker` instead, which opens the overlay. |
| `App::dispatch_action` handles `fx.create_session` by calling `App::create_new_session()` with hard-coded `~/claude`. | `fx.create_session` becomes `Option<String>` carrying the chosen path. `App::create_new_session(dir: &str)` takes the path explicitly. |

`Action::OpenNewSessionPicker` is intercepted at `App::dispatch_action` (mirroring `Action::ReloadConfig`): it reads the focused session's dir, populates `NewSessionState`, calls `read_dir`, and installs the overlay. The reducer arm for this action is empty.

The user closes the overlay one of three ways:

- **Esc** — overlay clears, no side effects.
- **Enter on a valid dir** — overlay clears, `fx.create_session = Some(path)`, dispatch creates the tmux session and switches to it.
- **Enter on an invalid path** — overlay stays, `error` field set.

## State

Add to `OverlayState`:

```rust
pub new_session: Option<NewSessionState>,
```

```rust
pub struct NewSessionState {
    /// User-visible path. `~` and `..` are NOT eagerly expanded;
    /// only resolved before `read_dir` / `tmux::new_session`.
    pub input: String,
    /// Byte offset into `input`.
    pub cursor: usize,
    /// All children (directories only) of the *parent dir* of `input`.
    /// Read by dispatch; not mutated by the reducer (except cleared on
    /// reread).
    pub entries: Vec<String>,
    /// Indices into `entries` after leaf-prefix + dotfile filtering.
    /// Recomputed by the reducer whenever `input` changes; this is what
    /// the renderer iterates and what `selected` indexes into.
    pub filtered: Vec<usize>,
    /// Index into `filtered`. Reducer clamps to `0..filtered.len()`.
    pub selected: usize,
    /// Last error encountered (read_dir failure, invalid Confirm, tmux
    /// failure). Cleared on the next successful state mutation.
    pub error: Option<String>,
}
```

Extend `SideEffect`:

```rust
pub create_session: Option<String>,  // was `bool`; now carries the chosen dir
pub reread_new_session_entries: bool,
```

`SideEffect::merge` treats `create_session` like the other `Option` fields and OR's the new bool.

## Key bindings

The keyboard handler in `app/action/keyboard.rs` gates on `state.overlay.new_session.is_some()` first (sibling to the existing rename / context-menu / exclude-editor gates) and dispatches to a new `new_session_key_to_action` helper.

| Key | Action |
|---|---|
| Printable char | `NewSessionInput(char)` — insert at cursor |
| Backspace | `NewSessionBackspace` — smart back (see below) |
| Left / Right | `NewSessionCursorLeft` / `NewSessionCursorRight` |
| Home / End | `NewSessionCursorHome` / `NewSessionCursorEnd` |
| Ctrl+U | `NewSessionClear` — empty `input` |
| Ctrl+W | `NewSessionDeleteSegment` — delete back to previous `/` |
| ↑ / ↓ | `NewSessionPrev` / `NewSessionNext` — move `selected` in the filtered view |
| Tab | `NewSessionTab` — replace leaf with the currently-selected filtered entry + `/` |
| Enter | `NewSessionConfirm` |
| Esc | `NewSessionCancel` |

**Smart Backspace:** if `cursor == input.len()` and `input.ends_with('/')` and `input.len() > 1`, remove the trailing `/` plus the previous segment (= go up one level). Otherwise delete one character at `cursor - 1`.

**Tab:** uses the entry at `entries[filtered[selected]]`. On empty `filtered`, no-op.

**Dotfile visibility:** an entry whose name starts with `.` is included in `filtered` iff the current leaf starts with `.`. No explicit toggle key — picks up the shell-completion intuition (typing `.` reveals dotfiles, otherwise hidden). Rejected `Ctrl+H` because many terminals deliver it as Backspace for ASCII compatibility.

**`reread_new_session_entries`:** the reducer sets it whenever `input` mutates in a way that changes the effective parent dir. Plain char edits within the same leaf do not trigger a reread — but they DO trigger a re-filter, which the reducer does in-place.

## Layout

Centered overlay, fixed width 60, height ~12. Scrolls if entries exceed visible rows. Borders, dim background, accent for selected row, pink/yellow for error.

```
┌─ New session ─────────────────────────────────┐
│ Path: ~/projects/foo/▮                         │
│                                                │
│ ▸ src/                                         │
│   tests/                                       │
│   docs/                                        │
│   target/                                      │
│                                                │
│ ⏎ create   ⇥ complete   ⎋ cancel                │
└────────────────────────────────────────────────┘
```

With an error:

```
┌─ New session ─────────────────────────────────┐
│ Path: ~/projects/missing/▮                     │
│                                                │
│  (no entries)                                  │
│                                                │
│ ⚠ not a directory                              │
│                                                │
│ ⏎ create   ⇥ complete   ⎋ cancel                │
└────────────────────────────────────────────────┘
```

## Path handling

- **Display form** = whatever the user typed. `~`, `..`, doubled `/` are preserved verbatim in `input`.
- **Resolution** happens at FS boundaries (`read_dir`, `tmux::new_session`, dir validity check) via a helper:

```rust
fn expand_path(s: &str, home: &Path) -> PathBuf
```

  - Leading `~` or `~/...` → `$HOME[/...]`.
  - Bare relative paths (no leading `/` or `~`) resolve against `$HOME` — keeps behavior predictable without depending on deck's CWD.
  - `..` and repeated `/` normalized via `PathBuf::components()`.

- **split_input(input) -> (parent, leaf)**:
  - If `input` is empty → `("", "")`.
  - If `input.ends_with('/')` → `(input, "")` — the parent is the full input string including its trailing `/`; the leaf is empty.
  - Else → split at last `/`: parent is everything up to and including the last `/` (or empty string if no `/`), leaf is the trailing segment.

- **Symlinks**: followed by default (via `fs::metadata`, which dereferences). Broken symlinks are silently skipped in the entries list.

## Error handling

All errors are non-fatal — they set `state.overlay.new_session.error` and leave the overlay open. The user can correct and try again.

| Trigger | error text |
|---|---|
| `read_dir(parent)` fails because parent doesn't exist | `not found` |
| `read_dir(parent)` fails with permission denied | `permission denied` |
| `read_dir(parent)` fails with anything else | the io::Error's display, truncated to ~40 chars |
| `NewSessionConfirm` with `input` that doesn't resolve to a directory | `not a directory` |
| `tmux::new_session(...)` returns `None` | `tmux failed to create session` |

Any successful mutation clears `error`.

The tmux-failure path is an incidental improvement: today `App::create_new_session` swallows a `None` from `tmux::new_session` silently. From the picker we surface it.

## IO placement

Reducer stays pure (no FS). Following the `ReloadConfig` pattern:

- `Action::OpenNewSessionPicker` is intercepted at `App::dispatch_action` and handled by a new `App::open_new_session_picker()` method. It reads the focused session's dir, calls `read_dir`, populates `NewSessionState`, and assigns it onto `state.overlay.new_session`. The matching reducer arm is empty.
- Mutating actions (`NewSessionInput`, `NewSessionBackspace`, `NewSessionTab`, etc.) go through the reducer. When the effective parent changes, the reducer sets `fx.reread_new_session_entries = true`.
- `App::execute_side_effects` honors that flag: it calls `read_dir` on the current parent and writes the result into `state.overlay.new_session.entries`. On error it sets `error` and clears `entries`.
- `NewSessionConfirm` reducer arm validates `input` resolves to a directory (via `fs::metadata`) — wait, that's IO. So this one ALSO goes through `App::dispatch_action`: a new `App::confirm_new_session()` method does the validation, sets `fx.create_session` on success, sets the `error` field on failure. Reducer arm is empty.

To summarize what each layer touches:

| Layer | Actions handled | FS access |
|---|---|---|
| `App::dispatch_action` interceptor | OpenNewSessionPicker, NewSessionConfirm | yes |
| reducer | everything else (Input, Backspace, Tab, cursor moves, ↑↓, Toggle, Clear, etc.) | no |
| `App::execute_side_effects` | follows `reread_new_session_entries` flag | yes |

## Rendering

Add `NewSessionView<'a>` in `ui/mod.rs` alongside `ExcludeEditorView`. `app/render.rs` builds it from `OverlayState.new_session` (lifetime-borrowing the strings). A new `ui::draw_new_session(frame, area, view, theme)` function draws the overlay, mirroring `draw_exclude_editor` structure: outer block, two-row inner split (input + entries), optional error row, footer help row.

The renderer reads the precomputed `filtered: Vec<usize>` from state and iterates `entries[filtered[i]]`. No filtering logic lives in the view layer — the reducer is the single source of truth for what's visible. This keeps `selected` honest (always in `0..filtered.len()`) and makes ↑/↓ predictable.

## Testing

**Pure-function unit tests** (no FS):

- `split_input` — empty, leaf-only, parent-only, deeply nested, trailing-slash variants
- `filter_entries` — case-insensitive prefix; dotfile visible iff leaf starts with `.`
- `smart_backspace` — at-end-of-trailing-slash (up one level), mid-input (char delete), empty input (no-op), input is exactly `/` (no-op — guarded by `input.len() > 1`)
- `tab_complete` — replaces leaf, appends `/`, moves cursor to end
- `expand_path` — `~` expansion, `..` normalization, redundant `/` normalization, bare-relative resolves against `$HOME`

**Reducer tests** (in `tests/unit/app/action/reduce.rs`):

- `NewSessionInput(ch)` inserts at cursor and advances cursor.
- Char input crossing a `/` boundary sets `fx.reread_new_session_entries = true`.
- Char input within the same leaf does NOT set the reread flag but does rebuild `filtered`.
- ↑/↓ stays within bounds for both empty and non-empty `filtered`.
- Typing `.` as the first char of the leaf surfaces dotfile entries in `filtered`; backspacing the `.` removes them.

**FS integration tests** (with a tempdir):

- Create a tree like `tmp/a/{src,tests,target}/`; open picker pointed at `tmp/a/`; Tab onto `src/`; Confirm; assert `fx.create_session == Some(tmp/a/src)`.
- `chmod 000` on a subdir; navigate into it; assert `error == Some("permission denied")` and overlay stays open.
- Confirm with input pointing at a file (not a dir); assert `error == Some("not a directory")`.

No mocking of the filesystem — tempdir is reliable and fast. Mirrors the precedent that `tmux::CommandRunner` exists for subprocess but no equivalent abstraction exists for `fs` in this codebase.

## Out-of-scope items surfaced during design

- The `~/claude` hardcode in `app/lifecycle.rs:28` is the duplicate flagged in the redundancy audit. This spec only removes the hardcode in `dispatch.rs:314` (replaced by the picker); the CLI path stays as-is. A separate refactor PR can unify them.
- A "show files too, but only dirs are selectable" mode is possible but adds layout decisions (icons? colors? toggle?). Defer.

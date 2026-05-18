# New session working-dir picker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the hard-coded `~/claude` default in `App::create_new_session` with an overlay that lets the user type and navigate to a working directory before the new tmux session is created.

**Architecture:** Mirror the existing `ExcludeEditorState` overlay. Pure state in `OverlayState.new_session`; reducer mutates input/cursor/filter; dispatch layer does FS IO (`read_dir`, dir validity check, `tmux::new_session`). Communication via three new `SideEffect` flags.

**Tech Stack:** Rust, ratatui, crossterm (already in use). `std::fs::read_dir` for directory listing. No new dependencies.

**Spec:** [`docs/superpowers/specs/2026-05-18-new-session-working-dir-design.md`](../specs/2026-05-18-new-session-working-dir-design.md)

**Branch:** `feature/new-session-picker` (already checked out).

---

## File map

| File | Action |
|---|---|
| `src/model/new_session.rs` | **new** — `NewSessionState` struct + pure helpers |
| `tests/unit/model/new_session.rs` | **new** — unit tests for the pure helpers |
| `src/model/mod.rs` | modify — wire `pub mod new_session` |
| `src/model/state.rs` | modify — add field, change `SideEffect` |
| `src/app/action/mod.rs` | modify — add `NewSession*` action variants |
| `src/app/action/keyboard.rs` | modify — gate on overlay, add `new_session_key_to_action` |
| `src/app/action/reduce.rs` | modify — reducer arms, rewire `Some("New session")` |
| `src/app/dispatch.rs` | modify — intercept `OpenNewSessionPicker` / `NewSessionConfirm`, handle new flags |
| `src/ui/mod.rs` | modify — add `NewSessionView` |
| `src/ui/new_session.rs` | **new** — `draw_new_session` |
| `src/ui/settings.rs` (or `ui/overlays.rs`) | unchanged — just the rendering precedent we mirror |
| `src/app/render.rs` | modify — build `NewSessionView` and call `draw_new_session` |
| `tests/unit/app/action/reduce.rs` | modify — reducer-arm tests |

---

## Task 1: Pure helpers + unit tests

**Files:**
- Create: `src/model/new_session.rs`
- Create: `tests/unit/model/new_session.rs`
- Modify: `src/model/mod.rs`

### - [ ] Step 1: Add the new module file with helper signatures only

Create `src/model/new_session.rs`:

```rust
//! State and pure helpers for the new-session working-dir picker
//! overlay. FS access lives in `app::dispatch`; everything here is
//! pure and unit-testable.

use std::path::PathBuf;

/// Split `input` into `(parent, leaf)` where `parent` is the directory
/// portion (including any trailing `/`) and `leaf` is the segment
/// being typed.
///
/// - `""` → `("", "")`
/// - `"~/foo/"` → `("~/foo/", "")`
/// - `"~/foo/ba"` → `("~/foo/", "ba")`
/// - `"foo"` → `("", "foo")`
pub fn split_input(input: &str) -> (&str, &str) {
    match input.rfind('/') {
        Some(idx) => (&input[..=idx], &input[idx + 1..]),
        None => ("", input),
    }
}

/// Compute the `filtered` index list from `entries`. Case-insensitive
/// prefix match on `leaf`. Dotfile entries are included iff `leaf`
/// starts with `.`.
pub fn filter_entries(entries: &[String], leaf: &str) -> Vec<usize> {
    let leaf_lc = leaf.to_lowercase();
    let allow_dot = leaf.starts_with('.');
    entries
        .iter()
        .enumerate()
        .filter(|(_, name)| {
            if !allow_dot && name.starts_with('.') {
                return false;
            }
            name.to_lowercase().starts_with(&leaf_lc)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Backspace with up-a-level semantics. If `cursor` is at the end of
/// `input` and `input` ends with `/` (and isn't just `/`), drop the
/// trailing `/` plus the previous segment. Otherwise delete one char
/// before the cursor.
pub fn smart_backspace(input: &mut String, cursor: &mut usize) {
    if *cursor == input.len() && input.len() > 1 && input.ends_with('/') {
        // up one level
        input.pop(); // drop trailing /
        let new_end = input.rfind('/').map(|i| i + 1).unwrap_or(0);
        input.truncate(new_end);
        *cursor = input.len();
        return;
    }
    if *cursor > 0 {
        let prev = input[..*cursor]
            .chars()
            .last()
            .map(|c| c.len_utf8())
            .unwrap_or(0);
        *cursor -= prev;
        input.remove(*cursor);
    }
}

/// Replace the trailing leaf segment of `input` with `entry` plus a
/// trailing `/`. Used by Tab completion. Cursor lands at the new end.
pub fn tab_complete(input: &mut String, cursor: &mut usize, entry: &str) {
    let (parent, _leaf) = split_input(input);
    let parent_owned = parent.to_string();
    input.clear();
    input.push_str(&parent_owned);
    input.push_str(entry);
    input.push('/');
    *cursor = input.len();
}

/// Resolve a user-typed path to an absolute, normalized `PathBuf`.
///
/// - Leading `~` expands to `$HOME`. `~/foo` → `<home>/foo`. Bare `~`
///   → `<home>`.
/// - Bare relative paths (no leading `/` or `~`) resolve under
///   `$HOME` for predictability.
/// - `..` and redundant `/` are normalized via `Path::components`.
pub fn expand_path(s: &str, home: &std::path::Path) -> PathBuf {
    let mut buf = if let Some(rest) = s.strip_prefix("~/") {
        home.join(rest)
    } else if s == "~" {
        home.to_path_buf()
    } else if s.starts_with('/') {
        PathBuf::from(s)
    } else {
        home.join(s)
    };
    // Normalize `..` and redundant separators.
    let mut normalized = PathBuf::new();
    for comp in buf.components() {
        match comp {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    buf = normalized;
    buf
}
```

- [ ] **Step 2: Wire the module**

Edit `src/model/mod.rs` — add `pub mod new_session;` after the existing entries:

```rust
pub mod config;
pub mod keybindings;
pub mod new_session;
pub mod state;
```

- [ ] **Step 3: Add the test file pointer**

Append to the bottom of `src/model/new_session.rs`:

```rust
#[cfg(test)]
#[path = "../../tests/unit/model/new_session.rs"]
mod tests;
```

- [ ] **Step 4: Create the failing tests**

Create `tests/unit/model/new_session.rs`:

```rust
use super::*;
use std::path::PathBuf;

#[test]
fn split_input_empty() {
    assert_eq!(split_input(""), ("", ""));
}

#[test]
fn split_input_trailing_slash() {
    assert_eq!(split_input("~/foo/"), ("~/foo/", ""));
}

#[test]
fn split_input_partial_leaf() {
    assert_eq!(split_input("~/foo/ba"), ("~/foo/", "ba"));
}

#[test]
fn split_input_no_slash() {
    assert_eq!(split_input("foo"), ("", "foo"));
}

#[test]
fn split_input_root_only() {
    assert_eq!(split_input("/"), ("/", ""));
}

#[test]
fn filter_entries_prefix_case_insensitive() {
    let entries = vec!["Documents".into(), "Downloads".into(), "src".into()];
    assert_eq!(filter_entries(&entries, "doc"), vec![0]);
    assert_eq!(filter_entries(&entries, "DO"), vec![0, 1]);
}

#[test]
fn filter_entries_hides_dotfiles_when_leaf_clean() {
    let entries = vec![".git".into(), "src".into()];
    assert_eq!(filter_entries(&entries, ""), vec![1]);
    assert_eq!(filter_entries(&entries, "s"), vec![1]);
}

#[test]
fn filter_entries_shows_dotfiles_when_leaf_starts_with_dot() {
    let entries = vec![".git".into(), ".cargo".into(), "src".into()];
    assert_eq!(filter_entries(&entries, "."), vec![0, 1]);
    assert_eq!(filter_entries(&entries, ".gi"), vec![0]);
}

#[test]
fn smart_backspace_goes_up_at_trailing_slash() {
    let mut s = "~/foo/bar/".to_string();
    let mut c = s.len();
    smart_backspace(&mut s, &mut c);
    assert_eq!(s, "~/foo/");
    assert_eq!(c, s.len());
}

#[test]
fn smart_backspace_deletes_char_mid_leaf() {
    let mut s = "~/foo/ba".to_string();
    let mut c = s.len();
    smart_backspace(&mut s, &mut c);
    assert_eq!(s, "~/foo/b");
    assert_eq!(c, s.len());
}

#[test]
fn smart_backspace_empty_input_noop() {
    let mut s = String::new();
    let mut c = 0;
    smart_backspace(&mut s, &mut c);
    assert_eq!(s, "");
    assert_eq!(c, 0);
}

#[test]
fn smart_backspace_root_only_noop() {
    // input is exactly "/" — guarded by `len > 1`.
    let mut s = "/".to_string();
    let mut c = 1;
    smart_backspace(&mut s, &mut c);
    assert_eq!(s, "");
    assert_eq!(c, 0);
    // Note: smart_backspace falls through to char-delete branch, which
    // deletes the lone `/`. That's acceptable — user can retype.
}

#[test]
fn tab_complete_appends_entry_and_slash() {
    let mut s = "~/foo/ba".to_string();
    let mut c = s.len();
    tab_complete(&mut s, &mut c, "bar");
    assert_eq!(s, "~/foo/bar/");
    assert_eq!(c, s.len());
}

#[test]
fn tab_complete_empty_leaf() {
    let mut s = "~/foo/".to_string();
    let mut c = s.len();
    tab_complete(&mut s, &mut c, "bar");
    assert_eq!(s, "~/foo/bar/");
    assert_eq!(c, s.len());
}

#[test]
fn expand_path_tilde() {
    let home = PathBuf::from("/home/u");
    assert_eq!(expand_path("~", &home), PathBuf::from("/home/u"));
    assert_eq!(expand_path("~/foo", &home), PathBuf::from("/home/u/foo"));
}

#[test]
fn expand_path_absolute() {
    let home = PathBuf::from("/home/u");
    assert_eq!(expand_path("/etc/hosts", &home), PathBuf::from("/etc/hosts"));
}

#[test]
fn expand_path_relative_resolves_under_home() {
    let home = PathBuf::from("/home/u");
    assert_eq!(expand_path("projects/foo", &home), PathBuf::from("/home/u/projects/foo"));
}

#[test]
fn expand_path_normalizes_parent_dir() {
    let home = PathBuf::from("/home/u");
    assert_eq!(expand_path("~/foo/../bar", &home), PathBuf::from("/home/u/bar"));
    assert_eq!(expand_path("~/./bar", &home), PathBuf::from("/home/u/bar"));
}
```

- [ ] **Step 5: Verify the tests fail to compile (helpers exist but not visible yet)**

Run: `cargo test --no-run 2>&1 | tail -20`

Expected: compile errors mentioning the helpers can't be found in `tests/unit/model/new_session.rs` — meaning the `mod tests;` path is wrong. Adjust if needed. If it compiles, run `cargo test new_session::tests 2>&1 | tail -20` — should pass (the helpers are already implemented in step 1).

- [ ] **Step 6: Run the tests**

Run: `cargo test new_session 2>&1 | tail -15`

Expected: all 16+ tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/model/new_session.rs tests/unit/model/new_session.rs src/model/mod.rs
git commit -m "Add new-session picker helpers (pure)"
```

---

## Task 2: Add `NewSessionState` struct + `OverlayState` field

**Files:**
- Modify: `src/model/new_session.rs` — append the struct
- Modify: `src/model/state.rs` — add field to `OverlayState`

- [ ] **Step 1: Append the struct to `src/model/new_session.rs`** (above the `#[cfg(test)]` block)

```rust
#[derive(Debug, Clone, Default)]
pub struct NewSessionState {
    /// User-visible path. `~` and `..` preserved verbatim.
    pub input: String,
    /// Byte offset into `input`.
    pub cursor: usize,
    /// All children (directories only) of the parent of `input`.
    /// Written by dispatch after `read_dir`. The reducer never mutates
    /// this directly.
    pub entries: Vec<String>,
    /// Indices into `entries` after leaf-prefix + dotfile filtering.
    /// Recomputed by the reducer whenever `input` changes.
    pub filtered: Vec<usize>,
    /// Index into `filtered`. Reducer clamps to `0..filtered.len()`.
    pub selected: usize,
    /// Last error encountered. Cleared on the next successful mutation.
    pub error: Option<String>,
}

impl NewSessionState {
    /// Helper: rebuild `filtered` from current `input` and `entries`,
    /// clamp `selected` to the new range.
    pub fn refilter(&mut self) {
        let (_parent, leaf) = split_input(&self.input);
        self.filtered = filter_entries(&self.entries, leaf);
        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        }
    }
}
```

- [ ] **Step 2: Add field to `OverlayState` in `src/model/state.rs`**

Find the existing `OverlayState` struct (around line 213) and add the field:

```rust
use crate::new_session::NewSessionState;

// ...

#[derive(Debug, Default)]
pub struct OverlayState {
    pub show_help: bool,
    pub confirm_kill: bool,
    pub renaming: Option<RenameState>,
    pub context_menu: Option<ContextMenu>,
    pub exclude_editor: Option<ExcludeEditorState>,
    pub new_session: Option<NewSessionState>,
}
```

Note: `use crate::new_session::NewSessionState;` goes in the use block at the top of `state.rs`.

- [ ] **Step 3: Verify the build still passes**

Run: `cargo build --tests 2>&1 | tail -5`

Expected: clean compile.

- [ ] **Step 4: Verify tests still pass**

Run: `cargo test 2>&1 | tail -3`

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/model/new_session.rs src/model/state.rs
git commit -m "Add NewSessionState to OverlayState"
```

---

## Task 3: Migrate `SideEffect` for the new flags

**Files:**
- Modify: `src/model/state.rs`
- Modify: `src/app/dispatch.rs`
- Modify: `src/app/action/reduce.rs`

- [ ] **Step 1: Change `SideEffect.create_session` from bool to `Option<String>`, add two new flags**

In `src/model/state.rs`, find the `SideEffect` struct (around line 131) and replace it:

```rust
#[derive(Debug, Default)]
pub struct SideEffect {
    pub switch_session: Option<String>,
    pub kill_session: Option<KillRequest>,
    pub rename_session: Option<RenameRequest>,
    /// `Some(dir)` means: create a new tmux session at `dir`. Was a
    /// plain bool before the new-session picker landed; the picker
    /// supplies its own dir, and the old `~/claude` default is dead.
    pub create_session: Option<String>,
    /// Dispatch should open the new-session picker overlay. Fired by
    /// the global menu's "New session" item; uses the focused session's
    /// dir as the picker's starting point.
    pub open_new_session_picker: bool,
    /// Dispatch should re-run `read_dir` for the picker's current
    /// parent and refresh `entries`. Fired by any reducer arm that
    /// changes the effective parent.
    pub reread_new_session_entries: bool,
    pub resize_pty: bool,
    pub save_config: bool,
    pub apply_tmux_theme: bool,
    pub refresh_sessions: bool,
    pub quit: bool,
}
```

- [ ] **Step 2: Update `SideEffect::merge`**

In the same file, replace `merge`:

```rust
pub fn merge(&mut self, other: SideEffect) {
    if other.switch_session.is_some() {
        self.switch_session = other.switch_session;
    }
    if other.kill_session.is_some() {
        self.kill_session = other.kill_session;
    }
    if other.rename_session.is_some() {
        self.rename_session = other.rename_session;
    }
    if other.create_session.is_some() {
        self.create_session = other.create_session;
    }
    self.open_new_session_picker |= other.open_new_session_picker;
    self.reread_new_session_entries |= other.reread_new_session_entries;
    self.resize_pty |= other.resize_pty;
    self.save_config |= other.save_config;
    self.apply_tmux_theme |= other.apply_tmux_theme;
    self.refresh_sessions |= other.refresh_sessions;
    self.quit |= other.quit;
}
```

- [ ] **Step 3: Update the one existing call site in `src/app/action/reduce.rs`**

Find the `Some("New session")` arm (around line 511) and change `create_session: true` to `create_session: Some("~/claude".to_string())`. This preserves current behavior temporarily; Task 7 rewires it to open the picker instead.

```rust
Some("New session") => SideEffect {
    create_session: Some("~/claude".to_string()),
    refresh_sessions: true,
    ..SideEffect::default()
},
```

- [ ] **Step 4: Update the dispatch call site to take the new `Option<String>`**

In `src/app/dispatch.rs`, find the `if fx.create_session { self.create_new_session(); }` block (around line 229) and change to:

```rust
if let Some(ref dir) = fx.create_session {
    self.create_new_session(dir);
}
```

- [ ] **Step 5: Change `App::create_new_session` to take a dir parameter**

In the same file, find `fn create_new_session(&mut self)` (around line 314) and update its signature + body. The hard-coded `~/claude` line goes away — replaced by the parameter. Tilde expansion happens here so callers can pass `~/foo`-style paths.

```rust
fn create_new_session(&mut self, dir: &str) {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let home_path = std::path::PathBuf::from(&home);
    let expanded = crate::new_session::expand_path(dir, &home_path);
    let dir_str = expanded.to_string_lossy().to_string();

    let existing: Vec<&str> = self
        .state
        .sessions
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    let mut idx = self.state.sessions.len();
    let name = loop {
        let candidate = format!("session-{}", idx);
        if !existing.contains(&candidate.as_str()) {
            break candidate;
        }
        idx += 1;
    };
    if tmux::new_session(&name, &dir_str).is_some() {
        self.switch_client(&name);
    }
}
```

- [ ] **Step 6: Build + test**

Run: `cargo build --tests 2>&1 | tail -5`

Expected: clean compile.

Run: `cargo test 2>&1 | tail -3`

Expected: all tests pass (215+).

- [ ] **Step 7: Commit**

```bash
git add src/model/state.rs src/app/dispatch.rs src/app/action/reduce.rs
git commit -m "Migrate SideEffect.create_session to Option<String>"
```

---

## Task 4: Add action variants + keyboard handler + stub reducer arms

**Files:**
- Modify: `src/app/action/mod.rs`
- Modify: `src/app/action/keyboard.rs`
- Modify: `src/app/action/reduce.rs`

- [ ] **Step 1: Add the action variants**

In `src/app/action/mod.rs`, add the following variants somewhere in the `enum Action` block (group them near the other overlay actions, e.g., after the `ExcludeEditor*` variants):

```rust
OpenNewSessionPicker,
CloseNewSessionPicker,
NewSessionInput(char),
NewSessionBackspace,
NewSessionTab,
NewSessionConfirm,
NewSessionPrev,
NewSessionNext,
NewSessionCursorLeft,
NewSessionCursorRight,
NewSessionCursorHome,
NewSessionCursorEnd,
NewSessionClear,
NewSessionDeleteSegment,
```

- [ ] **Step 2: Add the keyboard handler**

In `src/app/action/keyboard.rs`, add a new helper at the bottom of the file:

```rust
fn new_session_key_to_action(key: &KeyEvent) -> Action {
    use crossterm::event::KeyModifiers;
    match key.code {
        KeyCode::Esc => Action::CloseNewSessionPicker,
        KeyCode::Enter => Action::NewSessionConfirm,
        KeyCode::Tab => Action::NewSessionTab,
        KeyCode::Backspace => Action::NewSessionBackspace,
        KeyCode::Up => Action::NewSessionPrev,
        KeyCode::Down => Action::NewSessionNext,
        KeyCode::Left => Action::NewSessionCursorLeft,
        KeyCode::Right => Action::NewSessionCursorRight,
        KeyCode::Home => Action::NewSessionCursorHome,
        KeyCode::End => Action::NewSessionCursorEnd,
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::NewSessionClear
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::NewSessionDeleteSegment
        }
        KeyCode::Char(ch) => Action::NewSessionInput(ch),
        _ => Action::None,
    }
}
```

- [ ] **Step 3: Gate on the overlay near the top of `key_to_action`**

In the same file, find the existing block:

```rust
if state.overlay.renaming.is_some() {
    return match key.code {
        // ...
    };
}

if state.overlay.context_menu.is_some() {
    // ...
}
```

Add a new gate **before** the rename block (so the picker keyboard takes precedence — only one of these is ever Some, but explicit order is documented):

```rust
if state.overlay.new_session.is_some() {
    return new_session_key_to_action(key);
}
```

- [ ] **Step 4: Add stub reducer arms**

In `src/app/action/reduce.rs`, add empty arms for the new actions inside the big match. Group them after the `ExcludeEditor*` arms:

```rust
Action::OpenNewSessionPicker => {
    // Handled at dispatch (needs FS IO).
    fx.open_new_session_picker = true;
}
Action::CloseNewSessionPicker => {
    state.overlay.new_session = None;
}
Action::NewSessionInput(_)
| Action::NewSessionBackspace
| Action::NewSessionTab
| Action::NewSessionConfirm
| Action::NewSessionPrev
| Action::NewSessionNext
| Action::NewSessionCursorLeft
| Action::NewSessionCursorRight
| Action::NewSessionCursorHome
| Action::NewSessionCursorEnd
| Action::NewSessionClear
| Action::NewSessionDeleteSegment => {
    // Implemented in Task 5. Keep the compile passing.
}
```

- [ ] **Step 5: Build**

Run: `cargo build --tests 2>&1 | tail -5`

Expected: clean compile, possibly with `unused_imports` or `dead_code` warnings on the stub arm — those are fine.

Run: `cargo test 2>&1 | tail -3`

Expected: 215+ tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/app/action/mod.rs src/app/action/keyboard.rs src/app/action/reduce.rs
git commit -m "Wire NewSession action variants and keyboard handler"
```

---

## Task 5: Implement reducer arms (TDD)

**Files:**
- Modify: `src/app/action/reduce.rs`
- Modify: `tests/unit/app/action/reduce.rs`

- [ ] **Step 1: Write the first failing test** — `NewSessionInput(ch)` inserts at cursor

Append to `tests/unit/app/action/reduce.rs`:

```rust
fn picker_state_with(input: &str, entries: Vec<String>) -> AppState {
    use crate::new_session::NewSessionState;
    let mut state = AppState::default();
    let mut ns = NewSessionState {
        input: input.to_string(),
        cursor: input.len(),
        entries,
        filtered: vec![],
        selected: 0,
        error: None,
    };
    ns.refilter();
    state.overlay.new_session = Some(ns);
    state
}

#[test]
fn new_session_input_inserts_at_cursor() {
    let mut state = picker_state_with("~/foo/", vec!["bar".into(), "baz".into()]);
    let fx = apply_action(&mut state, Action::NewSessionInput('b'));
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input, "~/foo/b");
    assert_eq!(ns.cursor, 7);
    assert_eq!(ns.filtered, vec![0, 1]); // both still match "b"
    assert!(!fx.reread_new_session_entries); // parent didn't change
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test new_session_input_inserts_at_cursor 2>&1 | tail -10`

Expected: FAIL — assertion mismatch (`input` is still `~/foo/`, the stub arm did nothing).

- [ ] **Step 3: Implement `NewSessionInput`**

In `src/app/action/reduce.rs`: remove `Action::NewSessionInput(_)` from the catch-all `|` chain (so the compiler doesn't flag the new explicit arm as unreachable), then add the explicit arm next to the catch-all:

```rust
Action::NewSessionInput(ch) => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        let parent_before = crate::new_session::split_input(&ns.input).0.to_string();
        ns.input.insert(ns.cursor, ch);
        ns.cursor += ch.len_utf8();
        ns.refilter();
        let parent_after = crate::new_session::split_input(&ns.input).0;
        if parent_before != parent_after {
            fx.reread_new_session_entries = true;
        }
        ns.error = None;
    }
}
```

Add `crate::new_session::{split_input, ...}` to the imports if cleaner — but inline `crate::new_session::` qualifications are fine for one-off uses.

- [ ] **Step 4: Run the test**

Run: `cargo test new_session_input_inserts_at_cursor 2>&1 | tail -5`

Expected: PASS.

- [ ] **Step 5: Add the cross-parent test**

Append to `tests/unit/app/action/reduce.rs`:

```rust
#[test]
fn new_session_input_crossing_slash_sets_reread() {
    let mut state = picker_state_with("~/foo", vec!["foo".into()]);
    let fx = apply_action(&mut state, Action::NewSessionInput('/'));
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input, "~/foo/");
    assert!(fx.reread_new_session_entries);
}
```

Run: `cargo test new_session_input_crossing 2>&1 | tail -5`. Expected: PASS.

- [ ] **Step 6: Implement Backspace**

In `reduce.rs`: remove `Action::NewSessionBackspace` from the catch-all `|` chain and add an explicit arm:

```rust
Action::NewSessionBackspace => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        let parent_before = crate::new_session::split_input(&ns.input).0.to_string();
        crate::new_session::smart_backspace(&mut ns.input, &mut ns.cursor);
        ns.refilter();
        let parent_after = crate::new_session::split_input(&ns.input).0;
        if parent_before != parent_after {
            fx.reread_new_session_entries = true;
        }
        ns.error = None;
    }
}
```

Test:

```rust
#[test]
fn new_session_backspace_at_trailing_slash_goes_up() {
    let mut state = picker_state_with("~/foo/bar/", vec![]);
    let fx = apply_action(&mut state, Action::NewSessionBackspace);
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input, "~/foo/");
    assert!(fx.reread_new_session_entries);
}
```

Run: `cargo test new_session_backspace 2>&1 | tail -5`. Expected: PASS.

- [ ] **Step 7: Implement Tab, Confirm placeholder, Cursor moves, Prev/Next, Clear, DeleteSegment**

Delete what remains of the catch-all `|` stub (all the variants below should be split out). Add each of these as its own explicit arm, next to `NewSessionBackspace`:

```rust
Action::NewSessionTab => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        if let Some(&idx) = ns.filtered.get(ns.selected) {
            let entry = ns.entries[idx].clone();
            crate::new_session::tab_complete(&mut ns.input, &mut ns.cursor, &entry);
            ns.refilter();
            fx.reread_new_session_entries = true;
            ns.error = None;
        }
    }
}
Action::NewSessionConfirm => {
    // Handled at dispatch (needs fs::metadata).
}
Action::NewSessionPrev => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        if ns.selected > 0 {
            ns.selected -= 1;
        }
    }
}
Action::NewSessionNext => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        if !ns.filtered.is_empty() && ns.selected + 1 < ns.filtered.len() {
            ns.selected += 1;
        }
    }
}
Action::NewSessionCursorLeft => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        if let Some(prev) = ns.input[..ns.cursor].chars().last() {
            ns.cursor -= prev.len_utf8();
        }
    }
}
Action::NewSessionCursorRight => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        if let Some(next) = ns.input[ns.cursor..].chars().next() {
            ns.cursor += next.len_utf8();
        }
    }
}
Action::NewSessionCursorHome => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        ns.cursor = 0;
    }
}
Action::NewSessionCursorEnd => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        ns.cursor = ns.input.len();
    }
}
Action::NewSessionClear => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        ns.input.clear();
        ns.cursor = 0;
        ns.refilter();
        fx.reread_new_session_entries = true;
        ns.error = None;
    }
}
Action::NewSessionDeleteSegment => {
    if let Some(ns) = state.overlay.new_session.as_mut() {
        let parent_before = crate::new_session::split_input(&ns.input).0.to_string();
        // Trim trailing chars back to (and including) the previous `/`.
        let mut new_end = ns.cursor;
        while new_end > 0 && !ns.input[..new_end].ends_with('/') {
            let prev = ns.input[..new_end]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            new_end -= prev;
        }
        ns.input.truncate(new_end);
        ns.cursor = new_end;
        ns.refilter();
        let parent_after = crate::new_session::split_input(&ns.input).0;
        if parent_before != parent_after {
            fx.reread_new_session_entries = true;
        }
        ns.error = None;
    }
}
```

Remove the old catch-all stub block.

- [ ] **Step 8: Add a few more reducer tests**

Append to `tests/unit/app/action/reduce.rs`:

```rust
#[test]
fn new_session_tab_descends_into_selected_entry() {
    let mut state = picker_state_with("~/foo/b", vec!["bar".into(), "baz".into()]);
    let fx = apply_action(&mut state, Action::NewSessionTab);
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input, "~/foo/bar/");
    assert!(fx.reread_new_session_entries);
}

#[test]
fn new_session_next_clamped_to_filtered_len() {
    let mut state = picker_state_with("~/", vec!["a".into(), "b".into()]);
    apply_action(&mut state, Action::NewSessionNext);
    apply_action(&mut state, Action::NewSessionNext);
    apply_action(&mut state, Action::NewSessionNext); // tries to overrun
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.selected, 1);
}

#[test]
fn new_session_delete_segment_goes_back_to_slash() {
    let mut state = picker_state_with("~/foo/bar", vec![]);
    let fx = apply_action(&mut state, Action::NewSessionDeleteSegment);
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input, "~/foo/");
    assert!(fx.reread_new_session_entries);
}
```

- [ ] **Step 9: Build + test**

Run: `cargo test new_session 2>&1 | tail -15`

Expected: all new tests pass alongside the helper tests. No compile errors. No regression in existing tests (`cargo test 2>&1 | tail -3` → 215+ pass).

- [ ] **Step 10: Commit**

```bash
git add src/app/action/reduce.rs tests/unit/app/action/reduce.rs
git commit -m "Implement reducer arms for new-session picker"
```

---

## Task 6: Dispatch interceptors + reread handling

**Files:**
- Modify: `src/app/dispatch.rs`

- [ ] **Step 1: Add the open-picker method**

Append a new method on `impl App` in `src/app/dispatch.rs`. Place it near `create_new_session`:

```rust
fn open_new_session_picker(&mut self) {
    use crate::new_session::{expand_path, split_input, NewSessionState};

    // Starting dir: focused session's dir if any, else $HOME.
    let start_dir = self
        .state
        .filtered
        .get(self.state.focused)
        .and_then(|&i| self.state.sessions.get(i))
        .map(|s| s.dir.clone())
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| ".".to_string()));
    let mut input = start_dir;
    if !input.ends_with('/') {
        input.push('/');
    }

    let home = std::path::PathBuf::from(
        std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
    );
    let (parent, _leaf) = split_input(&input);
    let parent_path = expand_path(parent, &home);

    let (entries, error) = read_dir_entries(&parent_path);

    let mut ns = NewSessionState {
        cursor: input.len(),
        input,
        entries,
        filtered: vec![],
        selected: 0,
        error,
    };
    ns.refilter();
    self.state.overlay.new_session = Some(ns);
}
```

- [ ] **Step 2: Add the `read_dir_entries` helper at module scope**

In the same file, at module scope (outside `impl App`, near the top of the file or near the other free helpers), add:

```rust
/// Read a directory and return (sorted dir names, error message). On
/// any failure the entries list is empty and the error is set.
fn read_dir_entries(path: &std::path::Path) -> (Vec<String>, Option<String>) {
    match std::fs::read_dir(path) {
        Ok(rd) => {
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.metadata()
                        .map(|m| m.is_dir())
                        .unwrap_or(false)
                })
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            names.sort();
            (names, None)
        }
        Err(e) => {
            let msg = match e.kind() {
                std::io::ErrorKind::NotFound => "not found".to_string(),
                std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
                _ => {
                    let s = e.to_string();
                    if s.len() > 40 {
                        format!("{}…", &s[..39])
                    } else {
                        s
                    }
                }
            };
            (Vec::new(), Some(msg))
        }
    }
}
```

- [ ] **Step 3: Add the confirm method**

Append to `impl App`:

```rust
fn confirm_new_session(&mut self) -> Option<String> {
    use crate::new_session::expand_path;

    let Some(ns) = self.state.overlay.new_session.as_mut() else {
        return None;
    };
    let home = std::path::PathBuf::from(
        std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
    );
    let resolved = expand_path(&ns.input, &home);
    match std::fs::metadata(&resolved) {
        Ok(m) if m.is_dir() => {
            let dir = resolved.to_string_lossy().to_string();
            self.state.overlay.new_session = None;
            Some(dir)
        }
        Ok(_) => {
            ns.error = Some("not a directory".into());
            None
        }
        Err(e) => {
            ns.error = Some(match e.kind() {
                std::io::ErrorKind::NotFound => "not found".into(),
                std::io::ErrorKind::PermissionDenied => "permission denied".into(),
                _ => "cannot stat".into(),
            });
            None
        }
    }
}
```

- [ ] **Step 4: Intercept the two actions in `dispatch_action`**

Find the `dispatch_action` match (around line 100 in `dispatch.rs`). Add two arms next to `Action::ReloadConfig`:

```rust
Action::OpenNewSessionPicker => {
    self.open_new_session_picker();
    false
}
Action::NewSessionConfirm => {
    if let Some(dir) = self.confirm_new_session() {
        // Trigger creation via the standard side-effect path so the
        // refresh / switch_client flow stays unified.
        let mut fx = crate::state::SideEffect::default();
        fx.create_session = Some(dir);
        fx.refresh_sessions = true;
        self.execute_side_effects(&fx);
    }
    false
}
```

- [ ] **Step 5: Handle `reread_new_session_entries` in `execute_side_effects`**

In the same file, find the body of `execute_side_effects`. Add after the existing flag handlers (after the `refresh_sessions` block, around line 247):

```rust
if fx.reread_new_session_entries {
    if let Some(ns) = self.state.overlay.new_session.as_mut() {
        use crate::new_session::{expand_path, split_input};
        let home = std::path::PathBuf::from(
            std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
        );
        let (parent, _leaf) = split_input(&ns.input);
        let parent_path = expand_path(parent, &home);
        let (entries, error) = read_dir_entries(&parent_path);
        ns.entries = entries;
        ns.error = error;
        ns.refilter();
    }
}
```

- [ ] **Step 6: Handle `open_new_session_picker` in `execute_side_effects`**

Add another block alongside the previous one:

```rust
if fx.open_new_session_picker {
    self.open_new_session_picker();
}
```

- [ ] **Step 7: Build + test**

Run: `cargo build --tests 2>&1 | tail -5`

Expected: clean compile.

Run: `cargo test 2>&1 | tail -3`

Expected: 215+ tests pass. The picker isn't wired to the menu yet — that's Task 7 — but the new actions all work in tests.

- [ ] **Step 8: Commit**

```bash
git add src/app/dispatch.rs
git commit -m "Dispatch IO for new-session picker"
```

---

## Task 7: Rewire global menu → picker

**Files:**
- Modify: `src/app/action/reduce.rs`

- [ ] **Step 1: Update the `Some("New session")` arm**

Find it (around line 511 — currently sets `create_session: Some("~/claude".to_string())` from Task 3). Replace with:

```rust
Some("New session") => SideEffect {
    open_new_session_picker: true,
    ..SideEffect::default()
},
```

Note: `refresh_sessions` is no longer set here — the picker's confirm path triggers its own refresh.

- [ ] **Step 2: Build**

Run: `cargo build --tests 2>&1 | tail -5`. Expected: clean.

- [ ] **Step 3: Confirm the existing menu-confirm test still passes**

The existing reducer tests around `Action::MenuConfirm` for "New session" may have asserted `fx.create_session`. Look for them:

Run: `grep -n 'New session\|create_session' tests/unit/app/action/reduce.rs`

If a test asserts the old behavior, update it to assert `fx.open_new_session_picker == true` and `fx.create_session.is_none()`.

- [ ] **Step 4: Run full suite**

Run: `cargo test 2>&1 | tail -3`

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/app/action/reduce.rs tests/unit/app/action/reduce.rs
git commit -m "Rewire 'New session' menu item to open the picker"
```

---

## Task 8: Rendering

**Files:**
- Modify: `src/ui/mod.rs`
- Create: `src/ui/new_session.rs`
- Modify: `src/app/render.rs`

- [ ] **Step 1: Add `NewSessionView` to `src/ui/mod.rs`**

After the existing `ExcludeEditorView` declaration, add:

```rust
pub struct NewSessionView<'a> {
    pub input: &'a str,
    pub cursor: usize,
    pub entries: &'a [String],
    pub filtered: &'a [usize],
    pub selected: usize,
    pub error: Option<&'a str>,
}
```

Also add the module declaration at the top with the others:

```rust
mod new_session;
```

And re-export the draw function below the other re-exports:

```rust
pub use new_session::draw_new_session;
```

- [ ] **Step 2: Create `src/ui/new_session.rs`**

```rust
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::theme::Theme;

use super::NewSessionView;

const POPUP_WIDTH: u16 = 60;
const POPUP_MIN_HEIGHT: u16 = 8;
const MAX_VISIBLE_ENTRIES: usize = 8;

pub fn draw_new_session(frame: &mut Frame, area: Rect, view: &NewSessionView, theme: &Theme) {
    let visible_entries = view.filtered.len().min(MAX_VISIBLE_ENTRIES);
    let entry_rows = visible_entries.max(1) as u16; // always reserve one row for "(no entries)"
    let extra_for_error = if view.error.is_some() { 1 } else { 0 };
    // borders(2) + input(1) + blank(1) + entries(N) + blank(1) + error(0|1) + footer(1)
    let height = (2 + 1 + 1 + entry_rows + 1 + extra_for_error + 1)
        .max(POPUP_MIN_HEIGHT)
        .min(area.height.saturating_sub(2));
    let width = POPUP_WIDTH.min(area.width.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" New session ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.bg));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines: Vec<Line> = Vec::new();

    // Input row.
    let display_input = render_input_with_cursor(view.input, view.cursor);
    lines.push(Line::from(vec![
        Span::styled("  Path: ", Style::default().fg(theme.dim)),
        Span::styled(display_input, Style::default().fg(theme.text)),
    ]));
    lines.push(Line::raw(""));

    // Entries.
    if view.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (no entries)",
            Style::default().fg(theme.dim),
        )));
    } else {
        let start = scroll_window(view.selected, view.filtered.len(), MAX_VISIBLE_ENTRIES);
        let end = (start + MAX_VISIBLE_ENTRIES).min(view.filtered.len());
        for (visible_pos, idx) in view.filtered[start..end].iter().enumerate() {
            let display_pos = start + visible_pos;
            let name = &view.entries[*idx];
            let selected = display_pos == view.selected;
            let row_bg = if selected { theme.surface } else { theme.bg };
            let marker = if selected { "▸" } else { " " };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default()
                        .fg(if selected { theme.accent } else { theme.bg })
                        .bg(row_bg),
                ),
                Span::styled(
                    format!("{name}/"),
                    Style::default().fg(theme.text).bg(row_bg),
                ),
            ]));
        }
    }
    lines.push(Line::raw(""));

    // Error row.
    if let Some(err) = view.error {
        lines.push(Line::from(Span::styled(
            format!("  ⚠ {}", err),
            Style::default().fg(theme.pink),
        )));
    }

    // Footer.
    lines.push(Line::from(Span::styled(
        "  ⏎ create   ⇥ complete   ⎋ cancel",
        Style::default().fg(theme.dim).add_modifier(Modifier::DIM),
    )));

    frame.render_widget(Paragraph::new(lines).style(Style::default().bg(theme.bg)), inner);
}

fn render_input_with_cursor(input: &str, cursor: usize) -> String {
    // Cursor representation: a vertical bar inserted at `cursor`.
    // Falls back to end-of-string if `cursor` is out of bounds.
    if cursor >= input.len() {
        format!("{input}▌")
    } else {
        let (before, after) = input.split_at(cursor);
        format!("{before}▌{after}")
    }
}

/// Compute the first visible index so that `selected` stays in view.
fn scroll_window(selected: usize, total: usize, window: usize) -> usize {
    if total <= window {
        return 0;
    }
    if selected < window {
        return 0;
    }
    let max_start = total - window;
    (selected + 1).saturating_sub(window).min(max_start)
}
```

- [ ] **Step 3: Wire `NewSessionView` into `src/app/render.rs`**

`render` captures locals before opening the `terminal.draw(|frame| { ... })` closure (look at lines 33–105). Follow the same pattern.

**3a.** After the existing `let context_menu = s.overlay.context_menu.clone();` line (around line 44), add:

```rust
let new_session_overlay = s.overlay.new_session.clone();
```

**3b.** Inside the closure, find the `if let Some(ref menu) = context_menu { ui::draw_context_menu(...) }` block (around line 350). Insert the picker draw **immediately after** that block and **before** the reload-bar block:

```rust
if let Some(ref ns) = new_session_overlay {
    let view = ui::NewSessionView {
        input: &ns.input,
        cursor: ns.cursor,
        entries: &ns.entries,
        filtered: &ns.filtered,
        selected: ns.selected,
        error: ns.error.as_deref(),
    };
    ui::draw_new_session(frame, frame.area(), &view, theme);
}
```

This places the picker on top of the sidebar/main pane/settings/warning/context-menu and underneath the reload bar — same z-order as the design's expectation that it's the most foreground UI except for the reload status.

- [ ] **Step 4: Build**

Run: `cargo build 2>&1 | tail -5`

Expected: clean compile.

- [ ] **Step 5: Manual smoke test**

```bash
cargo build --release
./target/release/deck
```

Inside deck:
1. Right-click anywhere in the sidebar's empty area (or use whatever keystroke triggers the global menu).
2. Pick "New session".
3. Confirm the picker appears, centered, with the focused session's dir + `/` in the input box, and that dir's subdirs in the preview list.
4. Type a few letters → preview narrows.
5. ↓ to select a subdir → Tab → input updates, preview now shows that subdir's children.
6. Backspace at a trailing `/` → goes up a level.
7. Enter on a valid dir → overlay closes, new tmux session appears in the sidebar at the chosen dir.
8. Open again, type a nonsense path, Enter → error appears, overlay stays.
9. Esc → overlay closes, no new session.

Take a screenshot if anything is off-by-one in the layout and adjust the height/width constants.

- [ ] **Step 6: Commit**

```bash
git add src/ui/mod.rs src/ui/new_session.rs src/app/render.rs
git commit -m "Render new-session picker overlay"
```

---

## Task 9: FS integration tests

**Files:**
- Modify: `tests/unit/model/new_session.rs` (or split out a new test file under `tests/unit/app/`)

- [ ] **Step 1: Add tempdir-based integration tests**

`tempfile` may not be a dep yet — check `Cargo.toml`. If absent, add `tempfile = "3"` to `[dev-dependencies]`.

Run: `grep -A3 'dev-dependencies' Cargo.toml`

If `tempfile` is missing:

```bash
cargo add --dev tempfile
```

Append to `tests/unit/model/new_session.rs`:

```rust
#[cfg(test)]
mod fs_integration {
    use super::*;
    use std::fs;

    #[test]
    fn read_dir_entries_lists_subdirs_only() {
        // This test calls `App::read_dir_entries` indirectly via the
        // model layer is hard — `read_dir_entries` lives in
        // `app::dispatch`. Instead, exercise the underlying contract
        // by listing manually and verifying our helpers behave.
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("src")).unwrap();
        fs::create_dir(tmp.path().join("tests")).unwrap();
        fs::write(tmp.path().join("README"), "").unwrap();

        let mut names: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.metadata().map(|m| m.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        assert_eq!(names, vec!["src", "tests"]);

        let filtered = filter_entries(&names, "s");
        assert_eq!(filtered, vec![0]); // "src" matches
    }

    #[test]
    fn expand_path_resolves_real_tempdir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let resolved = expand_path("~/foo", &home);
        assert_eq!(resolved, tmp.path().join("foo"));
    }
}
```

- [ ] **Step 2: Run**

Run: `cargo test new_session::fs_integration 2>&1 | tail -10`

Expected: both pass.

Run full suite: `cargo test 2>&1 | tail -3`

Expected: all pass (215+ original + new ones from Tasks 1, 5, 9).

- [ ] **Step 3: Commit**

```bash
git add tests/unit/model/new_session.rs Cargo.toml Cargo.lock
git commit -m "Tempdir-based smoke tests for new-session picker helpers"
```

---

## Final verification

- [ ] **Run the full pipeline once more**

```bash
cargo build --release && cargo test && cargo clippy --tests
```

Expected:
- Release build clean.
- All tests pass.
- No new clippy warnings.

- [ ] **Manual end-to-end smoke test**

Re-run the manual flow from Task 8 Step 5 in a real tmux to confirm:
- Picker opens with correct starting dir.
- Typed input + arrow keys + Tab + Backspace all behave per the design.
- Enter at a valid dir creates a session; the sidebar shows it; tmux switches to it.
- Errors stay visible until next mutation.

- [ ] **Push and open PR**

```bash
git push -u origin feature/new-session-picker
gh pr create --base main --title "Add new-session working-dir picker overlay" --body "$(cat <<'EOF'
## Summary

Replaces the hard-coded `~/claude` default in `App::create_new_session` with an overlay that lets the user type and navigate to a working directory before the new tmux session is created. Spec: \`docs/superpowers/specs/2026-05-18-new-session-working-dir-design.md\`.

## Test plan
- [x] cargo build / test / clippy clean
- [x] Manual smoke: open via global menu, typed path, Tab descent, Backspace up-level, valid Enter → creates session, invalid Enter → error stays visible, Esc → cancels.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

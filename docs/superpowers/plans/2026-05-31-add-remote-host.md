# Refresh-Button Color + Add Remote Host — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** (1) The divider reconnect glyph `[⟳]` uses the per-host accent color when Connected (warnings kept for Connecting/Unreachable). (2) A global-menu "Add Remote Host" item opens a picker over `~/.ssh/config` hosts (plus free-text) that adds the chosen host to `config.remotes`, persists, and onboards it live.

**Architecture:** Part 1 is a one-line color change + test. Part 2 mirrors the existing new-session picker: a self-contained `model/add_remote.rs` (state + pure filter/choice helpers) and `ui/add_remote.rs` (popup render), wired through new `Action`s, reducer arms, `SideEffect` fields, and dispatch handling that reuses the existing `onboard_remote_host` path.

**Tech Stack:** Rust, ratatui + ratatui_textarea, crossterm, the system `ssh` client.

**Design doc:** `docs/superpowers/specs/2026-05-31-add-remote-host-design.md`

**Notes for the implementer:**
- `deck` is a **binary crate with no lib target** → run tests with `cargo test <filter>`, NOT `cargo test --lib`.
- The repo does **not** use rustfmt (its style is hand-formatted; `cargo fmt --check` is noisy across untouched files). Do NOT run `cargo fmt`. Match the surrounding style.
- Lint gate is `cargo clippy -- -D warnings` (checks the binary target). New `pub` items with no caller yet trigger `dead_code`; the plan adds `#[allow(dead_code)]` where noted and removes them in the final task once consumed.
- Stage only explicit paths when committing (the repo has untracked `.cursor/`/`.claude/`); never `git add -A`/`git add .`.
- Crate-root module paths: `crate::config`, `crate::state`, `crate::new_session` resolve via `main.rs`'s `pub(crate) use model::{...}`. Task 3 extends that so `crate::add_remote` resolves.

---

## Task 1: Refresh button defaults to the divider accent (Part 1)

**Files:**
- Modify: `src/ui/sidebar.rs` (`render_group_header`, the `reconnect_fg` match ~line 410)
- Test: `tests/unit/ui/sidebar.rs` (already wired into `sidebar.rs` via `#[cfg(test)] #[path] mod tests;`)

- [ ] **Step 1: Write the failing test**

Append to `tests/unit/ui/sidebar.rs`:

```rust
#[test]
fn reconnect_glyph_color_follows_status() {
    use crate::state::HostStatus;
    let theme = &crate::theme::THEMES[0];
    let accent = theme.teal;
    for (status, expected) in [
        (HostStatus::Connected, theme.teal), // unified with the divider accent
        (HostStatus::Connecting, theme.yellow),
        (HostStatus::Unreachable, theme.pink),
    ] {
        let mut lines = Vec::new();
        super::render_group_header(&mut lines, "@h", accent, status, 40, theme);
        let glyph = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "[\u{27f3}]")
            .expect("reconnect glyph span present");
        assert_eq!(glyph.style.fg, Some(expected), "status {status:?}");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test reconnect_glyph_color_follows_status`
Expected: FAIL on the Connected case (`[⟳]` is currently `theme.green`, not `theme.teal`).

- [ ] **Step 3: Make the change**

In `src/ui/sidebar.rs`, in `render_group_header`, change the `reconnect_fg` match arm for `Connected` from `theme.green` to `accent`:

```rust
    let reconnect_fg = match status {
        HostStatus::Connected => accent,
        HostStatus::Connecting => theme.yellow,
        HostStatus::Unreachable => theme.pink,
    };
```

(`accent` is already a parameter of `render_group_header`, used for the label/rule/`[…]`.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test reconnect_glyph_color_follows_status`
Expected: PASS.

- [ ] **Step 5: Build, lint, commit**

Run: `cargo clippy -- -D warnings` (clean) and `cargo test` (all green).
```bash
git add src/ui/sidebar.rs tests/unit/ui/sidebar.rs
git commit -m "feat(sidebar): reconnect glyph uses divider accent when Connected"
```

---

## Task 2: Enumerate `~/.ssh/config` hosts (`infra/ssh.rs`)

**Files:**
- Modify: `src/infra/ssh.rs` (add `parse_config_hosts` + `config_hosts`)
- Test: `tests/unit/infra/ssh.rs` (create if absent; wire via a test module in `ssh.rs`)

- [ ] **Step 1: Check for an existing test module**

Run: `grep -n "mod tests" src/infra/ssh.rs`. If there is no `#[cfg(test)] #[path = "../../tests/unit/infra/ssh.rs"] mod tests;` block at the bottom of `src/infra/ssh.rs`, add one in Step 4. If `tests/unit/infra/ssh.rs` already exists, append to it; otherwise create it.

- [ ] **Step 2: Write the failing test**

Create or append to `tests/unit/infra/ssh.rs`:

```rust
use crate::infra::ssh::parse_config_hosts;

#[test]
fn parses_concrete_hosts_excluding_wildcards() {
    let sample = "\
# work hosts
Host prod-web-1
    HostName 10.0.0.1
    User deploy

Host prod-web-2 staging
    HostName 10.0.0.2

Host *
    ServerAliveInterval 30

Host build-?
    HostName builder

Host !secret
    HostName x
";
    let hosts = parse_config_hosts(sample);
    assert_eq!(hosts, vec!["prod-web-1", "prod-web-2", "staging"]);
}

#[test]
fn dedups_and_preserves_first_seen_order() {
    let sample = "Host a\nHost b\nHost a\n";
    assert_eq!(parse_config_hosts(sample), vec!["a", "b"]);
}

#[test]
fn empty_or_no_hosts_yields_empty() {
    assert!(parse_config_hosts("").is_empty());
    assert!(parse_config_hosts("# comment only\n  IdentityFile ~/x\n").is_empty());
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test parse_config_hosts` (or `cargo test parses_concrete_hosts`)
Expected: FAIL — `parse_config_hosts` not found.

- [ ] **Step 4: Implement**

Add to `src/infra/ssh.rs` (after the existing functions; keep the existing `use` block, add `std::fs` usage inline as shown):

```rust
/// Parse `~/.ssh/config` text into the list of concrete `Host` aliases.
/// Each `Host` line may list several patterns; we keep those without
/// wildcard/negation characters (`*`, `?`, `!`), de-duped, first-seen order.
/// Effective per-host options are irrelevant here — the picker only needs the
/// alias to add to deck (later resolved via `ssh -G`).
#[allow(dead_code)] // caller arrives with the Add Remote Host picker (Task 4)
pub fn parse_config_hosts(content: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let keyword = tokens.next().unwrap_or("");
        if !keyword.eq_ignore_ascii_case("host") {
            continue;
        }
        for pat in tokens {
            if pat.contains('*') || pat.contains('?') || pat.contains('!') {
                continue;
            }
            if !hosts.iter().any(|h| h == pat) {
                hosts.push(pat.to_string());
            }
        }
    }
    hosts
}

/// Read `~/.ssh/config` and return its concrete `Host` aliases. A missing or
/// unreadable file yields an empty list (the picker still accepts typed input).
#[allow(dead_code)] // caller arrives with the Add Remote Host picker (Task 4)
pub fn config_hosts() -> Vec<String> {
    let path = ssh_config_path();
    match std::fs::read_to_string(path) {
        Ok(content) => parse_config_hosts(&content),
        Err(_) => Vec::new(),
    }
}
```

`ssh_config_path()` already exists in this file (private fn returning `~/.ssh/config`).

If Step 1 found no test module, also add at the bottom of `src/infra/ssh.rs`:
```rust
#[cfg(test)]
#[path = "../../tests/unit/infra/ssh.rs"]
mod tests;
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test parse_config_hosts`
Expected: PASS (3 tests).

- [ ] **Step 6: Build, lint, commit**

Run: `cargo clippy -- -D warnings` (clean — the `#[allow(dead_code)]` covers the not-yet-called fns) and `cargo test`.
```bash
git add src/infra/ssh.rs tests/unit/infra/ssh.rs
git commit -m "feat(ssh): enumerate ~/.ssh/config Host aliases"
```

---

## Task 3: Picker state + helpers (`model/add_remote.rs`)

**Files:**
- Create: `src/model/add_remote.rs`
- Create: `tests/unit/model/add_remote.rs`
- Modify: `src/model/mod.rs` (add `pub mod add_remote;`)
- Modify: `src/main.rs` (add `add_remote` to the `pub(crate) use model::{...}` re-export)

- [ ] **Step 1: Write the failing tests**

Create `tests/unit/model/add_remote.rs`:

```rust
use crate::add_remote::{filter_hosts, AddRemoteState};

fn hosts() -> Vec<String> {
    vec!["prod-web-1".into(), "prod-web-2".into(), "staging".into()]
}

#[test]
fn filter_empty_matches_all() {
    assert_eq!(filter_hosts(&hosts(), ""), vec![0, 1, 2]);
    assert_eq!(filter_hosts(&hosts(), "   "), vec![0, 1, 2]);
}

#[test]
fn filter_is_case_insensitive_substring() {
    assert_eq!(filter_hosts(&hosts(), "WEB"), vec![0, 1]);
    assert_eq!(filter_hosts(&hosts(), "stag"), vec![2]);
    assert!(filter_hosts(&hosts(), "nope").is_empty());
}

#[test]
fn new_shows_all_and_refilter_clamps_selected() {
    let mut s = AddRemoteState::new(hosts());
    assert_eq!(s.filtered, vec![0, 1, 2]);
    s.selected = 2;
    // type "stag" → only "staging" remains; selected clamps to 0
    s.input = crate::add_remote::make_textarea("stag");
    s.refilter();
    assert_eq!(s.filtered, vec![2]);
    assert_eq!(s.selected, 0);
}

#[test]
fn chosen_host_prefers_highlighted_then_typed() {
    // highlighted candidate when the list is non-empty
    let mut s = AddRemoteState::new(hosts());
    s.selected = 1;
    assert_eq!(s.chosen_host().as_deref(), Some("prod-web-2"));

    // typed literal when nothing matches the filter
    s.input = crate::add_remote::make_textarea("brand-new-host");
    s.refilter();
    assert!(s.filtered.is_empty());
    assert_eq!(s.chosen_host().as_deref(), Some("brand-new-host"));

    // empty input + empty candidate list → nothing to add
    let mut empty = AddRemoteState::new(vec![]);
    assert_eq!(empty.chosen_host(), None);
    empty.input = crate::add_remote::make_textarea("   ");
    empty.refilter();
    assert_eq!(empty.chosen_host(), None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test add_remote`
Expected: FAIL — `crate::add_remote` does not resolve yet.

- [ ] **Step 3: Create the module**

Create `src/model/add_remote.rs`:

```rust
//! State + pure helpers for the "Add Remote Host" picker. Mirrors the
//! new-session picker's split: this module owns the overlay state and the
//! filtering / choice logic; rendering lives in `ui/add_remote.rs`.

// Consumers (reducer/dispatch/UI) arrive in Tasks 4-5; the final task removes
// this once everything is wired.
#![allow(dead_code)]

use ratatui_textarea::{CursorMove, TextArea};

#[derive(Debug, Clone)]
pub struct AddRemoteState {
    /// Doubles as a live filter over `hosts` and a free-text hostname.
    pub input: TextArea<'static>,
    /// `~/.ssh/config` candidates minus hosts already in config.remotes.
    /// Set when the picker opens; the reducer never refills it.
    pub hosts: Vec<String>,
    /// Indices into `hosts` matching the input (case-insensitive substring).
    pub filtered: Vec<usize>,
    /// Index into `filtered`; clamped to `0..filtered.len()`.
    pub selected: usize,
    /// Last error (empty / already-added). Cleared on the next input edit.
    pub error: Option<String>,
}

impl AddRemoteState {
    /// Open over the given candidate hosts; all visible initially.
    pub fn new(hosts: Vec<String>) -> Self {
        let filtered = (0..hosts.len()).collect();
        Self {
            input: make_textarea(""),
            hosts,
            filtered,
            selected: 0,
            error: None,
        }
    }

    /// First line of the input textarea.
    pub fn input_str(&self) -> &str {
        self.input.lines().first().map(String::as_str).unwrap_or("")
    }

    /// Rebuild `filtered` from the current input; clamp `selected`.
    pub fn refilter(&mut self) {
        self.filtered = filter_hosts(&self.hosts, self.input_str());
        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        }
    }

    /// The host to add on confirm: the highlighted candidate when the filtered
    /// list is non-empty, otherwise the trimmed free-text input. `None` when
    /// there is nothing to add.
    pub fn chosen_host(&self) -> Option<String> {
        if let Some(&idx) = self.filtered.get(self.selected) {
            return self.hosts.get(idx).cloned();
        }
        let typed = self.input_str().trim();
        if typed.is_empty() {
            None
        } else {
            Some(typed.to_string())
        }
    }
}

pub fn make_textarea(s: &str) -> TextArea<'static> {
    let mut ta = TextArea::new(vec![s.to_string()]);
    ta.move_cursor(CursorMove::End);
    ta
}

/// Indices of `hosts` whose name contains `needle` (case-insensitive). An
/// empty/whitespace needle matches everything.
pub fn filter_hosts(hosts: &[String], needle: &str) -> Vec<usize> {
    let needle = needle.trim().to_ascii_lowercase();
    hosts
        .iter()
        .enumerate()
        .filter(|(_, h)| needle.is_empty() || h.to_ascii_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
#[path = "../../tests/unit/model/add_remote.rs"]
mod tests;
```

- [ ] **Step 4: Register the module**

In `src/model/mod.rs`, add (alphabetical, before `config` is fine — match the existing ordering, the file lists `config, keybindings, new_session, state`):
```rust
pub mod add_remote;
```
In `src/main.rs`, add `add_remote` to the existing re-export:
```rust
pub(crate) use model::{add_remote, config, keybindings, new_session, state};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test add_remote`
Expected: PASS (4 tests).

- [ ] **Step 6: Build, lint, commit**

Run: `cargo clippy -- -D warnings` (clean — module-level allow) and `cargo test`.
```bash
git add src/model/add_remote.rs tests/unit/model/add_remote.rs src/model/mod.rs src/main.rs
git commit -m "feat(add-remote): picker state + filter/choice helpers"
```

---

## Task 4: Wire the picker (menu, actions, reducer, dispatch, keys)

This task makes the picker fully functional except rendering. All of it lands
together because adding `Action` variants requires their reducer arms
(exhaustiveness), and the `SideEffect` fields must be both set (reducer) and
consumed (dispatch) to avoid dead-code.

**Files:**
- Modify: `src/model/state.rs` (`GLOBAL_MENU_ITEMS`, `OverlayState`, `SideEffect`)
- Modify: `src/app/action/mod.rs` (new `Action` variants)
- Modify: `src/app/action/reduce.rs` (menu arm + AddRemote arms)
- Modify: `src/app/dispatch.rs` (open picker + onboard on confirm)
- Modify: `src/app/action/keyboard.rs` (route keys when picker open)
- Modify: `src/model/add_remote.rs` + `src/infra/ssh.rs` (remove now-stale allows)

- [ ] **Step 1: Add the menu item + state fields (`src/model/state.rs`)**

In `GLOBAL_MENU_ITEMS`, add `"Add Remote Host"` right after `"New session"`:
```rust
const GLOBAL_MENU_ITEMS: &[&str] = &[
    "New session",
    "Add Remote Host",
    "Toggle layout",
    "Toggle borders",
    "Settings",
    "Quit",
];
```

In `pub struct OverlayState` (which is `#[derive(Debug, Default)]`), add a field after `new_session`:
```rust
    pub add_remote: Option<crate::add_remote::AddRemoteState>,
```
Because `OverlayState` derives `Default`, the new `Option` field defaults to `None` automatically — nothing else to change. (`AddRemoteState` derives `Clone`, so `s.overlay.add_remote.clone()` in Task 5 works.)

In `pub struct SideEffect`, add two fields (after `open_new_session_picker` and near `remove_remote_host`):
```rust
    /// Dispatch should open the Add Remote Host picker (build the candidate
    /// list from ~/.ssh/config minus already-added hosts).
    pub open_add_remote_picker: bool,
    /// A host was just added; dispatch should onboard it (spawn connection)
    /// the same way `reload_config` does for a newly-configured host.
    pub add_remote_host: Option<String>,
```
(`SideEffect` derives `Default`, so new `bool`/`Option` fields default fine.)

- [ ] **Step 2: Add the `Action` variants (`src/app/action/mod.rs`)**

Add before the `None,` variant:
```rust
    AddRemoteInputKey(crossterm::event::KeyEvent),
    AddRemoteNext,
    AddRemotePrev,
    AddRemoteConfirm,
    AddRemoteClose,
```

- [ ] **Step 3: Add the reducer arms (`src/app/action/reduce.rs`)**

In the global-menu match (the `MenuKind::Global =>` block inside `Action::MenuConfirm`), add a case alongside `"New session"`:
```rust
                        Some("Add Remote Host") => SideEffect {
                            open_add_remote_picker: true,
                            ..SideEffect::default()
                        },
```

Add these arms to the main `match action` (place them next to the `NewSession*` arms; before `Action::None => {}`):
```rust
        Action::AddRemoteInputKey(key) => {
            if let Some(ar) = state.overlay.add_remote.as_mut() {
                ar.input.input(key);
                ar.refilter();
                ar.error = None;
            }
        }
        Action::AddRemotePrev => {
            if let Some(ar) = state.overlay.add_remote.as_mut() {
                if ar.selected > 0 {
                    ar.selected -= 1;
                }
            }
        }
        Action::AddRemoteNext => {
            if let Some(ar) = state.overlay.add_remote.as_mut() {
                if !ar.filtered.is_empty() && ar.selected + 1 < ar.filtered.len() {
                    ar.selected += 1;
                }
            }
        }
        Action::AddRemoteClose => {
            state.overlay.add_remote = None;
        }
        Action::AddRemoteConfirm => {
            // Resolve first (immutable borrow released before we mutate state).
            let chosen = state
                .overlay
                .add_remote
                .as_ref()
                .and_then(|ar| ar.chosen_host());
            let host = match chosen {
                None => {
                    if let Some(ar) = state.overlay.add_remote.as_mut() {
                        ar.error = Some("enter a hostname".into());
                    }
                    return fx;
                }
                Some(h) => h,
            };
            if state.config_remotes.iter().any(|r| r.host == host) {
                if let Some(ar) = state.overlay.add_remote.as_mut() {
                    ar.error = Some("already added".into());
                }
                return fx;
            }
            state.config_remotes.push(crate::config::RemoteConfig {
                host: host.clone(),
                forwards: vec![],
            });
            state.overlay.add_remote = None;
            fx.save_config = true;
            fx.refresh_sessions = true;
            fx.add_remote_host = Some(host);
        }
```
(`return fx;` matches the early-return style already used elsewhere in this match, e.g. the `ReorderSession`/`MenuConfirm` arms.)

- [ ] **Step 4: Consume the side effects in dispatch (`src/app/dispatch.rs`)**

In the side-effect-handling section (where `fx.remove_remote_host` and `fx.open_new_session_picker` are consumed), add:
```rust
        if fx.open_add_remote_picker {
            self.open_add_remote_picker();
        }
        if let Some(ref host) = fx.add_remote_host {
            self.onboard_remote_host(host);
        }
```
Place the `add_remote_host` block near the existing `if let Some(ref host) = fx.remove_remote_host {` block, and `open_add_remote_picker` near `if fx.open_new_session_picker`.

Add the helper method on `impl App` (next to `open_new_session_picker`):
```rust
    fn open_add_remote_picker(&mut self) {
        use std::collections::HashSet;
        let existing: HashSet<&str> = self
            .state
            .config_remotes
            .iter()
            .map(|r| r.host.as_str())
            .collect();
        let hosts: Vec<String> = crate::infra::ssh::config_hosts()
            .into_iter()
            .filter(|h| !existing.contains(h.as_str()))
            .collect();
        self.state.overlay.add_remote = Some(crate::add_remote::AddRemoteState::new(hosts));
    }
```
`onboard_remote_host(&mut self, host: &str)` already exists (used by `reload_config`); it seeds the runtime row + spawns the connection. `fx.refresh_sessions` (already consumed in this section) then surfaces the host's sessions.

- [ ] **Step 5: Route keys to the picker (`src/app/action/keyboard.rs`)**

Near the top of `key_to_action`, right after the `new_session` overlay check:
```rust
    if state.overlay.add_remote.is_some() {
        return add_remote_key_to_action(key);
    }
```
Add the helper (near `new_session_key_to_action`):
```rust
fn add_remote_key_to_action(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::AddRemoteClose,
        KeyCode::Enter => Action::AddRemoteConfirm,
        KeyCode::Up => Action::AddRemotePrev,
        KeyCode::Down => Action::AddRemoteNext,
        _ => Action::AddRemoteInputKey(*key),
    }
}
```

- [ ] **Step 6: Remove the now-stale allows**

`crate::add_remote::AddRemoteState`/`filter_hosts` are now used by the reducer/dispatch, and `config_hosts`/`parse_config_hosts` by dispatch. Remove:
- the module-level `#![allow(dead_code)]` line in `src/model/add_remote.rs`,
- the two `#[allow(dead_code)]` lines on `parse_config_hosts` and `config_hosts` in `src/infra/ssh.rs`.

Then run `cargo clippy -- -D warnings`. If any item still warns (genuinely unused), restore just that allow and report it — but with the wiring in place all four should be live.

- [ ] **Step 7: Build, lint, test, commit**

Run: `cargo build` (clean), `cargo clippy -- -D warnings` (clean), `cargo test` (all green — no new tests this task; the reducer logic is covered by Task 3's helper tests + the manual check).
```bash
git add src/model/state.rs src/app/action/mod.rs src/app/action/reduce.rs src/app/dispatch.rs src/app/action/keyboard.rs src/model/add_remote.rs src/infra/ssh.rs
git commit -m "feat(add-remote): wire picker — menu, actions, reducer, dispatch, keys"
```

---

## Task 5: Render the picker (`ui/add_remote.rs`)

**Files:**
- Create: `src/ui/add_remote.rs`
- Modify: `src/ui/mod.rs` (declare module + re-export `draw_add_remote`)
- Modify: `src/app/render.rs` (clone overlay + draw when open)

- [ ] **Step 1: Create the render module**

Create `src/ui/add_remote.rs`:

```rust
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;

use crate::add_remote::AddRemoteState;
use crate::theme::Theme;
use crate::ui::form::field_row;
use crate::ui::widgets::{popup_frame, PopupStyle, TextAreaColors};

const POPUP_WIDTH: u16 = 56;
const MAX_VISIBLE: usize = 8;

pub fn draw_add_remote(frame: &mut Frame, area: Rect, state: &AddRemoteState, theme: &Theme) {
    // Always reserve at least one list row (for the "(no hosts)" line).
    let visible = state.filtered.len().min(MAX_VISIBLE).max(1);
    let extra_err = if state.error.is_some() { 1 } else { 0 };
    // borders(2) + host(1) + blank(1) + list(visible) + blank(1) + [err] + footer(1)
    let height = (2 + 1 + 1 + visible as u16 + 1 + extra_err + 1)
        .min(area.height.saturating_sub(2));
    let width = POPUP_WIDTH.min(area.width.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    let inner = popup_frame(
        frame.buffer_mut(),
        popup,
        PopupStyle {
            title: Some(" Add Remote Host "),
            border_fg: theme.accent,
            bg: theme.bg,
        },
    );

    let mut constraints = vec![
        Constraint::Length(1), // host input
        Constraint::Length(1), // blank
    ];
    for _ in 0..visible {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // blank
    if state.error.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // footer
    constraints.push(Constraint::Min(0));
    let rows = Layout::vertical(constraints).split(inner);
    let mut i = 0;

    // --- host input ---
    field_row(
        frame.buffer_mut(),
        rows[i],
        "  host: ",
        Style::default().fg(theme.accent),
        &state.input,
        true,
        TextAreaColors {
            fg: theme.text,
            bg: theme.bg,
            cursor_fg: theme.bg,
            cursor_bg: theme.accent,
        },
    );
    i += 1;
    i += 1; // blank

    // --- candidate list ---
    if state.filtered.is_empty() {
        Paragraph::new(Span::styled(
            "    (no ~/.ssh/config hosts \u{2014} type a hostname)",
            Style::default().fg(theme.dim),
        ))
        .render(rows[i], frame.buffer_mut());
        i += 1;
    } else {
        let start = scroll_window(state.selected, state.filtered.len(), MAX_VISIBLE);
        let end = (start + MAX_VISIBLE).min(state.filtered.len());
        for (pos, idx) in state.filtered[start..end].iter().enumerate() {
            let display = start + pos;
            let sel = display == state.selected;
            let bg = if sel { theme.surface } else { theme.bg };
            let marker = if sel { "\u{25b8}" } else { " " };
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default()
                        .fg(if sel { theme.accent } else { theme.bg })
                        .bg(bg),
                ),
                Span::styled(
                    state.hosts[*idx].clone(),
                    Style::default().fg(theme.text).bg(bg),
                ),
            ]))
            .render(rows[i], frame.buffer_mut());
            i += 1;
        }
        for _ in (end - start)..visible {
            i += 1; // pad unused list rows
        }
    }
    i += 1; // blank

    // --- error ---
    if let Some(err) = &state.error {
        Paragraph::new(Span::styled(
            format!("  \u{26a0} {err}"),
            Style::default().fg(theme.pink),
        ))
        .render(rows[i], frame.buffer_mut());
        i += 1;
    }

    // --- footer ---
    Paragraph::new(Span::styled(
        "  \u{23ce} add   \u{2191}\u{2193} select   \u{238b} cancel",
        Style::default().fg(theme.dim).add_modifier(Modifier::DIM),
    ))
    .render(rows[i], frame.buffer_mut());
}

/// First visible index so `selected` stays in view.
fn scroll_window(selected: usize, total: usize, window: usize) -> usize {
    if total <= window || selected < window {
        return 0;
    }
    let max_start = total - window;
    (selected + 1).saturating_sub(window).min(max_start)
}
```

- [ ] **Step 2: Declare + export the module (`src/ui/mod.rs`)**

Add a module declaration alongside `mod new_session;` (match its visibility — `new_session` is `mod new_session;` private):
```rust
mod add_remote;
```
Add a re-export alongside `pub use new_session::draw_new_session;`:
```rust
pub use add_remote::draw_add_remote;
```

- [ ] **Step 3: Draw it when open (`src/app/render.rs`)**

Near the top where other overlays are cloned (e.g. `let new_session_overlay = s.overlay.new_session.clone();`), add:
```rust
        let add_remote_overlay = s.overlay.add_remote.clone();
```
Near the `draw_new_session` call block, add:
```rust
            if let Some(ref ar) = add_remote_overlay {
                ui::draw_add_remote(frame, frame.area(), ar, theme);
            }
```

- [ ] **Step 4: Build, lint, test**

Run: `cargo build` (clean), `cargo clippy -- -D warnings` (clean), `cargo test` (all green).

- [ ] **Step 5: Commit**

```bash
git add src/ui/add_remote.rs src/ui/mod.rs src/app/render.rs
git commit -m "feat(add-remote): render the host picker popup"
```

---

## Task 6: Final verification

- [ ] **Step 1: Full build + lint + tests**

Run: `cargo build && cargo clippy -- -D warnings && cargo test`
Expected: clean build, no warnings (confirm no stray `#[allow(dead_code)]` remain on the add-remote/ssh items added by this feature), all tests green.

- [ ] **Step 2: Manual end-to-end**

In `./target/release/deck` (`cargo build --release` first):

1. **Part 1**: a Connected remote host's `[⟳]` is now the same accent color as its `@host` label and `[…]` (not green). Make a host Unreachable (bad host) → `[⟳]` is pink.
2. Right-click empty sidebar area → context menu shows **Add Remote Host** right under **New session**.
3. Select it → popup lists your `~/.ssh/config` hosts (minus already-added). Type to filter; `↑/↓` to select; `⏎` adds the highlighted host → its section appears and connects within ~1s.
4. Type a hostname not in `~/.ssh/config` (list goes empty) → `⏎` adds it verbatim.
5. Try adding a host already in the list → `⚠ already added`, popup stays open. `⎋` closes with no change.
6. Confirm the added host persists across a restart (written to `~/.config/deck/config.json`).

---

## Self-review notes

Spec coverage:
- Part 1 reconnect color (Connected→accent; Connecting/Unreachable kept) → Task 1.
- `~/.ssh/config` enumeration (wildcards excluded, manual-input fallback) → Task 2.
- `AddRemoteState` + filter + chosen-host resolution → Task 3.
- Menu item, actions, reducer (filter/confirm/duplicate guard), dispatch onboard, key routing → Task 4.
- Popup rendering (input + list + error + footer, empty-state line) → Task 5.
- Allow cleanup + manual verification → Task 6.

Type/name consistency: `AddRemoteState`, `filter_hosts`, `make_textarea`, `chosen_host`, `refilter`, `AddRemote{InputKey,Next,Prev,Confirm,Close}`, `open_add_remote_picker`, `add_remote_host`, `open_add_remote_picker()`, `draw_add_remote`, `config_hosts`, `parse_config_hosts` are used identically across the tasks that define and consume them.

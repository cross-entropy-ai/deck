# Compact Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a compact view mode (2 lines/card) alongside the existing expanded mode (5 lines/card), toggled in Settings or via `c` shortcut, persisted to config.

**Architecture:** Add `ViewMode` enum to state, make `CARD_HEIGHT` a function of view mode, add a compact card renderer alongside the existing expanded renderer, plumb view mode through sidebar drawing, scroll offset, and click-to-session mapping. Add Settings row and `c` shortcut.

**Tech Stack:** Rust, ratatui

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/state.rs` | Modify | `ViewMode` enum, `view_mode` field on `AppState`, update `session_at_row` to use dynamic card height |
| `src/config.rs` | Modify | `view_mode` field on `Config`, parse/serialize |
| `src/ui.rs` | Modify | `card_height()` function replacing `CARD_HEIGHT` const, `draw_sessions_compact()`, pass `view_mode` to `draw_sessions`/`draw_sidebar`/`scroll_offset`, `SettingsView.view_mode`, 5th settings entry |
| `src/action.rs` | Modify | `ToggleViewMode` action, settings index shift, `c` keybind |
| `src/app.rs` | Modify | Load/store `view_mode`, pass to renderer, `save_config` |

---

### Task 1: Add ViewMode enum and config persistence

**Files:**
- Modify: `src/state.rs`
- Modify: `src/config.rs`

- [ ] **Step 1: Write failing tests for config round-trip**

Add to the `tests` module in `src/config.rs`:

```rust
    #[test]
    fn parse_json_with_view_mode() {
        let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28,
  "view_mode": "compact"
}"#;
        let config = parse_json(json).unwrap();
        assert_eq!(config.view_mode, "compact");
    }

    #[test]
    fn parse_json_without_view_mode_uses_default() {
        let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28
}"#;
        let config = parse_json(json).unwrap();
        assert_eq!(config.view_mode, "expanded");
    }

    #[test]
    fn config_to_json_includes_view_mode() {
        let mut config = Config::default();
        config.view_mode = "compact".to_string();
        let json = config.to_json();
        assert!(json.contains(r#""view_mode": "compact""#));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test config::tests`
Expected: compilation error — no `view_mode` field on Config.

- [ ] **Step 3: Add ViewMode enum to state.rs**

In `src/state.rs`, add after the `FilterMode` block (after line 66):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Expanded,
    Compact,
}
```

- [ ] **Step 4: Add `view_mode` field to Config and Default**

In `src/config.rs`, add to `Config` struct:
```rust
    pub view_mode: String,
```

Add to `Default`:
```rust
            view_mode: "expanded".to_string(),
```

- [ ] **Step 5: Update parse_json to handle view_mode**

In `src/config.rs`, in the `parse_json` function, add a case in the key match:
```rust
            "view_mode" => config.view_mode = val.trim_matches('"').to_string(),
```

- [ ] **Step 6: Update to_json to include view_mode**

In `src/config.rs`, change the `to_json` format string. Replace:
```rust
            "{{\n  \"theme\": {},\n  \"layout\": {},\n  \"show_borders\": {},\n  \"sidebar_width\": {},\n  \"exclude_patterns\": {}\n}}\n",
```
with:
```rust
            "{{\n  \"theme\": {},\n  \"layout\": {},\n  \"show_borders\": {},\n  \"sidebar_width\": {},\n  \"view_mode\": {},\n  \"exclude_patterns\": {}\n}}\n",
```

And add `quote(&self.view_mode),` as the 5th argument (before `patterns_json`).

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test config::tests`
Expected: all 14 tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/state.rs src/config.rs
git commit -m "feat: add ViewMode enum and config persistence for view_mode"
```

---

### Task 2: Plumb ViewMode through AppState and card height

**Files:**
- Modify: `src/state.rs`
- Modify: `src/ui.rs`
- Modify: `src/app.rs`
- Modify: `src/action.rs` (test helper only)

- [ ] **Step 1: Add `view_mode` to AppState**

In `src/state.rs`, add `view_mode: ViewMode` to the `AppState` struct (after `layout_mode: LayoutMode,`):
```rust
    pub view_mode: ViewMode,
```

Add `view_mode: ViewMode` parameter to `AppState::new()` (after `layout_mode`):
```rust
    pub fn new(
        theme_index: usize,
        layout_mode: LayoutMode,
        view_mode: ViewMode,
        show_borders: bool,
        sidebar_width: u16,
        term_width: u16,
        term_height: u16,
        exclude_patterns: Vec<String>,
        compiled_patterns: Vec<ExcludePattern>,
    ) -> Self {
```

Initialize in the `Self { ... }` block (after `layout_mode,`):
```rust
            view_mode,
```

- [ ] **Step 2: Replace CARD_HEIGHT const with card_height function in ui.rs**

In `src/ui.rs`, change:
```rust
pub const CARD_HEIGHT: usize = 5;
```
to:
```rust
use crate::state::ViewMode;

pub fn card_height(view_mode: ViewMode) -> usize {
    match view_mode {
        ViewMode::Expanded => 5,
        ViewMode::Compact => 2,
    }
}
```

- [ ] **Step 3: Update scroll_offset in ui.rs**

Change the `scroll_offset` function signature and body:
```rust
fn scroll_offset(focused: usize, visible_height: u16, ch: usize) -> usize {
    let focused_bottom = (focused + 1) * ch;
    let visible = visible_height as usize;
    if focused_bottom > visible {
        focused_bottom - visible
    } else {
        0
    }
}
```

Update the call in `draw_sessions` (around line 349):
```rust
    let scroll = scroll_offset(focused, area.height, card_height(ViewMode::Expanded));
```

(This will be changed to use a `view_mode` parameter in Task 3; for now hardcode `Expanded` to keep things compiling.)

- [ ] **Step 4: Update state.rs imports and session_at_row**

In `src/state.rs`, change the import from:
```rust
use crate::ui::{self, SessionView, CARD_HEIGHT};
```
to:
```rust
use crate::ui::{self, SessionView, card_height};
```

In `session_at_row`, replace:
```rust
        let card_height = CARD_HEIGHT;
```
with:
```rust
        let ch = card_height(self.view_mode);
```

And update all uses of `card_height` in that function to use `ch`:
```rust
        let focused_bottom = (self.focused + 1) * ch;
```
and:
```rust
        let idx = clicked_row / ch;
```

- [ ] **Step 5: Update app.rs to load view_mode from config and pass to AppState**

In `src/app.rs`, in `App::new()`, after loading `show_borders` from config:
```rust
        let view_mode = match cfg.view_mode.as_str() {
            "compact" => ViewMode::Compact,
            _ => ViewMode::Expanded,
        };
```

Add `use crate::state::ViewMode;` to the imports at the top of app.rs (alongside the existing state imports).

Update the `AppState::new()` call to pass `view_mode` after `layout_mode`:
```rust
        let state = AppState::new(
            theme_index,
            layout_mode,
            view_mode,
            show_borders,
            sidebar_width,
            term_width,
            term_height,
            exclude_patterns,
            compiled_patterns,
        );
```

Update `save_config()` to include `view_mode`:
```rust
    fn save_config(&self) {
        Config {
            theme: THEMES[self.state.theme_index].name.to_string(),
            layout: match self.state.layout_mode {
                LayoutMode::Horizontal => "horizontal",
                LayoutMode::Vertical => "vertical",
            }
            .to_string(),
            show_borders: self.state.show_borders,
            sidebar_width: self.state.sidebar_width,
            view_mode: match self.state.view_mode {
                ViewMode::Expanded => "expanded",
                ViewMode::Compact => "compact",
            }
            .to_string(),
            exclude_patterns: self.state.exclude_patterns.clone(),
        }
        .save();
    }
```

- [ ] **Step 6: Fix make_test_state in action.rs tests**

Update the `make_test_state` helper to pass the new parameter. Change:
```rust
        let mut state = AppState::new(0, LayoutMode::Horizontal, true, 28, 120, 40, vec![], vec![]);
```
to:
```rust
        let mut state = AppState::new(0, LayoutMode::Horizontal, ViewMode::Expanded, true, 28, 120, 40, vec![], vec![]);
```

Add `ViewMode` to the test imports:
```rust
    use crate::state::{AppState, FilterMode, FocusMode, LayoutMode, MainView, SessionRow, ViewMode};
```

- [ ] **Step 7: Build and run all tests**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/state.rs src/ui.rs src/app.rs src/action.rs
git commit -m "feat: plumb ViewMode through AppState, replace CARD_HEIGHT const with card_height()"
```

---

### Task 3: Compact card renderer

**Files:**
- Modify: `src/ui.rs`

- [ ] **Step 1: Add view_mode parameter to draw_sessions and draw_sidebar**

In `src/ui.rs`, change `draw_sessions` signature to add `view_mode: ViewMode`:
```rust
fn draw_sessions(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    spinner_frame: &str,
    theme: &Theme,
    view_mode: ViewMode,
) {
```

At the start of `draw_sessions`, after the empty-sessions check, dispatch by mode. Wrap the existing body (from line ~198 `let width = area.width...` through to the end of the function) in a match arm for `Expanded`, and add a `Compact` arm that calls a new function:

```rust
    match view_mode {
        ViewMode::Expanded => {
            // ... existing code stays here unchanged ...
        }
        ViewMode::Compact => {
            draw_sessions_compact(frame, area, sessions, focused, spinner_frame, theme);
        }
    }
```

Update the scroll line inside the Expanded arm from:
```rust
    let scroll = scroll_offset(focused, area.height, card_height(ViewMode::Expanded));
```
to:
```rust
    let scroll = scroll_offset(focused, area.height, card_height(view_mode));
```

- [ ] **Step 2: Add view_mode parameter to draw_sidebar**

In `draw_sidebar`, add `view_mode: ViewMode` parameter (after `spinner_frame`):
```rust
pub fn draw_sidebar(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    sidebar_active: bool,
    theme: &Theme,
    filter_mode: FilterMode,
    show_help: bool,
    confirm_kill: Option<&str>,
    rename_input: Option<(&str, usize)>,
    show_borders: bool,
    tabs_mode: bool,
    spinner_frame: &str,
    view_mode: ViewMode,
) {
```

Pass it through to `draw_sessions`:
```rust
        draw_sessions(
            frame,
            sessions_area,
            sessions,
            focused,
            spinner_frame,
            theme,
            view_mode,
        );
```

- [ ] **Step 3: Update the call in app.rs**

In `src/app.rs`, in the `render` method, update the `draw_sidebar` call to pass `view_mode`. After `&spinner_frame,` add:

```rust
                s.view_mode,
```

Wait — `s` is already destructured into local vars. Read `view_mode` from `s` alongside the others. At around line 378 (where `let main_view = s.main_view;`), add:
```rust
        let view_mode = s.view_mode;
```

Then pass `view_mode` as the last arg to `draw_sidebar`.

- [ ] **Step 4: Implement draw_sessions_compact**

Add this function in `src/ui.rs`, after `draw_sessions`:

```rust
fn draw_sessions_compact(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    spinner_frame: &str,
    theme: &Theme,
) {
    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    for (i, session) in sessions.iter().enumerate() {
        let is_focused = i == focused;
        let is_current = session.is_current;
        let is_emphasized = is_focused || is_current;

        let accent_color = if is_current {
            theme.green
        } else if is_focused {
            theme.accent
        } else {
            theme.bg
        };
        let accent = if is_current || is_focused { "▌" } else { " " };
        let name_style = if is_current && is_focused {
            Style::default()
                .fg(theme.green)
                .add_modifier(Modifier::BOLD)
        } else if is_focused || is_current {
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.secondary)
        };
        let index_style = if is_focused {
            Style::default().fg(theme.secondary)
        } else {
            Style::default().fg(theme.dim)
        };
        let bg = if is_focused { theme.surface } else { theme.bg };

        // Row 1: accent + activity + index + name + branch + git status
        let activity_text = format_activity_compact(session.idle_seconds, spinner_frame);
        let activity_color = idle_color(theme, session.idle_seconds, is_emphasized);
        let idx_str = format!("{:>2}", i + 1);

        let mut spans = vec![
            Span::styled(accent, Style::default().fg(accent_color).bg(bg)),
            Span::styled(&activity_text, Style::default().fg(activity_color).bg(bg)),
            Span::styled(idx_str, index_style.bg(bg)),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(truncate(session.name, width.saturating_sub(6)), name_style.bg(bg)),
        ];

        if !session.branch.is_empty() {
            let branch_color = if is_focused {
                theme.pink
            } else if is_current {
                theme.secondary
            } else {
                theme.muted
            };
            spans.push(Span::styled("  ", Style::default().bg(bg)));
            spans.push(Span::styled(
                truncate(session.branch, width.saturating_sub(20)),
                Style::default().fg(branch_color).bg(bg),
            ));

            let status = format_git_status(session, true);
            if !status.is_empty() {
                let status_color = if status == "✓" {
                    if is_emphasized { theme.green } else { theme.muted }
                } else if is_emphasized {
                    theme.yellow
                } else {
                    theme.dim
                };
                spans.push(Span::styled(" ", Style::default().bg(bg)));
                spans.push(Span::styled(status, Style::default().fg(status_color).bg(bg)));
            }
        }

        lines.push(pad_line(spans, bg, width));

        // Row 2: directory
        let text_width = width.saturating_sub(6);
        let dir_display = truncate(&shorten_dir(session.dir), text_width);
        let dir_color = if is_focused {
            theme.teal
        } else if is_current {
            theme.secondary
        } else {
            theme.muted
        };
        lines.push(pad_line(
            vec![
                Span::styled("      ", Style::default().bg(bg)),
                Span::styled(dir_display, Style::default().fg(dir_color).bg(bg)),
            ],
            bg,
            width,
        ));
    }

    let scroll = scroll_offset(focused, area.height, card_height(ViewMode::Compact));
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(theme.bg))
            .scroll((scroll as u16, 0)),
        area,
    );
}
```

- [ ] **Step 5: Build and run tests**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/ui.rs src/app.rs
git commit -m "feat: add compact card renderer (2 lines/card)"
```

---

### Task 4: ToggleViewMode action, Settings row, and `c` keybind

**Files:**
- Modify: `src/state.rs`
- Modify: `src/action.rs`
- Modify: `src/ui.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Write tests for ToggleViewMode**

Add to the `tests` module in `src/action.rs`:

```rust
    #[test]
    fn toggle_view_mode_flips_and_saves() {
        let mut state = make_test_state(1);
        assert_eq!(state.view_mode, ViewMode::Expanded);
        let fx = apply_action(&mut state, Action::ToggleViewMode);
        assert_eq!(state.view_mode, ViewMode::Compact);
        assert!(fx.save_config);
        let fx = apply_action(&mut state, Action::ToggleViewMode);
        assert_eq!(state.view_mode, ViewMode::Expanded);
        assert!(fx.save_config);
    }

    #[test]
    fn settings_adjust_view_mode_toggles() {
        let mut state = make_test_state(1);
        state.settings_selected = 3;
        let fx = apply_action(&mut state, Action::SettingsAdjust(1));
        assert_eq!(state.view_mode, ViewMode::Compact);
        assert!(fx.save_config);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test action::tests`
Expected: compilation error — `Action::ToggleViewMode` not defined.

- [ ] **Step 3: Bump SETTINGS_ITEM_COUNT**

In `src/state.rs`, change:
```rust
pub const SETTINGS_ITEM_COUNT: usize = 4;
```
to:
```rust
pub const SETTINGS_ITEM_COUNT: usize = 5;
```

- [ ] **Step 4: Add ToggleViewMode action variant**

In `src/action.rs`, add after `ToggleBorders,`:
```rust
    ToggleViewMode,
```

- [ ] **Step 5: Add apply_action handler for ToggleViewMode**

In `src/action.rs`, in `apply_action()`, add after the `Action::ToggleBorders` handler:

```rust
        Action::ToggleViewMode => {
            state.view_mode = match state.view_mode {
                ViewMode::Expanded => ViewMode::Compact,
                ViewMode::Compact => ViewMode::Expanded,
            };
            fx.save_config = true;
        }
```

Add `ViewMode` to the imports at the top of `action.rs`:
```rust
use crate::state::{
    AppState, ContextMenu, FilterMode, FocusMode, KillRequest, LayoutMode, MainView, MenuKind,
    RenameRequest, RenameState, SideEffect, ViewMode, GLOBAL_MENU_ITEMS, SESSION_MENU_ITEMS,
    SETTINGS_ITEM_COUNT,
};
```

- [ ] **Step 6: Shift SettingsAdjust indices**

In `src/action.rs`, in the `Action::SettingsAdjust(direction)` match, change:
- Index 3 → `ToggleViewMode` (was `OpenExcludeEditor`)
- Index 4 → `OpenExcludeEditor` (new)

Replace:
```rust
        Action::SettingsAdjust(direction) => match state.settings_selected {
            0 => {
                let _ = direction;
                apply_action(state, Action::OpenThemePicker);
            }
            1 => {
                let inner = apply_action(state, Action::ToggleLayout);
                fx.resize_pty = inner.resize_pty;
                fx.save_config = inner.save_config;
            }
            2 => {
                let inner = apply_action(state, Action::ToggleBorders);
                fx.resize_pty = inner.resize_pty;
                fx.save_config = inner.save_config;
            }
            3 => {
                let _ = direction;
                apply_action(state, Action::OpenExcludeEditor);
            }
            _ => {}
        },
```
with:
```rust
        Action::SettingsAdjust(direction) => match state.settings_selected {
            0 => {
                let _ = direction;
                apply_action(state, Action::OpenThemePicker);
            }
            1 => {
                let inner = apply_action(state, Action::ToggleLayout);
                fx.resize_pty = inner.resize_pty;
                fx.save_config = inner.save_config;
            }
            2 => {
                let inner = apply_action(state, Action::ToggleBorders);
                fx.resize_pty = inner.resize_pty;
                fx.save_config = inner.save_config;
            }
            3 => {
                let _ = direction;
                let inner = apply_action(state, Action::ToggleViewMode);
                fx.save_config = inner.save_config;
            }
            4 => {
                let _ = direction;
                apply_action(state, Action::OpenExcludeEditor);
            }
            _ => {}
        },
```

- [ ] **Step 7: Add `c` keybind in sidebar**

In `src/action.rs`, in `sidebar_key_to_action`, add before the `_ => Action::None` fallback:
```rust
        // Toggle compact/expanded view
        KeyCode::Char('c') => Action::ToggleViewMode,
```

- [ ] **Step 8: Add View row to Settings UI**

In `src/ui.rs`, add `view_mode: ViewMode` to the `SettingsView` struct:
```rust
    pub view_mode: ViewMode,
```

In `draw_settings_page`, add a "View" entry in the `entries` vec after "Borders" and before "Exclude":
```rust
        (
            "View",
            match settings.view_mode {
                ViewMode::Expanded => "Expanded".to_string(),
                ViewMode::Compact => "Compact".to_string(),
            },
            "Left/right toggles compact mode",
        ),
```

- [ ] **Step 9: Pass view_mode to SettingsView in app.rs**

In `src/app.rs`, in the `SettingsView` construction, add:
```rust
            view_mode: s.view_mode,
```

- [ ] **Step 10: Run all tests**

Run: `cargo test`
Expected: all tests pass (existing + 2 new).

- [ ] **Step 11: Commit**

```bash
git add src/state.rs src/action.rs src/ui.rs src/app.rs
git commit -m "feat: add ToggleViewMode action, Settings row, and c keybind"
```

---

### Task 5: Final cleanup

**Files:**
- All modified files

- [ ] **Step 1: Run clippy**

Run: `cargo clippy`
Expected: no new warnings from our changes.

- [ ] **Step 2: Run full test suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 3: Build release**

Run: `cargo build --release`
Expected: builds successfully.

- [ ] **Step 4: Fix any issues**

Fix any clippy warnings or test failures from our changes.

- [ ] **Step 5: Commit fixes if needed**

```bash
git add -A
git commit -m "chore: fix clippy warnings"
```

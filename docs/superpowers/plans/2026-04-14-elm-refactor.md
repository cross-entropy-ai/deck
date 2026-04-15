# Elm-Style Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor app.rs into three files (action.rs, state.rs, app.rs) using an Elm-style architecture with Action enum and pure functions, enabling unit testing of all business logic.

**Architecture:** Events are mapped to Actions via pure functions (`key_to_action`, `mouse_to_action`). Actions are applied to state via a pure function (`apply_action`) that returns side effects. App is a thin shell that executes side effects and manages PTY/render. All state lives in `AppState`.

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29

---

## File Structure

```
src/
  action.rs    — NEW: Action enum + key_to_action() / mouse_to_action()
  state.rs     — NEW: AppState, SideEffect, apply_action(), ContextMenu, enums
  app.rs       — REWRITE: thin shell (App struct with pty/parser/spinner, event loop, render)
  main.rs      — MODIFY: add `mod action; mod state;`
  // all other files unchanged
```

**What moves where:**

| Current location (app.rs) | Destination |
|---|---|
| `LayoutMode`, `FocusMode`, `FilterMode`, `SessionRow`, `ContextMenu`, `MenuKind` | state.rs |
| `SIDEBAR_MIN/MAX`, `SIDEBAR_HEIGHT*` constants | state.rs |
| `SESSION_MENU_ITEMS`, `GLOBAL_MENU_ITEMS` | state.rs |
| `handle_key`, `handle_sidebar_key`, `handle_mouse`, `handle_context_menu_key` | action.rs (rewritten as pure functions) |
| `session_at_row`, `session_at_col`, `menu_item_at` | state.rs (methods on AppState) |
| `recompute_filter`, `sync_order`, `apply_order`, `reorder_session` | state.rs (methods on AppState) |
| `kill_focused_session`, `do_kill_focused_session`, navigation logic | state.rs (inside apply_action) |
| `resize_sidebar`, `resize_sidebar_height`, `effective_sidebar_height`, `pty_size` | state.rs (methods on AppState) |
| `App` struct, `new`, `run`, `render`, `switch_project`, `create_new_session`, PTY methods | app.rs (stays, rewritten) |

---

### Task 1: Create state.rs with AppState and enums

**Files:**
- Create: `src/state.rs`
- Modify: `src/main.rs`

This task moves all enums, data types, constants, and the AppState struct into state.rs. No logic yet — just the types and a constructor. The goal is to make it compile with the new module registered.

- [ ] **Step 1: Create `src/state.rs` with types and AppState**

```rust
use crate::theme::THEMES;
use crate::ui::{self, SessionView, CARD_HEIGHT};

// --- Constants ---

pub const SIDEBAR_MIN: u16 = 16;
pub const SIDEBAR_MAX: u16 = 60;
pub const SIDEBAR_HEIGHT: u16 = 4;
pub const SIDEBAR_HEIGHT_MIN: u16 = 3;
pub const SIDEBAR_HEIGHT_MAX: u16 = 4;

pub const SESSION_MENU_ITEMS: &[&str] = &["Switch", "Kill", "Move up", "Move down"];
pub const GLOBAL_MENU_ITEMS: &[&str] = &["New session", "Toggle layout", "Toggle borders", "Cycle theme", "Cycle filter", "Quit"];

// --- Enums ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusMode {
    Main,
    Sidebar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    All,
    Working,
    Idle,
}

impl FilterMode {
    pub fn label(self) -> &'static str {
        match self {
            FilterMode::All => "Projects",
            FilterMode::Working => "Working",
            FilterMode::Idle => "Idle",
        }
    }

    pub fn next_label(self) -> &'static str {
        self.next().label()
    }

    pub fn next(self) -> Self {
        match self {
            FilterMode::All => FilterMode::Working,
            FilterMode::Working => FilterMode::Idle,
            FilterMode::Idle => FilterMode::All,
        }
    }
}

// --- Context menu ---

#[derive(Debug, Clone)]
pub enum MenuKind {
    Session { filtered_idx: usize },
    Global,
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub kind: MenuKind,
    pub x: u16,
    pub y: u16,
    pub selected: usize,
}

impl ContextMenu {
    pub fn items(&self) -> &[&str] {
        match self.kind {
            MenuKind::Session { .. } => SESSION_MENU_ITEMS,
            MenuKind::Global => GLOBAL_MENU_ITEMS,
        }
    }
}

// --- Session data ---

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub name: String,
    pub dir: String,
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub is_current: bool,
    pub idle_seconds: u64,
}

// --- Side effects ---

#[derive(Debug, Default)]
pub struct SideEffect {
    pub switch_session: Option<String>,
    pub kill_session: Option<KillRequest>,
    pub create_session: bool,
    pub resize_pty: bool,
    pub save_config: bool,
    pub refresh_sessions: bool,
    pub quit: bool,
}

/// Info needed to execute a kill: which session to kill, and optionally
/// which session to switch to first (if killing the current session).
#[derive(Debug)]
pub struct KillRequest {
    pub name: String,
    pub switch_to: Option<String>,
}

// --- AppState ---

pub struct AppState {
    // Session data
    pub sessions: Vec<SessionRow>,
    pub filtered: Vec<usize>,
    pub focused: usize,
    pub current_session: String,
    pub filter_mode: FilterMode,
    pub session_order: Vec<String>,

    // UI state
    pub focus_mode: FocusMode,
    pub theme_index: usize,
    pub layout_mode: LayoutMode,
    pub sidebar_width: u16,
    pub sidebar_height: u16,
    pub show_help: bool,
    pub confirm_kill: bool,
    pub show_borders: bool,
    pub context_menu: Option<ContextMenu>,
    pub hover_separator: bool,
    pub dragging_separator: bool,

    // Terminal dimensions
    pub term_width: u16,
    pub term_height: u16,
}

impl AppState {
    pub fn new(
        theme_index: usize,
        layout_mode: LayoutMode,
        show_borders: bool,
        sidebar_width: u16,
        term_width: u16,
        term_height: u16,
    ) -> Self {
        Self {
            sessions: Vec::new(),
            filtered: Vec::new(),
            focused: 0,
            current_session: String::new(),
            filter_mode: FilterMode::All,
            session_order: Vec::new(),
            focus_mode: FocusMode::Main,
            theme_index,
            layout_mode,
            sidebar_width,
            sidebar_height: SIDEBAR_HEIGHT,
            show_help: false,
            confirm_kill: false,
            show_borders,
            context_menu: None,
            hover_separator: false,
            dragging_separator: false,
            term_width,
            term_height,
        }
    }
}
```

- [ ] **Step 2: Add `mod state;` to main.rs**

In `src/main.rs`, add `mod state;` after the existing module declarations. The line should be inserted so the list stays alphabetical:

```rust
mod app;
mod bridge;
mod config;
mod git;
mod metrics;
mod pty;
mod state;
mod theme;
mod tmux;
mod ui;
```

- [ ] **Step 3: Build**

Run: `cargo check 2>&1`
Expected: compiles with warnings about unused imports/fields (state.rs types are defined but not yet used). No errors.

- [ ] **Step 4: Commit**

```bash
git add src/state.rs src/main.rs
git commit -m "refactor: add state.rs with AppState, enums, and SideEffect types"
```

---

### Task 2: Add helper methods to AppState

**Files:**
- Modify: `src/state.rs`

Move the pure computation methods from App onto AppState: hit-testing, filtering, ordering, resize clamping, pty size calculation. These methods only read/write AppState fields and have no side effects.

- [ ] **Step 1: Add session hit-testing methods**

Append to the `impl AppState` block in `src/state.rs`:

```rust
impl AppState {
    // ... (existing new() method) ...

    pub fn effective_sidebar_height(&self) -> u16 {
        if self.show_borders { 4 } else { 2 }
    }

    pub fn pty_size(&self) -> (u16, u16) {
        let bo = if self.show_borders { 2u16 } else { 0 };
        match self.layout_mode {
            LayoutMode::Horizontal => {
                let cols = self.term_width.saturating_sub(self.sidebar_width + 1 + bo).max(1);
                let rows = self.term_height.saturating_sub(bo).max(1);
                (rows, cols)
            }
            LayoutMode::Vertical => {
                let cols = self.term_width.saturating_sub(bo).max(1);
                let rows = self.term_height.saturating_sub(self.effective_sidebar_height() + bo).max(1);
                (rows, cols)
            }
        }
    }

    /// Map a screen row to a filtered session index (horizontal/card mode).
    pub fn session_at_row(&self, row: u16) -> Option<usize> {
        let b = if self.show_borders { 1u16 } else { 0 };
        let sidebar_h = match self.layout_mode {
            LayoutMode::Horizontal => self.term_height,
            LayoutMode::Vertical => self.effective_sidebar_height(),
        };
        let header_height = 2u16;
        let footer_height = 2u16;
        let sessions_top = b + header_height;
        let sessions_bottom = sidebar_h.saturating_sub(b + footer_height);
        if row < sessions_top || row >= sessions_bottom {
            return None;
        }
        let visible_height = sessions_bottom - sessions_top;
        let card_height = CARD_HEIGHT;
        let focused_bottom = (self.focused + 1) * card_height;
        let visible = visible_height as usize;
        let scroll = if focused_bottom > visible {
            focused_bottom - visible
        } else {
            0
        };
        let clicked_row = row as usize - sessions_top as usize + scroll;
        let idx = clicked_row / card_height;
        if idx < self.filtered.len() {
            Some(idx)
        } else {
            None
        }
    }

    /// Map a screen column to a tab index in vertical/tabs mode.
    pub fn session_at_col(&self, col: u16) -> Option<usize> {
        let b = if self.show_borders { 1u16 } else { 0 };
        let views: Vec<SessionView> = self
            .filtered
            .iter()
            .map(|&i| {
                let s = &self.sessions[i];
                SessionView {
                    name: s.name.as_str(),
                    dir: s.dir.as_str(),
                    branch: s.branch.as_str(),
                    ahead: s.ahead,
                    behind: s.behind,
                    staged: s.staged,
                    modified: s.modified,
                    untracked: s.untracked,
                    is_current: s.is_current,
                    idle_seconds: s.idle_seconds,
                }
            })
            .collect();
        let ranges = ui::tab_col_ranges(&views);
        let local_col = col.saturating_sub(b);
        for (i, &(start, end)) in ranges.iter().enumerate() {
            if local_col >= start && local_col < end {
                return Some(i);
            }
        }
        None
    }

    /// Map a screen position to a context menu item index.
    pub fn menu_item_at(&self, col: u16, row: u16) -> Option<usize> {
        let menu = self.context_menu.as_ref()?;
        let items = menu.items();
        let menu_width = ui::context_menu_width(items);
        let menu_height = items.len() as u16 + 2;
        let mx = menu.x.min(self.term_width.saturating_sub(menu_width));
        let my = menu.y.min(self.term_height.saturating_sub(menu_height));
        if col > mx && col < mx + menu_width - 1 && row > my && row < my + menu_height - 1 {
            let idx = (row - my - 1) as usize;
            if idx < items.len() {
                return Some(idx);
            }
        }
        None
    }

    // --- Filtering and ordering ---

    pub fn recompute_filter(&mut self) {
        self.filtered = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| match self.filter_mode {
                FilterMode::All => true,
                FilterMode::Working => s.idle_seconds < 3,
                FilterMode::Idle => s.idle_seconds >= 3,
            })
            .map(|(i, _)| i)
            .collect();

        if !self.filtered.is_empty() && self.focused >= self.filtered.len() {
            self.focused = self.filtered.len() - 1;
        }
    }

    pub fn sync_order(&mut self) {
        let names: Vec<String> = self.sessions.iter().map(|s| s.name.clone()).collect();
        self.session_order.retain(|n| names.contains(n));
        for name in &names {
            if !self.session_order.contains(name) {
                self.session_order.push(name.clone());
            }
        }
    }

    pub fn apply_order(&mut self) {
        let order = &self.session_order;
        self.sessions.sort_by_key(|s| {
            order
                .iter()
                .position(|n| n == &s.name)
                .unwrap_or(usize::MAX)
        });
    }

    /// Clamp and set sidebar width. Returns true if it changed.
    pub fn resize_sidebar(&mut self, new_width: u16) -> bool {
        let clamped = new_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX.min(self.term_width - 10));
        if clamped == self.sidebar_width {
            return false;
        }
        self.sidebar_width = clamped;
        true
    }

    /// Clamp and set sidebar height. Returns true if it changed.
    pub fn resize_sidebar_height(&mut self, new_height: u16) -> bool {
        let clamped = new_height.clamp(SIDEBAR_HEIGHT_MIN, SIDEBAR_HEIGHT_MAX.min(self.term_height - 6));
        if clamped == self.sidebar_height {
            return false;
        }
        self.sidebar_height = clamped;
        true
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo check 2>&1`
Expected: compiles. Warnings about unused methods are fine — they'll be used in the next tasks.

- [ ] **Step 3: Commit**

```bash
git add src/state.rs
git commit -m "refactor: add helper methods to AppState"
```

---

### Task 3: Create action.rs with Action enum and apply_action

**Files:**
- Create: `src/action.rs`
- Modify: `src/main.rs`

This is the core task. Define the Action enum and implement `apply_action` as a pure function that handles every action.

- [ ] **Step 1: Create `src/action.rs` with Action enum and apply_action**

```rust
use crate::state::{
    AppState, ContextMenu, FilterMode, FocusMode, KillRequest, LayoutMode, MenuKind, SideEffect,
    SIDEBAR_HEIGHT_MAX, SIDEBAR_HEIGHT_MIN, SIDEBAR_MAX, SIDEBAR_MIN,
};
use crate::theme::THEMES;

#[derive(Debug)]
pub enum Action {
    // Navigation
    FocusNext,
    FocusPrev,
    FocusIndex(usize),

    // Session operations
    SwitchProject,
    KillSession,
    ConfirmKill,
    CancelKill,
    CreateSession,
    ReorderSession(i32),

    // UI toggles
    ToggleLayout,
    ToggleBorders,
    CycleTheme,
    ToggleHelp,
    DismissHelp,

    // Filter
    CycleFilter,

    // Focus mode
    SetFocusMain,
    SetFocusSidebar,
    ToggleFocus,

    // Context menu
    OpenSessionMenu { filtered_idx: usize, x: u16, y: u16 },
    OpenGlobalMenu { x: u16, y: u16 },
    MenuNext,
    MenuPrev,
    MenuConfirm,
    MenuDismiss,
    MenuHover(usize),
    MenuClickItem(usize),

    // Resize
    ResizeSidebar(u16),
    ResizeSidebarHeight(u16),
    StartDrag,
    StopDrag,
    SetHoverSeparator(bool),

    // Terminal
    Resize(u16, u16),

    // PTY passthrough (handled by App, not apply_action)
    ForwardKey(Vec<u8>),
    ForwardMouse(Vec<u8>),

    // Lifecycle
    Quit,

    // No-op
    None,
}

pub fn apply_action(state: &mut AppState, action: Action) -> SideEffect {
    let mut fx = SideEffect::default();

    match action {
        // --- Navigation ---
        Action::FocusNext => {
            if !state.filtered.is_empty() {
                state.focused = (state.focused + 1).min(state.filtered.len() - 1);
            }
        }
        Action::FocusPrev => {
            if state.focused > 0 {
                state.focused -= 1;
            }
        }
        Action::FocusIndex(idx) => {
            if idx < state.filtered.len() {
                state.focused = idx;
            }
        }

        // --- Session operations ---
        Action::SwitchProject => {
            if let Some(&session_idx) = state.filtered.get(state.focused) {
                let name = state.sessions[session_idx].name.clone();
                fx.switch_session = Some(name);
                fx.refresh_sessions = true;
            }
        }
        Action::KillSession => {
            if state.sessions.len() > 1 && state.filtered.get(state.focused).is_some() {
                state.confirm_kill = true;
            }
        }
        Action::ConfirmKill => {
            state.confirm_kill = false;
            if state.sessions.len() <= 1 {
                return fx;
            }
            let Some(&session_idx) = state.filtered.get(state.focused) else {
                return fx;
            };
            let is_current = state.sessions[session_idx].is_current;
            let name = state.sessions[session_idx].name.clone();

            let next_focused = if state.focused + 1 < state.filtered.len() {
                state.focused
            } else {
                state.focused.saturating_sub(1)
            };

            let switch_to = if is_current {
                let alt_idx = if state.focused + 1 < state.filtered.len() {
                    state.focused + 1
                } else if state.focused > 0 {
                    state.focused - 1
                } else {
                    return fx;
                };
                Some(state.sessions[state.filtered[alt_idx]].name.clone())
            } else {
                Option::None
            };

            state.session_order.retain(|n| n != &name);
            state.focused = next_focused.min(state.filtered.len().saturating_sub(1));

            fx.kill_session = Some(KillRequest { name, switch_to });
            fx.refresh_sessions = true;
        }
        Action::CancelKill => {
            state.confirm_kill = false;
        }
        Action::CreateSession => {
            fx.create_session = true;
            fx.refresh_sessions = true;
        }
        Action::ReorderSession(direction) => {
            if state.filter_mode != FilterMode::All {
                return fx;
            }
            let Some(&session_idx) = state.filtered.get(state.focused) else {
                return fx;
            };
            let name = state.sessions[session_idx].name.clone();
            if let Some(pos) = state.session_order.iter().position(|n| n == &name) {
                let new_pos = (pos as i32 + direction)
                    .clamp(0, state.session_order.len() as i32 - 1) as usize;
                if new_pos != pos {
                    state.session_order.swap(pos, new_pos);
                    state.apply_order();
                    state.recompute_filter();
                    if let Some(new_focused) = state
                        .filtered
                        .iter()
                        .position(|&i| state.sessions[i].name == name)
                    {
                        state.focused = new_focused;
                    }
                }
            }
        }

        // --- UI toggles ---
        Action::ToggleLayout => {
            state.layout_mode = match state.layout_mode {
                LayoutMode::Horizontal => LayoutMode::Vertical,
                LayoutMode::Vertical => LayoutMode::Horizontal,
            };
            fx.resize_pty = true;
            fx.save_config = true;
        }
        Action::ToggleBorders => {
            state.show_borders = !state.show_borders;
            fx.resize_pty = true;
            fx.save_config = true;
        }
        Action::CycleTheme => {
            state.theme_index = (state.theme_index + 1) % THEMES.len();
            fx.save_config = true;
        }
        Action::ToggleHelp => {
            state.show_help = true;
        }
        Action::DismissHelp => {
            state.show_help = false;
        }

        // --- Filter ---
        Action::CycleFilter => {
            state.filter_mode = state.filter_mode.next();
            state.recompute_filter();
        }

        // --- Focus mode ---
        Action::SetFocusMain => {
            state.focus_mode = FocusMode::Main;
        }
        Action::SetFocusSidebar => {
            state.focus_mode = FocusMode::Sidebar;
        }
        Action::ToggleFocus => {
            state.focus_mode = match state.focus_mode {
                FocusMode::Main => FocusMode::Sidebar,
                FocusMode::Sidebar => FocusMode::Main,
            };
        }

        // --- Context menu ---
        Action::OpenSessionMenu { filtered_idx, x, y } => {
            state.focused = filtered_idx;
            state.context_menu = Some(ContextMenu {
                kind: MenuKind::Session { filtered_idx },
                x,
                y,
                selected: 0,
            });
        }
        Action::OpenGlobalMenu { x, y } => {
            state.context_menu = Some(ContextMenu {
                kind: MenuKind::Global,
                x,
                y,
                selected: 0,
            });
        }
        Action::MenuNext => {
            if let Some(ref mut menu) = state.context_menu {
                let len = menu.items().len();
                menu.selected = (menu.selected + 1).min(len - 1);
            }
        }
        Action::MenuPrev => {
            if let Some(ref mut menu) = state.context_menu {
                if menu.selected > 0 {
                    menu.selected -= 1;
                }
            }
        }
        Action::MenuConfirm => {
            let menu = match state.context_menu.take() {
                Some(m) => m,
                Option::None => return fx,
            };
            match menu.kind {
                MenuKind::Session { filtered_idx } => {
                    state.focused = filtered_idx;
                    match menu.selected {
                        0 => {
                            // Switch
                            let inner = apply_action(state, Action::SwitchProject);
                            fx.switch_session = inner.switch_session;
                            fx.refresh_sessions = inner.refresh_sessions;
                            state.focus_mode = FocusMode::Main;
                        }
                        1 => {
                            // Kill
                            apply_action(state, Action::KillSession);
                        }
                        2 => {
                            // Move up
                            apply_action(state, Action::ReorderSession(-1));
                        }
                        3 => {
                            // Move down
                            apply_action(state, Action::ReorderSession(1));
                        }
                        _ => {}
                    }
                }
                MenuKind::Global => {
                    match menu.selected {
                        0 => {
                            fx.create_session = true;
                            fx.refresh_sessions = true;
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
                            let inner = apply_action(state, Action::CycleTheme);
                            fx.save_config = inner.save_config;
                        }
                        4 => {
                            apply_action(state, Action::CycleFilter);
                        }
                        5 => {
                            fx.quit = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        Action::MenuDismiss => {
            state.context_menu = None;
        }
        Action::MenuHover(idx) => {
            if let Some(ref mut menu) = state.context_menu {
                menu.selected = idx;
            }
        }

        // --- Resize ---
        Action::ResizeSidebar(width) => {
            if state.resize_sidebar(width) {
                fx.resize_pty = true;
            }
        }
        Action::ResizeSidebarHeight(height) => {
            if state.resize_sidebar_height(height) {
                fx.resize_pty = true;
            }
        }
        Action::StartDrag => {
            state.dragging_separator = true;
        }
        Action::StopDrag => {
            state.dragging_separator = false;
            fx.save_config = true;
        }
        Action::SetHoverSeparator(hover) => {
            state.hover_separator = hover;
        }

        // --- Terminal resize ---
        Action::Resize(w, h) => {
            state.term_width = w;
            state.term_height = h;
            fx.resize_pty = true;
        }

        // --- Passthrough (handled by App directly, not here) ---
        Action::ForwardKey(_) | Action::ForwardMouse(_) => {}

        // --- Lifecycle ---
        Action::Quit => {
            fx.quit = true;
        }

        Action::None => {}
    }

    fx
}
```

- [ ] **Step 2: Add `mod action;` to main.rs**

```rust
mod action;
mod app;
mod bridge;
mod config;
mod git;
mod metrics;
mod pty;
mod state;
mod theme;
mod tmux;
mod ui;
```

- [ ] **Step 3: Build**

Run: `cargo check 2>&1`
Expected: compiles. Warnings about unused variants/functions are fine.

- [ ] **Step 4: Commit**

```bash
git add src/action.rs src/main.rs
git commit -m "refactor: add action.rs with Action enum and apply_action"
```

---

### Task 4: Add event mapping functions to action.rs

**Files:**
- Modify: `src/action.rs`

Add `key_to_action` and `mouse_to_action` — pure functions that translate crossterm events into Actions. These replicate the logic currently in `App::handle_key`, `App::handle_sidebar_key`, and `App::handle_mouse`, but return an Action instead of mutating state.

- [ ] **Step 1: Add key_to_action and mouse_to_action**

Append to `src/action.rs`, after the `apply_action` function:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

pub fn key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    // Context menu intercepts all keys
    if state.context_menu.is_some() {
        return match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::MenuNext,
            KeyCode::Char('k') | KeyCode::Up => Action::MenuPrev,
            KeyCode::Enter => Action::MenuConfirm,
            _ => Action::MenuDismiss,
        };
    }

    // Ctrl+S always toggles focus mode
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
        return Action::ToggleFocus;
    }

    match state.focus_mode {
        FocusMode::Main => {
            let bytes = crate::pty::encode_key(key);
            if bytes.is_empty() {
                Action::None
            } else {
                Action::ForwardKey(bytes)
            }
        }
        FocusMode::Sidebar => sidebar_key_to_action(key, state),
    }
}

fn sidebar_key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    // Help showing: any key dismisses
    if state.show_help {
        return Action::DismissHelp;
    }

    // Kill confirmation
    if state.confirm_kill {
        return if key.code == KeyCode::Char('y') {
            Action::ConfirmKill
        } else {
            Action::CancelKill
        };
    }

    let code = key.code;
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    match code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Esc => Action::SetFocusMain,

        // Help
        KeyCode::Char('h') | KeyCode::Char('?') => Action::ToggleHelp,

        // Navigation
        KeyCode::Char('j') | KeyCode::Down if !alt => Action::FocusNext,
        KeyCode::Char('k') | KeyCode::Up if !alt => Action::FocusPrev,

        // Switch project
        KeyCode::Enter => Action::SwitchProject,

        // Number keys 1-9 quick jump
        KeyCode::Char(c @ '1'..='9') if !alt => {
            let idx = (c as usize) - ('1' as usize);
            if idx < state.filtered.len() {
                Action::FocusIndex(idx)
            } else {
                Action::None
            }
        }

        // Kill session
        KeyCode::Char('x') => Action::KillSession,

        // Filter
        KeyCode::Char('f') => Action::CycleFilter,

        // Theme cycle
        KeyCode::Char('t') => Action::CycleTheme,

        // Toggle borders
        KeyCode::Char('b') => Action::ToggleBorders,

        // Toggle layout
        KeyCode::Char('l') => Action::ToggleLayout,

        // Reorder: Alt+Up / Alt+Down
        KeyCode::Up if alt => Action::ReorderSession(-1),
        KeyCode::Down if alt => Action::ReorderSession(1),

        _ => Action::None,
    }
}

pub fn mouse_to_action(mouse: &MouseEvent, state: &AppState) -> Action {
    // Context menu intercepts all mouse events
    if state.context_menu.is_some() {
        return match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(idx) = state.menu_item_at(mouse.column, mouse.row) {
                    return Action::MenuClickItem(idx);
                }
                Action::MenuDismiss
            }
            MouseEventKind::Down(MouseButton::Right) => Action::MenuDismiss,
            MouseEventKind::Moved => {
                if let Some(idx) = state.menu_item_at(mouse.column, mouse.row) {
                    Action::MenuHover(idx)
                } else {
                    Action::None
                }
            }
            _ => Action::None,
        };
    }

    let (on_separator, in_sidebar) = match state.layout_mode {
        LayoutMode::Horizontal => {
            let gap_col = state.sidebar_width;
            let on_sep =
                mouse.column >= gap_col.saturating_sub(1) && mouse.column <= gap_col + 1;
            let in_sb = mouse.column < state.sidebar_width;
            (on_sep, in_sb)
        }
        LayoutMode::Vertical => {
            let in_sb = mouse.row < state.effective_sidebar_height();
            (false, in_sb)
        }
    };

    match mouse.kind {
        MouseEventKind::Moved => {
            return Action::SetHoverSeparator(on_separator);
        }
        MouseEventKind::Down(MouseButton::Left) if on_separator => {
            return Action::StartDrag;
        }
        MouseEventKind::Drag(MouseButton::Left) if state.dragging_separator => {
            return match state.layout_mode {
                LayoutMode::Horizontal => Action::ResizeSidebar(mouse.column + 1),
                LayoutMode::Vertical => Action::ResizeSidebarHeight(mouse.row + 1),
            };
        }
        MouseEventKind::Up(MouseButton::Left) if state.dragging_separator => {
            return Action::StopDrag;
        }
        _ => {}
    }

    // Scroll in sidebar area
    if in_sidebar {
        match mouse.kind {
            MouseEventKind::ScrollUp => return Action::FocusPrev,
            MouseEventKind::ScrollDown => return Action::FocusNext,
            _ => {}
        }
    }

    // Click in sidebar area
    if mouse.kind == MouseEventKind::Down(MouseButton::Left) && in_sidebar {
        let idx = match state.layout_mode {
            LayoutMode::Horizontal => state.session_at_row(mouse.row),
            LayoutMode::Vertical => state.session_at_col(mouse.column),
        };
        if let Some(idx) = idx {
            return Action::FocusIndex(idx);
        }
        return Action::SetFocusSidebar;
    }

    // Right-click in sidebar area
    if mouse.kind == MouseEventKind::Down(MouseButton::Right) && in_sidebar {
        let idx = match state.layout_mode {
            LayoutMode::Horizontal => state.session_at_row(mouse.row),
            LayoutMode::Vertical => state.session_at_col(mouse.column),
        };
        return if let Some(idx) = idx {
            Action::OpenSessionMenu {
                filtered_idx: idx,
                x: mouse.column,
                y: mouse.row,
            }
        } else {
            Action::OpenGlobalMenu {
                x: mouse.column,
                y: mouse.row,
            }
        };
    }

    // Click/interact in main pane area
    if !in_sidebar && !on_separator && !state.dragging_separator {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            // Need to both set focus and forward mouse
            let b = if state.show_borders { 1u16 } else { 0 };
            let (col_off, row_off) = match state.layout_mode {
                LayoutMode::Horizontal => (state.sidebar_width + 1 + b, b),
                LayoutMode::Vertical => (b, state.effective_sidebar_height()),
            };
            let bytes = crate::pty::encode_mouse(mouse, col_off, row_off);
            if bytes.is_empty() {
                return Action::SetFocusMain;
            }
            // We need SetFocusMain + ForwardMouse. Since we can only return one action,
            // we return ForwardMouse and handle the focus change there.
            return Action::ForwardMouse(bytes);
        }
        let b = if state.show_borders { 1u16 } else { 0 };
        let (col_off, row_off) = match state.layout_mode {
            LayoutMode::Horizontal => (state.sidebar_width + 1 + b, b),
            LayoutMode::Vertical => (b, state.effective_sidebar_height()),
        };
        let bytes = crate::pty::encode_mouse(mouse, col_off, row_off);
        if !bytes.is_empty() {
            return Action::ForwardMouse(bytes);
        }
    }

    Action::None
}
```

**Important behavioral notes for the mapping:**

There are a few places where the original code does two things at once (e.g., click in sidebar = focus sidebar + select session + switch project; click in main = set focus main + forward mouse; context menu click = set selected + confirm). These compound behaviors need special handling:

1. **Sidebar click (select + switch):** `FocusIndex` action followed by `SwitchProject` — handled by App dispatching two actions sequentially.
2. **Main pane left-click (focus + forward):** `ForwardMouse` action — App sets focus to Main when it sees this action.
3. **Context menu left-click (select + confirm):** `MenuClickItem(idx)` returned from mouse mapping — App dispatches `MenuHover(idx)` then `MenuConfirm`.
4. **Number keys (focus + switch):** `FocusIndex` action — App follows up with `SwitchProject` + `SetFocusMain`.
5. **Enter key (switch + focus main):** `SwitchProject` action — App follows up with `SetFocusMain`.

These compound dispatches are documented and handled in Task 5 (App rewrite).

- [ ] **Step 2: Add the crossterm imports at the top of action.rs**

Make sure the crossterm imports are at the top of the file (before the Action enum). Move the `use crossterm::event::...` line to the top imports section:

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::state::{
    AppState, ContextMenu, FilterMode, FocusMode, KillRequest, LayoutMode, MenuKind, SideEffect,
    SIDEBAR_HEIGHT_MAX, SIDEBAR_HEIGHT_MIN, SIDEBAR_MAX, SIDEBAR_MIN,
};
use crate::theme::THEMES;
```

(Remove the duplicate `use crossterm::event::...` from the middle of the file if it was placed inline.)

- [ ] **Step 3: Build**

Run: `cargo check 2>&1`
Expected: compiles. Unused import warnings for `SIDEBAR_*` constants are fine since they're only used indirectly through AppState methods now. Remove any truly unused imports.

- [ ] **Step 4: Commit**

```bash
git add src/action.rs
git commit -m "refactor: add key_to_action and mouse_to_action event mapping"
```

---

### Task 5: Rewrite app.rs as thin shell

**Files:**
- Rewrite: `src/app.rs`

Replace the current app.rs with a thin shell that uses `AppState`, `Action`, `key_to_action`, `mouse_to_action`, and `apply_action`. The `App` struct keeps only PTY, parser, spinner, and a reference to state. All event handling goes through the action pipeline.

- [ ] **Step 1: Rewrite `src/app.rs`**

Replace the entire contents of `src/app.rs` with:

```rust
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind, MouseButton, MouseEventKind};
use portable_pty::PtySize;
use ratatui::layout::{Constraint, Layout};
use ratatui::DefaultTerminal;

use crate::action::{self, Action};
use crate::bridge;
use crate::config::Config;
use crate::git;
use crate::pty::{self, Pty, PtyEvent};
use crate::state::{AppState, FocusMode, LayoutMode, SessionRow, SIDEBAR_MAX, SIDEBAR_MIN};
use crate::theme::THEMES;
use crate::tmux;
use crate::ui::{self, SessionView};

const POLL_MS: u64 = 16;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub struct App {
    state: AppState,
    pty: Pty,
    parser: vt100::Parser,
    spinner: rattles::Rattler<rattles::presets::braille::Dots>,
}

impl App {
    pub fn new(term_width: u16, term_height: u16) -> io::Result<Self> {
        let cfg = Config::load();

        let theme_index = THEMES
            .iter()
            .position(|t| t.name == cfg.theme)
            .unwrap_or(0);
        let layout_mode = match cfg.layout.as_str() {
            "vertical" => LayoutMode::Vertical,
            _ => LayoutMode::Horizontal,
        };
        let show_borders = cfg.show_borders;
        let sidebar_width = cfg.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);

        let state = AppState::new(
            theme_index,
            layout_mode,
            show_borders,
            sidebar_width,
            term_width,
            term_height,
        );

        let (pty_rows, pty_cols) = state.pty_size();
        let pty = Pty::spawn(
            "tmux",
            &["attach"],
            PtySize {
                rows: pty_rows,
                cols: pty_cols,
                pixel_width: 0,
                pixel_height: 0,
            },
        )?;
        let parser = vt100::Parser::new(pty_rows, pty_cols, 0);

        let mut app = App {
            state,
            pty,
            parser,
            spinner: rattles::presets::braille::dots(),
        };

        app.refresh_sessions();
        if let Some(pos) = app.state.filtered.iter().position(|&i| app.state.sessions[i].is_current) {
            app.state.focused = pos;
        }

        Ok(app)
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut last_refresh = Instant::now();
        let mut pty_alive = true;

        loop {
            // 1. Drain PTY output
            for event in self.pty.drain() {
                match event {
                    PtyEvent::Output(data) => self.parser.process(&data),
                    PtyEvent::Exited => pty_alive = false,
                }
            }

            // 2. Render
            self.render(terminal)?;

            // 3. Poll input and dispatch
            if event::poll(Duration::from_millis(POLL_MS))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        let action = action::key_to_action(&key, &self.state);
                        if self.dispatch(action) {
                            break;
                        }
                    }
                    Event::Mouse(mouse) => {
                        let action = action::mouse_to_action(&mouse, &self.state);
                        self.dispatch(action);
                    }
                    Event::Resize(w, h) => {
                        self.dispatch(Action::Resize(w, h));
                    }
                    _ => {}
                }
            }

            // 4. Periodic refresh
            if last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.refresh_sessions();
                last_refresh = Instant::now();
            }

            // 5. If PTY died, try to reattach
            if !pty_alive {
                if tmux::list_sessions().is_empty() {
                    break;
                }
                match self.respawn_pty() {
                    Ok(()) => {
                        pty_alive = true;
                        self.refresh_sessions();
                    }
                    Err(_) => break,
                }
            }
        }

        Ok(())
    }

    /// Dispatch an action through the pipeline. Returns true if the app should exit.
    fn dispatch(&mut self, action: Action) -> bool {
        // Handle compound actions that need multiple dispatches
        match &action {
            // PTY passthrough — no state change needed
            Action::ForwardKey(bytes) => {
                let _ = self.pty.write(bytes);
                return false;
            }
            Action::ForwardMouse(bytes) => {
                let _ = self.pty.write(bytes);
                // Left-click in main area also sets focus
                self.state.focus_mode = FocusMode::Main;
                return false;
            }
            _ => {}
        }

        // Check for compound key actions that need follow-up
        let needs_switch = matches!(&action, Action::SwitchProject);
        let is_focus_index_from_number_key = matches!(&action, Action::FocusIndex(_))
            && self.state.focus_mode == FocusMode::Sidebar
            && self.state.context_menu.is_none()
            && !self.state.show_help
            && !self.state.confirm_kill;

        // Context menu click: MenuClickItem needs follow-up MenuConfirm
        let is_menu_click = matches!(&action, Action::MenuClickItem(_));

        let fx = action::apply_action(&mut self.state, action);

        // Compound: SwitchProject from keyboard → also set focus main
        if needs_switch && fx.switch_session.is_some() {
            self.state.focus_mode = FocusMode::Main;
        }

        // Compound: FocusIndex from number key → also switch + focus main
        if is_focus_index_from_number_key {
            let inner_fx = action::apply_action(&mut self.state, Action::SwitchProject);
            if inner_fx.switch_session.is_some() {
                self.execute_side_effects(&inner_fx);
                self.state.focus_mode = FocusMode::Main;
            }
        }

        // Compound: Menu click → select item, then confirm
        if is_menu_click {
            if let Action::MenuClickItem(idx) = action {
                action::apply_action(&mut self.state, Action::MenuHover(idx));
            }
            let inner_fx = action::apply_action(&mut self.state, Action::MenuConfirm);
            self.execute_side_effects(&inner_fx);
            if inner_fx.quit {
                return true;
            }
        }

        // Compound: Sidebar left-click on a session → also switch project
        // (FocusIndex from mouse click in sidebar)
        // This is handled by checking if we came from a sidebar click
        // We detect this via: focus_mode is now Sidebar and we just focused an index
        // Actually — sidebar click in original code does: focus sidebar + select + switch
        // mouse_to_action returns FocusIndex for sidebar clicks. We need to switch too.
        // We set focus to sidebar and dispatch switch.

        self.execute_side_effects(&fx);
        fx.quit
    }

    fn execute_side_effects(&mut self, fx: &crate::state::SideEffect) {
        if let Some(ref name) = fx.switch_session {
            if self.pty.slave_tty.is_empty() {
                tmux::switch_session(name);
            } else {
                tmux::switch_client_for_tty(&self.pty.slave_tty, name);
            }
        }

        if let Some(ref kill) = fx.kill_session {
            if let Some(ref alt_name) = kill.switch_to {
                if self.pty.slave_tty.is_empty() {
                    tmux::switch_session(alt_name);
                } else {
                    tmux::switch_client_for_tty(&self.pty.slave_tty, alt_name);
                }
            }
            tmux::kill_session(&kill.name);
        }

        if fx.create_session {
            self.create_new_session();
        }

        if fx.resize_pty {
            self.resize_pty();
        }

        if fx.save_config {
            self.save_config();
        }

        if fx.refresh_sessions {
            self.refresh_sessions();
        }
    }

    fn create_new_session(&mut self) {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let dir = format!("{}/claude", home);
        let existing: Vec<&str> = self.state.sessions.iter().map(|s| s.name.as_str()).collect();
        let mut idx = self.state.sessions.len();
        let name = loop {
            let candidate = format!("session-{}", idx);
            if !existing.contains(&candidate.as_str()) {
                break candidate;
            }
            idx += 1;
        };
        if tmux::new_session(&name, &dir).is_some() {
            if self.pty.slave_tty.is_empty() {
                tmux::switch_session(&name);
            } else {
                tmux::switch_client_for_tty(&self.pty.slave_tty, &name);
            }
        }
    }

    fn render(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let s = &self.state;
        let sidebar_active = s.focus_mode == FocusMode::Sidebar;
        let focused = s.focused;
        let theme = &THEMES[s.theme_index];
        let filter_label = s.filter_mode.label().to_string();
        let filter_next = s.filter_mode.next_label().to_string();
        let confirm_kill = s.confirm_kill;
        let show_help = s.show_help;
        let context_menu = s.context_menu.clone();
        let show_borders = s.show_borders;
        let layout_mode = s.layout_mode;
        let sidebar_width = s.sidebar_width;

        let confirm_name = if confirm_kill {
            s.filtered
                .get(s.focused)
                .map(|&i| s.sessions[i].name.clone())
        } else {
            None
        };

        let views_owned: Vec<SessionRow> = s
            .filtered
            .iter()
            .map(|&i| s.sessions[i].clone())
            .collect();

        let screen_snapshot = Some(self.parser.screen().clone());
        let hover_sep = s.hover_separator || s.dragging_separator;

        terminal.draw(|frame| {
            let views: Vec<SessionView> = views_owned
                .iter()
                .map(|r| SessionView {
                    name: r.name.as_str(),
                    dir: r.dir.as_str(),
                    branch: r.branch.as_str(),
                    ahead: r.ahead,
                    behind: r.behind,
                    staged: r.staged,
                    modified: r.modified,
                    untracked: r.untracked,
                    is_current: r.is_current,
                    idle_seconds: r.idle_seconds,
                })
                .collect();

            let (sidebar_area, gap_area, main_area) = match layout_mode {
                LayoutMode::Horizontal => {
                    let [s, g, m] = Layout::horizontal([
                        Constraint::Length(sidebar_width),
                        Constraint::Length(1),
                        Constraint::Min(1),
                    ])
                    .areas(frame.area());
                    (s, Some(g), m)
                }
                LayoutMode::Vertical => {
                    let tab_h = if show_borders { 4u16 } else { 2u16 };
                    let [s, m] = Layout::vertical([
                        Constraint::Length(tab_h),
                        Constraint::Min(1),
                    ])
                    .areas(frame.area());
                    (s, None, m)
                }
            };

            ui::draw_sidebar(
                frame,
                sidebar_area,
                &views,
                focused,
                sidebar_active,
                theme,
                &filter_label,
                show_help,
                confirm_name.as_deref(),
                show_borders,
                layout_mode == LayoutMode::Vertical,
                self.spinner.current_frame(),
                &filter_next,
            );

            if let Some(gap) = gap_area {
                let sep_fg = if hover_sep { theme.subtle } else { theme.dim };
                for y in gap.y..gap.bottom() {
                    if let Some(cell) = frame.buffer_mut().cell_mut((gap.x, y)) {
                        cell.set_char('│');
                        cell.set_style(ratatui::style::Style::default().fg(sep_fg));
                    }
                }
            }

            if let Some(ref screen) = screen_snapshot {
                if show_borders {
                    let main_border_color = if sidebar_active { theme.dim } else { theme.accent };
                    let main_block = ratatui::widgets::Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .border_set(ratatui::symbols::border::ROUNDED)
                        .border_style(ratatui::style::Style::default().fg(main_border_color));
                    let main_inner = main_block.inner(main_area);
                    frame.render_widget(main_block, main_area);
                    bridge::render_screen(screen, main_inner, frame.buffer_mut());
                    if !sidebar_active {
                        bridge::set_cursor(frame, screen, main_inner);
                    }
                } else {
                    bridge::render_screen(screen, main_area, frame.buffer_mut());
                    if !sidebar_active {
                        bridge::set_cursor(frame, screen, main_area);
                    }
                }
            }

            if let Some(ref menu) = context_menu {
                ui::draw_context_menu(frame, menu.x, menu.y, menu.selected, menu.items(), theme);
            }
        })?;

        Ok(())
    }

    fn refresh_sessions(&mut self) {
        let current = if self.pty.slave_tty.is_empty() {
            tmux::current_session()
        } else {
            tmux::current_session_for_tty(&self.pty.slave_tty)
        }
        .unwrap_or_default();
        let sessions = tmux::list_sessions();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.state.sessions = sessions
            .into_iter()
            .filter(|s| !s.name.starts_with('_'))
            .map(|s| {
                let git_info = git::get_git_info(&s.dir);
                let idle_seconds = now.saturating_sub(s.activity);

                SessionRow {
                    is_current: s.name == current,
                    name: s.name,
                    dir: s.dir,
                    branch: git_info.branch,
                    ahead: git_info.ahead,
                    behind: git_info.behind,
                    staged: git_info.staged,
                    modified: git_info.modified,
                    untracked: git_info.untracked,
                    idle_seconds,
                }
            })
            .collect();

        self.state.sync_order();
        self.state.apply_order();
        self.state.recompute_filter();

        if self.state.focus_mode != FocusMode::Sidebar || self.state.current_session != current {
            if let Some(pos) = self.state.filtered.iter().position(|&i| self.state.sessions[i].is_current) {
                self.state.focused = pos;
            }
        }

        self.state.current_session = current;

        if !self.state.filtered.is_empty() && self.state.focused >= self.state.filtered.len() {
            self.state.focused = self.state.filtered.len() - 1;
        }
    }

    fn resize_pty(&mut self) {
        let (pty_rows, pty_cols) = self.state.pty_size();
        self.parser.screen_mut().set_size(pty_rows, pty_cols);
        let _ = self.pty.resize(PtySize {
            rows: pty_rows,
            cols: pty_cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    fn respawn_pty(&mut self) -> io::Result<()> {
        let (pty_rows, pty_cols) = self.state.pty_size();
        self.pty = Pty::spawn(
            "tmux",
            &["attach"],
            PtySize {
                rows: pty_rows,
                cols: pty_cols,
                pixel_width: 0,
                pixel_height: 0,
            },
        )?;
        self.parser = vt100::Parser::new(pty_rows, pty_cols, 0);
        Ok(())
    }

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
        }
        .save();
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo check 2>&1`
Expected: compiles with no errors. Fix any compilation issues — the most likely ones are:
- Missing public visibility on types used across modules
- Import path adjustments

- [ ] **Step 3: Commit**

```bash
git add src/app.rs
git commit -m "refactor: rewrite app.rs as thin shell using action pipeline"
```

---

### Task 6: Fix compound action dispatch for sidebar clicks

**Files:**
- Modify: `src/action.rs`
- Modify: `src/app.rs`

The original code has several compound behaviors where one input triggers multiple state changes. The most important ones:

1. **Sidebar left-click on session:** sets focus to sidebar, selects session index, switches project
2. **Number key 1-9:** focuses index, switches project, sets focus to main
3. **Enter key:** switches project, sets focus to main

These were partially handled in Task 5's `dispatch()` method, but need careful verification. The approach: `mouse_to_action` returns a compound-aware action, and `dispatch` handles the sequencing.

- [ ] **Step 1: Add SidebarClickSession action variant**

In `src/action.rs`, add a new variant to the Action enum:

```rust
    // Compound sidebar actions
    SidebarClickSession(usize),  // focus sidebar + select index + switch
```

- [ ] **Step 2: Update mouse_to_action for sidebar clicks**

In `mouse_to_action`, replace the sidebar left-click handling:

```rust
    // Click in sidebar area
    if mouse.kind == MouseEventKind::Down(MouseButton::Left) && in_sidebar {
        let idx = match state.layout_mode {
            LayoutMode::Horizontal => state.session_at_row(mouse.row),
            LayoutMode::Vertical => state.session_at_col(mouse.column),
        };
        if let Some(idx) = idx {
            return Action::SidebarClickSession(idx);
        }
        return Action::SetFocusSidebar;
    }
```

- [ ] **Step 3: Handle SidebarClickSession in dispatch**

In `src/app.rs`, add handling in `dispatch()` before the main `apply_action` call. Add this match arm alongside `ForwardKey` and `ForwardMouse`:

```rust
            Action::SidebarClickSession(idx) => {
                action::apply_action(&mut self.state, Action::SetFocusSidebar);
                action::apply_action(&mut self.state, Action::FocusIndex(idx));
                let fx = action::apply_action(&mut self.state, Action::SwitchProject);
                self.execute_side_effects(&fx);
                return false;
            }
```

And add it to the `apply_action` match in `action.rs` as a no-op (it's handled by dispatch):

```rust
        Action::SidebarClickSession(_) => {}
```

- [ ] **Step 4: Simplify number key and Enter handling in dispatch**

Clean up the `dispatch` method. The `is_focus_index_from_number_key` detection is fragile. Instead, add explicit compound variants:

In `action.rs`, add to the enum:

```rust
    NumberKeyJump(usize),   // focus index + switch + focus main
```

Update `sidebar_key_to_action`:

```rust
        // Number keys 1-9 quick jump
        KeyCode::Char(c @ '1'..='9') if !alt => {
            let idx = (c as usize) - ('1' as usize);
            if idx < state.filtered.len() {
                Action::NumberKeyJump(idx)
            } else {
                Action::None
            }
        }
```

And `SwitchProject` from Enter already needs follow-up `SetFocusMain`. Handle both in `dispatch`:

```rust
            Action::NumberKeyJump(idx) => {
                action::apply_action(&mut self.state, Action::FocusIndex(idx));
                let fx = action::apply_action(&mut self.state, Action::SwitchProject);
                self.execute_side_effects(&fx);
                self.state.focus_mode = crate::state::FocusMode::Main;
                return false;
            }
```

For `SwitchProject` from Enter, handle in `dispatch` before the main apply_action:

```rust
            Action::SwitchProject => {
                let fx = action::apply_action(&mut self.state, Action::SwitchProject);
                self.execute_side_effects(&fx);
                self.state.focus_mode = crate::state::FocusMode::Main;
                return false;
            }
```

Wait — this creates infinite recursion since `dispatch` matches `SwitchProject` and also passes it to `apply_action` via the default path. The solution: handle all compound actions at the top of `dispatch`, return early, and only fall through to the generic path for simple actions.

Restructure `dispatch` to:

```rust
    fn dispatch(&mut self, action: Action) -> bool {
        match action {
            // PTY passthrough
            Action::ForwardKey(ref bytes) => {
                let _ = self.pty.write(bytes);
                return false;
            }
            Action::ForwardMouse(ref bytes) => {
                let _ = self.pty.write(bytes);
                self.state.focus_mode = FocusMode::Main;
                return false;
            }

            // Compound: sidebar click → focus sidebar + select + switch
            Action::SidebarClickSession(idx) => {
                action::apply_action(&mut self.state, Action::SetFocusSidebar);
                action::apply_action(&mut self.state, Action::FocusIndex(idx));
                let fx = action::apply_action(&mut self.state, Action::SwitchProject);
                self.execute_side_effects(&fx);
                return false;
            }

            // Compound: number key → focus + switch + go to main
            Action::NumberKeyJump(idx) => {
                action::apply_action(&mut self.state, Action::FocusIndex(idx));
                let fx = action::apply_action(&mut self.state, Action::SwitchProject);
                self.execute_side_effects(&fx);
                self.state.focus_mode = FocusMode::Main;
                return false;
            }

            // Compound: Enter → switch + go to main
            Action::SwitchProject => {
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                self.state.focus_mode = FocusMode::Main;
                return fx.quit;
            }

            // Compound: context menu click → select item + confirm
            Action::MenuClickItem(idx) => {
                action::apply_action(&mut self.state, Action::MenuHover(idx));
                let fx = action::apply_action(&mut self.state, Action::MenuConfirm);
                self.execute_side_effects(&fx);
                return fx.quit;
            }

            // All simple actions
            _ => {
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                return fx.quit;
            }
        }
    }
```

- [ ] **Step 5: Add no-op arms in apply_action for compound variants**

In `src/action.rs`, add these to the match in `apply_action`:

```rust
        // Compound actions (dispatched by App, not handled here)
        Action::SidebarClickSession(_) | Action::NumberKeyJump(_) | Action::MenuClickItem(_) => {}
```

- [ ] **Step 6: Build**

Run: `cargo check 2>&1`
Expected: compiles with no errors.

- [ ] **Step 7: Commit**

```bash
git add src/action.rs src/app.rs
git commit -m "refactor: handle compound actions in dispatch"
```

---

### Task 7: Manual testing

**Files:** None (testing only)

- [ ] **Step 1: Build release**

Run: `cargo build --release 2>&1`
Expected: compiles with no errors.

- [ ] **Step 2: Test in tmux**

Launch the sidebar inside a tmux session and verify every interaction:

1. **Navigation:** j/k, Up/Down, scroll wheel — session highlight moves
2. **Switch:** Enter, click on session, number keys 1-9 — session switches, focus returns to main
3. **Focus:** Ctrl+S toggles sidebar/main focus, Esc goes to main
4. **Filter:** f cycles All/Working/Idle
5. **Theme:** t cycles themes
6. **Layout:** l toggles horizontal/vertical
7. **Borders:** b toggles borders
8. **Help:** h/? shows help, any key dismisses
9. **Kill:** x shows confirmation, y confirms, other key cancels
10. **Reorder:** Alt+Up/Down reorders sessions
11. **Right-click session:** context menu appears, hover highlights, click executes, Esc/right-click dismisses
12. **Right-click empty area:** global menu appears with all options working
13. **Separator drag:** drag the separator to resize sidebar
14. **Mouse in main pane:** clicks forward to tmux, left-click focuses main
15. **Spinner:** active sessions show animated spinner

- [ ] **Step 3: Commit final state**

If any fixes were needed during testing, commit them:

```bash
git add -A
git commit -m "fix: address issues found during manual testing"
```

If no fixes needed, skip this step.

---

### Task 8: Add unit tests for apply_action

**Files:**
- Modify: `src/action.rs`

Add tests that exercise `apply_action` without any IO dependencies.

- [ ] **Step 1: Add test module to action.rs**

Append to the bottom of `src/action.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, FilterMode, FocusMode, LayoutMode, SessionRow};

    fn make_session(name: &str, idle: u64) -> SessionRow {
        SessionRow {
            name: name.to_string(),
            dir: format!("/tmp/{}", name),
            branch: "main".to_string(),
            ahead: 0,
            behind: 0,
            staged: 0,
            modified: 0,
            untracked: 0,
            is_current: false,
            idle_seconds: idle,
        }
    }

    fn make_test_state(n: usize) -> AppState {
        let mut state = AppState::new(0, LayoutMode::Horizontal, true, 28, 120, 40);
        state.sessions = (0..n)
            .map(|i| make_session(&format!("sess-{}", i), 0))
            .collect();
        if !state.sessions.is_empty() {
            state.sessions[0].is_current = true;
        }
        state.session_order = state.sessions.iter().map(|s| s.name.clone()).collect();
        state.recompute_filter();
        state
    }

    #[test]
    fn focus_next_advances() {
        let mut state = make_test_state(5);
        state.focused = 0;
        apply_action(&mut state, Action::FocusNext);
        assert_eq!(state.focused, 1);
    }

    #[test]
    fn focus_next_stops_at_end() {
        let mut state = make_test_state(5);
        state.focused = 4;
        apply_action(&mut state, Action::FocusNext);
        assert_eq!(state.focused, 4);
    }

    #[test]
    fn focus_prev_decrements() {
        let mut state = make_test_state(5);
        state.focused = 3;
        apply_action(&mut state, Action::FocusPrev);
        assert_eq!(state.focused, 2);
    }

    #[test]
    fn focus_prev_stops_at_zero() {
        let mut state = make_test_state(5);
        state.focused = 0;
        apply_action(&mut state, Action::FocusPrev);
        assert_eq!(state.focused, 0);
    }

    #[test]
    fn focus_index_sets_position() {
        let mut state = make_test_state(5);
        apply_action(&mut state, Action::FocusIndex(3));
        assert_eq!(state.focused, 3);
    }

    #[test]
    fn focus_index_out_of_range_ignored() {
        let mut state = make_test_state(5);
        state.focused = 2;
        apply_action(&mut state, Action::FocusIndex(10));
        assert_eq!(state.focused, 2);
    }

    #[test]
    fn kill_session_requires_confirmation() {
        let mut state = make_test_state(3);
        state.focused = 1;
        let fx = apply_action(&mut state, Action::KillSession);
        assert!(state.confirm_kill);
        assert!(fx.kill_session.is_none());
    }

    #[test]
    fn kill_single_session_prevented() {
        let mut state = make_test_state(1);
        apply_action(&mut state, Action::KillSession);
        assert!(!state.confirm_kill);
    }

    #[test]
    fn confirm_kill_returns_side_effect() {
        let mut state = make_test_state(3);
        state.focused = 1;
        state.confirm_kill = true;
        let fx = apply_action(&mut state, Action::ConfirmKill);
        assert!(!state.confirm_kill);
        assert!(fx.kill_session.is_some());
        let kill = fx.kill_session.unwrap();
        assert_eq!(kill.name, "sess-1");
        assert!(kill.switch_to.is_none()); // not current session
    }

    #[test]
    fn confirm_kill_current_session_provides_switch_target() {
        let mut state = make_test_state(3);
        state.sessions[1].is_current = true;
        state.sessions[0].is_current = false;
        state.focused = 1;
        state.confirm_kill = true;
        let fx = apply_action(&mut state, Action::ConfirmKill);
        let kill = fx.kill_session.unwrap();
        assert_eq!(kill.name, "sess-1");
        assert!(kill.switch_to.is_some());
    }

    #[test]
    fn cancel_kill_clears_flag() {
        let mut state = make_test_state(3);
        state.confirm_kill = true;
        apply_action(&mut state, Action::CancelKill);
        assert!(!state.confirm_kill);
    }

    #[test]
    fn cycle_filter_rotates() {
        let mut state = make_test_state(3);
        assert_eq!(state.filter_mode, FilterMode::All);
        apply_action(&mut state, Action::CycleFilter);
        assert_eq!(state.filter_mode, FilterMode::Working);
        apply_action(&mut state, Action::CycleFilter);
        assert_eq!(state.filter_mode, FilterMode::Idle);
        apply_action(&mut state, Action::CycleFilter);
        assert_eq!(state.filter_mode, FilterMode::All);
    }

    #[test]
    fn toggle_layout_flips_and_signals_resize() {
        let mut state = make_test_state(1);
        assert_eq!(state.layout_mode, LayoutMode::Horizontal);
        let fx = apply_action(&mut state, Action::ToggleLayout);
        assert_eq!(state.layout_mode, LayoutMode::Vertical);
        assert!(fx.resize_pty);
        assert!(fx.save_config);
    }

    #[test]
    fn toggle_borders_signals_resize_and_save() {
        let mut state = make_test_state(1);
        let was = state.show_borders;
        let fx = apply_action(&mut state, Action::ToggleBorders);
        assert_ne!(state.show_borders, was);
        assert!(fx.resize_pty);
        assert!(fx.save_config);
    }

    #[test]
    fn cycle_theme_wraps() {
        let mut state = make_test_state(1);
        let theme_count = THEMES.len();
        state.theme_index = theme_count - 1;
        let fx = apply_action(&mut state, Action::CycleTheme);
        assert_eq!(state.theme_index, 0);
        assert!(fx.save_config);
    }

    #[test]
    fn toggle_focus() {
        let mut state = make_test_state(1);
        assert_eq!(state.focus_mode, FocusMode::Main);
        apply_action(&mut state, Action::ToggleFocus);
        assert_eq!(state.focus_mode, FocusMode::Sidebar);
        apply_action(&mut state, Action::ToggleFocus);
        assert_eq!(state.focus_mode, FocusMode::Main);
    }

    #[test]
    fn switch_project_returns_session_name() {
        let mut state = make_test_state(3);
        state.focused = 2;
        let fx = apply_action(&mut state, Action::SwitchProject);
        assert_eq!(fx.switch_session.as_deref(), Some("sess-2"));
        assert!(fx.refresh_sessions);
    }

    #[test]
    fn quit_signals_quit() {
        let mut state = make_test_state(1);
        let fx = apply_action(&mut state, Action::Quit);
        assert!(fx.quit);
    }

    #[test]
    fn dismiss_help() {
        let mut state = make_test_state(1);
        state.show_help = true;
        apply_action(&mut state, Action::DismissHelp);
        assert!(!state.show_help);
    }

    #[test]
    fn open_and_navigate_context_menu() {
        let mut state = make_test_state(3);
        apply_action(&mut state, Action::OpenSessionMenu { filtered_idx: 1, x: 10, y: 5 });
        assert!(state.context_menu.is_some());
        assert_eq!(state.focused, 1);

        apply_action(&mut state, Action::MenuNext);
        assert_eq!(state.context_menu.as_ref().unwrap().selected, 1);

        apply_action(&mut state, Action::MenuPrev);
        assert_eq!(state.context_menu.as_ref().unwrap().selected, 0);

        apply_action(&mut state, Action::MenuDismiss);
        assert!(state.context_menu.is_none());
    }

    #[test]
    fn resize_signals_pty_resize() {
        let mut state = make_test_state(1);
        let fx = apply_action(&mut state, Action::Resize(200, 50));
        assert_eq!(state.term_width, 200);
        assert_eq!(state.term_height, 50);
        assert!(fx.resize_pty);
    }

    #[test]
    fn reorder_session_moves_up() {
        let mut state = make_test_state(3);
        state.focused = 1;
        apply_action(&mut state, Action::ReorderSession(-1));
        // sess-1 should now be at position 0
        assert_eq!(state.sessions[0].name, "sess-1");
        assert_eq!(state.sessions[1].name, "sess-0");
        assert_eq!(state.focused, 0);
    }

    #[test]
    fn reorder_only_in_all_filter() {
        let mut state = make_test_state(3);
        state.filter_mode = FilterMode::Working;
        state.focused = 1;
        let original_order: Vec<String> = state.sessions.iter().map(|s| s.name.clone()).collect();
        apply_action(&mut state, Action::ReorderSession(-1));
        let new_order: Vec<String> = state.sessions.iter().map(|s| s.name.clone()).collect();
        assert_eq!(original_order, new_order);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test 2>&1`
Expected: all tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/action.rs
git commit -m "test: add unit tests for apply_action"
```

---

### Task 9: Clean up and final commit

**Files:**
- Possibly modify: `src/state.rs`, `src/action.rs`, `src/app.rs`

- [ ] **Step 1: Remove dead code warnings**

Run: `cargo check 2>&1 | grep warning`

Fix any remaining warnings (unused imports, dead code). Common fixes:
- Remove unused `use` statements
- Add `#[allow(dead_code)]` only if the code is intentionally kept for future use

- [ ] **Step 2: Final build and test**

Run: `cargo test 2>&1 && cargo build --release 2>&1`
Expected: all tests pass, release build succeeds.

- [ ] **Step 3: Commit any cleanup**

```bash
git add -A
git commit -m "refactor: clean up warnings after Elm-style refactor"
```

- [ ] **Step 4: Push**

```bash
git push
```

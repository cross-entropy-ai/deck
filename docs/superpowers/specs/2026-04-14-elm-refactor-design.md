# Elm-Style Refactor: Action Enum + Pure Functions

**Goal:** Refactor app.rs (1142 lines) into three files with clear separation: event mapping, state management, and IO shell. Pure internal refactor — no external behavior changes.

**Motivation:** Readability (app.rs too large, mixed concerns) + testability (business logic untestable without PTY/terminal).

---

## File Structure

```
src/
  action.rs    — Action enum + key_to_action() / mouse_to_action() pure functions (~150 lines)
  state.rs     — AppState struct + apply_action() pure function (~450 lines)
  app.rs       — App struct: event loop, PTY, render (~500 lines)
```

All other files unchanged.

---

## Action Enum

Fine-grained, one variant per user action:

```rust
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

    // Resize
    ResizeSidebar(u16),
    ResizeSidebarHeight(u16),
    StartDrag,
    StopDrag,
    SetHoverSeparator(bool),

    // Terminal
    Resize(u16, u16),

    // PTY passthrough
    ForwardKey(Vec<u8>),
    ForwardMouse(Vec<u8>),

    // Lifecycle
    Quit,

    // No-op
    None,
}
```

## Event Mapping (Pure Functions)

```rust
pub fn key_to_action(key: &KeyEvent, state: &AppState) -> Action
pub fn mouse_to_action(mouse: &MouseEvent, state: &AppState) -> Action
```

Read-only access to state for mode-dependent mapping (sidebar vs main, context menu open, help showing, etc). No side effects.

## AppState

All state that `apply_action` reads and writes:

```rust
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
```

Excludes: `pty`, `parser`, `spinner` (remain in App).

## apply_action (Pure Function)

```rust
pub struct SideEffect {
    pub switch_session: Option<String>,
    pub kill_session: Option<String>,
    pub create_session: bool,
    pub resize_pty: bool,
    pub save_config: bool,
    pub quit: bool,
}

pub fn apply_action(state: &mut AppState, action: Action) -> SideEffect
```

Modifies state, returns side effects to execute. Does not perform IO.

## App (Thin Shell)

```rust
pub struct App {
    state: AppState,
    pty: Pty,
    parser: vt100::Parser,
    spinner: Rattler<Dots>,
}
```

Event loop: `poll -> map -> apply -> execute`. Render reads `state` + `parser`. Side effect execution (tmux calls, PTY writes, config saves) happens here.

## Testing

`apply_action` is fully testable without PTY or terminal:

```rust
#[test]
fn focus_next_stops_at_end() {
    let mut state = make_test_state(5);
    state.focused = 4;
    apply_action(&mut state, Action::FocusNext);
    assert_eq!(state.focused, 4);
}

#[test]
fn kill_requires_confirmation() {
    let mut state = make_test_state(3);
    state.focused = 1;
    let fx = apply_action(&mut state, Action::KillSession);
    assert!(state.confirm_kill);
    assert!(fx.kill_session.is_none());
}
```

`key_to_action` and `mouse_to_action` are also testable — construct a KeyEvent/MouseEvent, pass a state, assert the returned Action.

## Constraints

- No external behavior changes
- All existing keybindings, mouse interactions, context menus preserved exactly
- Enums (LayoutMode, FocusMode, FilterMode, SessionRow, ContextMenu, MenuKind) move to state.rs
- ui.rs unchanged (already pure functions)

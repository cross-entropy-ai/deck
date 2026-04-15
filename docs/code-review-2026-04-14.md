# Code Review: deck

**Date**: 2026-04-14
**Scope**: Full codebase (~3900 LOC, 13 source files)
**Focus**: KISS, SOLID, structure clarity

---

## Overall Assessment

This is a well-structured project. The Elm-like architecture (State → Action → SideEffect) is clean, module boundaries are clear, and the code is readable. For ~3900 LOC, it's quite disciplined. Below are the areas where I see genuine optimization space.

---

## SOLID Issues

### 1. `draw_sidebar()` violates DRY — bordered/non-bordered paths are near-identical

`src/ui.rs:27-134` — the bordered and non-bordered branches repeat the same header/sessions/footer layout logic. The only difference is wrapping in a `Block`.

```rust
// Current: two parallel branches with identical structure
if show_borders {
    let block = Block::default()...;
    let content = block.inner(area);
    frame.render_widget(block, area);
    // draw_header, draw_sessions, draw_footer  ← same
    return;
}
// draw_header, draw_sessions, draw_footer  ← same again
```

Can be collapsed to:

```rust
let content = if show_borders {
    let block = Block::default()...;
    let c = block.inner(area);
    frame.render_widget(block, area);
    c
} else {
    frame.render_widget(Block::default().style(...), area);
    area
};
// draw_header, draw_sessions, draw_footer — once
```

### 2. Menu items use magic indices — Open/Closed violation

`src/action.rs:308-367` — `MenuConfirm` dispatches on `menu.selected` with raw `0, 1, 2, 3, 4`. Adding a menu item requires editing both the `SESSION_MENU_ITEMS`/`GLOBAL_MENU_ITEMS` arrays AND these match arms in perfect sync. This is fragile:

```rust
MenuKind::Session { filtered_idx } => {
    match menu.selected {
        0 => { /* Switch */ }
        1 => { /* Kill */ }
        2 => { /* Move up */ }
        3 => { /* Move down */ }
        ...
    }
}
```

Better: match on the item string, or define an enum for menu actions.

### 3. `AppState` fields are all `pub` — Interface Segregation issue

`src/state.rs:126-154` — every field is `pub`, so `action.rs`, `app.rs`, and `ui.rs` all have full read/write access. `action.rs` legitimately needs to mutate state, but `ui.rs` should only need read access. For a project this size it's workable, but a `pub(crate)` + accessor methods approach would make mutation boundaries explicit.

### 4. `session_at_col()` builds a full `Vec<SessionView>` just for hit-testing

`src/state.rs:276-305` — on every mouse move event, this allocates a `Vec<SessionView>` containing copies of all filtered sessions, just to call `tab_col_ranges()` which only uses name + index. `tab_col_ranges()` could take `&[(usize, &str)]` or just the session names instead.

---

## Structural Simplifications

### 5. `visible_sessions()` is a zero-value wrapper

`src/app.rs:118-120`:
```rust
fn visible_sessions() -> Vec<tmux::SessionInfo> {
    tmux::list_sessions()
}
```
Just call `tmux::list_sessions()` directly.

### 6. Repeated TTY-aware switch pattern

This pattern appears 3 times (`switch_to_session_if_safe`, `create_new_session`, `current_attached_session`):
```rust
if self.pty.slave_tty.is_empty() {
    tmux::switch_session(name);
} else {
    tmux::switch_client_for_tty(&self.pty.slave_tty, name);
}
```
Could be extracted to a method like `fn switch_client(&self, session: &str)`.

### 7. `ContextMenu.items` allocates owned Strings from static slices

`src/state.rs:73` — `items: Vec<String>`, but the items always come from `&'static str` constants. Every menu open allocates unnecessary Strings. Could be `Vec<&'static str>` or just store a reference to the static slice + the kind (which already tells you which items to use).

### 8. Mouse offset calculation duplicated in `mouse_to_action`

`src/action.rs:631-646` — the border offset + layout-specific offset logic for PTY mouse forwarding is duplicated between the left-click branch and the general forwarding branch. Factor it into a helper.

### 9. `render()` clones too much per frame

`src/app.rs:371-396` — every 16ms frame clones `context_menu`, `warning_state`, `screen_snapshot` (the entire vt100 screen), and builds a full `Vec<SessionRow>`. The screen clone is especially expensive. This is driven by borrow checker constraints (can't borrow `self.state` and `self.parser` simultaneously in the closure), but could potentially be addressed by restructuring the draw call.

### 10. `SIDEBAR_HEIGHT_MIN=3, SIDEBAR_HEIGHT_MAX=4` — over-parametrized

`src/state.rs:9-11` — three constants for a range of {3, 4}. The sidebar height resize mechanism barely does anything. Either expand the range or simplify to a fixed value.

---

## What's Already Good

- **SideEffect pattern**: separating state mutation from IO is the right call. The reducer returns a description of what to do, and `execute_side_effects` does the IO. This is clean and testable.
- **Small modules**: `git.rs` (75 LOC), `bridge.rs` (69 LOC), `config.rs` (90 LOC) each do one thing well.
- **Hand-rolled JSON parser**: fits the KISS principle — avoids pulling in serde for a 4-field flat config.
- **Test coverage**: 27 tests on the reducer, 3 on the instance guard, 3 on nesting guard — the critical business logic is tested.
- **`SessionView` vs `SessionRow`**: clean owned-vs-borrowed boundary between state and UI.
- **NestingGuard**: the safety mechanism for preventing recursive deck is well-isolated.

---

## Priority Ranking

If I were to address these, in order of impact:

1. **Collapse `draw_sidebar` duplication** (#1) — ~30 lines removed, clearer structure
2. **Extract TTY-aware switch method** (#6) — removes 3x duplication
3. **Fix menu magic indices** (#2) — prevents bugs when menu items change
4. **Remove `visible_sessions` wrapper** (#5) — trivial cleanup
5. **Avoid screen clone per frame** (#9) — performance win for the render loop

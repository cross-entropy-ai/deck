# Compact Mode

## Summary

Add a compact view mode that renders each session card in 2 lines instead of 5, allowing 2-3x more sessions visible on screen. Toggled via Settings UI row or `c` shortcut. Persisted to config.

## View Modes

**Expanded (current, 5 lines/card):**
```
 ▌⠋ 1  my-project
  1m   ~/code/my-project
       main
      ↑2 ↓1 ~3
 
```

**Compact (2 lines/card):**
```
 ▌⠋ 1  my-project  main ↑2 ↓1 ~3
       ~/code/my-project
```

Line 1: accent + activity icon + index + name + branch + git status (reuse `format_git_status` with compact=true)
Line 2: directory path (reuse `shorten_dir` + `truncate`)

In compact mode, idle badge moves inline: `format_activity_compact` already exists and returns spinner or `1m`/`2h`/`2d`.

## Data Model

### ViewMode enum (`state.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Expanded,
    Compact,
}
```

Add `view_mode: ViewMode` to `AppState`. Default: `Expanded`.

### Config (`config.rs`)

Add `view_mode: String` field. Values: `"expanded"` (default), `"compact"`. Parse/serialize in `parse_json`/`to_json`.

### CARD_HEIGHT

Replace the single `pub const CARD_HEIGHT: usize = 5` with a function:

```rust
pub fn card_height(view_mode: ViewMode) -> usize {
    match view_mode {
        ViewMode::Expanded => 5,
        ViewMode::Compact => 2,
    }
}
```

All callers of `CARD_HEIGHT` (ui.rs `scroll_offset`, state.rs `session_at_row`) need to receive `view_mode` and call `card_height()`.

## Rendering

### `draw_sessions` (ui.rs)

Add `view_mode: ViewMode` parameter. Dispatch to existing logic for Expanded, new `draw_sessions_compact` for Compact.

### `draw_sessions_compact` (ui.rs)

For each session, render 2 lines:

**Line 1:** `accent` + `activity_icon` + `index` + `name` + `branch` + `git_status_compact`

- Reuse the same accent/icon/index/name styling from expanded mode
- Append branch in `theme.pink`/`theme.muted` (same colors as expanded row 3)
- Append `format_git_status(session, true)` — the compact variant already exists and uses no-space separators
- If `session.branch.is_empty()`, skip branch and git status

**Line 2:** `"      "` + directory (same as expanded row 2, reuse `shorten_dir`/`truncate`)

No blank separator line between cards (that's what makes it compact).

### `scroll_offset` (ui.rs)

Change signature to `fn scroll_offset(focused: usize, visible_height: u16, card_height: usize) -> usize`. Pass `card_height(view_mode)`.

### `session_at_row` (state.rs)

Already uses `CARD_HEIGHT` constant. Change to call `card_height(self.view_mode)` (needs `ViewMode` import from state, which is local).

## Settings UI

### Settings row

Add a 5th entry (before Exclude, after Borders):

| Label | Value | Help |
|-------|-------|------|
| View | Compact / Expanded | Left/right toggles view mode |

`SETTINGS_ITEM_COUNT`: `4` → `5`.

Exclude editor moves to index 4 (was 3).

### SettingsView

Add `view_mode: ViewMode` to `SettingsView` struct.

## Actions

New action: `ToggleViewMode` — flips between Expanded and Compact, triggers `save_config`.

In `SettingsAdjust` handler: index 3 → `ToggleViewMode`. Index 4 → `OpenExcludeEditor` (shifted from 3).

Sidebar shortcut: `c` key → `ToggleViewMode`.

## Vertical tab mode

Compact mode only affects horizontal layout (card-based sidebar). Vertical tab mode is unaffected — it already has its own compact tab rendering.

Pass `view_mode` to `draw_sessions` only. The tab path (`draw_sidebar_tabs`) ignores it.

## Testing

- Unit test `card_height()` returns correct values
- Unit test `ToggleViewMode` action flips state and triggers save_config
- Unit test `session_at_row` works with both view modes
- Unit test `SettingsAdjust(1)` at index 3 toggles view mode (not opens exclude editor)

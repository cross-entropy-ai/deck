# Session/Window Tree View

## Goal

Show tmux windows under each session in the sidebar as a collapsible tree, allowing users to navigate and switch to specific windows.

## Data Layer

### New tmux functions

- `tmux::list_windows(session_name) -> Vec<WindowInfo>` — calls `tmux list-windows -t <session> -F "#{window_index}\t#{window_name}\t#{window_active}"` and returns a list of `WindowInfo { index: u32, name: String, is_active: bool }`.
- `tmux::select_window(target: &str)` — calls `tmux select-window -t <target>` where target is `session:window_index`. Combined with `switch_session` (or `switch-client`) to jump to a specific window in a specific session.

### Data refresh

`refresh_sessions` already calls `tmux list-windows -a` for activity timestamps. Extend this to also capture window name and active flag per session, or make a second pass with `list_windows` per session. The per-session approach is simpler and the window count is small enough that the overhead is negligible at 1s refresh intervals.

## State Layer

### New types

```rust
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub index: u32,
    pub name: String,
    pub is_active: bool,
}
```

### SessionRow changes

Add `windows: Vec<WindowInfo>` field to `SessionRow`. Populated during `refresh_sessions`.

### AppState changes

- Add `expanded: HashSet<usize>` — stores indices into `sessions` (not filtered indices) for sessions whose window tree is expanded.
- The existing `focused: usize` continues to point into a logical "visible rows" list. A visible row is either a session or a window belonging to an expanded session.

### Navigation model

Introduce a helper to map between `focused` index and the underlying session/window:

```rust
enum TreeNode {
    Session(usize),           // filtered index
    Window(usize, usize),     // filtered index, window vec index
}
```

`AppState::visible_nodes(&self) -> Vec<TreeNode>` builds the flat list by iterating filtered sessions, inserting window nodes for expanded ones. `focused` indexes into this list.

## Interaction

| Key | On Session | On Window |
|-----|-----------|-----------|
| Enter | Switch to session | Switch to session + select window |
| Right / Tab | Expand (show windows) | No-op |
| Left | Collapse (if expanded), else no-op | Jump to parent session + collapse |
| Tab | Toggle expand/collapse | Jump to parent session + collapse |
| j/k | Move down/up through visible nodes | Same |

When a session is collapsed, any focused window underneath it moves focus back to the session row.

## Rendering

### Expanded mode (4-line session card)

Windows appear as 1-line rows directly after the session card:

```
▌ 1  my-project
   ~/code/my-project
    main +2 ~1
    ├ 1: editor *
    ├ 2: server
    └ 3: logs
  2  other-project
   ~/code/other
    main
```

### Compact mode (2-line session card)

Same tree lines after the compact card:

```
▌ 1  my-project    main +2
    ├ 1: editor *
    └ 2: server
  2  other-project  main
```

### Window row styling

- Indent: 4 chars (`    `)
- Tree connector: `├` for non-last, `└` for last window
- Format: `{connector} {index}: {name}` with optional ` *` for active window
- Focused window row gets `theme.surface` background, same as focused session
- Active window marker `*` in `theme.accent` color
- Window name in `theme.secondary`, index in `theme.dim`

### Expand/collapse indicator

Add a small indicator on the session name line: `▸` (collapsed, has windows) or `▾` (expanded). Sessions with only 1 window don't show the indicator (expanding would be redundant, but still allowed).

## Mouse support

- Click on a window row: switch to that session + window
- Click on a session row: same as before (switch to session)
- No special click target for expand/collapse — use keyboard or double-click (stretch goal, not in v1)

## Config persistence

No config changes needed. Expand/collapse state is ephemeral (resets on restart). This matches the behavior of most tree views.

## Scope

This spec covers horizontal sidebar layout only. Vertical/tabs mode does not show the tree (tabs are too compact for a tree structure).

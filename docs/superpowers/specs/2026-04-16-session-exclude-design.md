# Session Exclude Patterns

## Summary

Allow users to configure patterns that exclude matching tmux sessions from the sidebar. Patterns are persisted in `~/.config/deck/config.json`. Supports glob patterns and regex (wrapped in `//`).

Replaces the current hardcoded `_` prefix filter in the `App::refresh_sessions()` session filter logic with a user-configurable default pattern `_*`.

## Pattern Format

- **Glob**: `_*`, `scratch*`, `temp-*` — standard glob matching against session name
- **Regex**: `/^test-.+$/`, `/scratch/` — regex matching, delimited by `//`

Matching is against the full session name.

## Config

Add `exclude_patterns` array to `config.json`:

```json
{
  "theme": "Catppuccin Latte",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 51,
  "exclude_patterns": ["_*"]
}
```

Default value: `["_*"]` (preserves existing behavior).

## Data Flow

### Config layer (`config.rs`)

- Add `exclude_patterns: Vec<String>` to `Config` struct, default `vec!["_*".to_string()]`
- Parse from JSON array in `parse_json()` — handle the array manually since the parser is line-based
- Serialize in `save()` — write as JSON array

### App layer (`app.rs`)

- Store `exclude_patterns: Vec<String>` in `AppState`
- `refresh_sessions()`: replace the hardcoded `!s.name.starts_with('_')` filter with pattern matching against `exclude_patterns`
- `save_config()`: include `exclude_patterns` in the saved config

### Pattern matching (`config.rs`)

Add a function `fn session_excluded(name: &str, patterns: &[String]) -> bool`:
- For each pattern:
  - If wrapped in `//`: strip delimiters, compile as `regex::Regex`, test against name
  - Otherwise: use simple glob matching (support `*` and `?` wildcards)
- Return true if any pattern matches

For glob matching, implement a minimal matcher inline (only `*` and `?`) rather than pulling in a glob crate — session names are simple strings.

For regex, add the `regex` crate as a direct dependency (not currently in the tree).

Cache compiled regexes: compile patterns once when config loads or patterns change, store alongside the raw strings in `AppState`.

### Compiled pattern storage

```rust
pub enum ExcludePattern {
    Glob(String),
    Regex(regex::Regex),
}
```

Store `Vec<ExcludePattern>` in `AppState`. Recompile when patterns are added/removed.

## Settings UI

### Settings page (`ui.rs`)

Add a 4th settings entry:

| Label | Value | Help |
|-------|-------|------|
| Exclude | `3 patterns` | Enter opens the pattern list |

`SETTINGS_ITEM_COUNT` in `state.rs`: change from `3` to `4`.

### Exclude editor popup (`ui.rs`)

When the user presses Enter on the Exclude row, open a popup (similar to theme picker):

```
┌─ Exclude Patterns ─────────┐
│  _*                         │
│  scratch*                   │
│  /^test-.+$/                │
│                             │
│  a: add  d: delete  Esc: close │
└─────────────────────────────┘
```

- Up/Down (j/k): navigate patterns
- `a`: enter add mode — show an inline text input at the bottom of the list
- `d` or `x`: delete selected pattern (immediate, no confirm)
- Enter (in add mode): commit the new pattern, validate regex if `//`-wrapped
- Esc: close popup (or cancel add mode if active)

### State additions (`state.rs`)

```rust
pub struct ExcludeEditorState {
    pub selected: usize,
    pub adding: bool,
    pub input: String,
    pub cursor: usize,
    pub error: Option<String>,  // shown when regex is invalid
}
```

Add to `AppState`:
- `exclude_patterns: Vec<String>` (raw pattern strings for config persistence)
- `compiled_patterns: Vec<ExcludePattern>` (compiled for matching)
- `exclude_editor: Option<ExcludeEditorState>` (None when popup closed)

### Actions (`action.rs`)

New actions:
- `OpenExcludeEditor`
- `CloseExcludeEditor`
- `ExcludeEditorNext` / `ExcludeEditorPrev`
- `ExcludeEditorStartAdd`
- `ExcludeEditorDelete`
- `ExcludeEditorInput(char)` / `ExcludeEditorBackspace` / `ExcludeEditorConfirm`

On add/delete: recompile patterns, trigger `save_config` and `refresh_sessions`.

## Key Binding Integration

In `map_key_settings()` (action.rs), when `exclude_editor` is `Some`:
- Route j/k/up/down to editor navigation
- Route `a`, `d`, `x`, Esc, Enter to editor actions
- When `adding` is true, route character keys to input

This takes priority over normal settings key handling when the popup is open.

## Error Handling

- Invalid regex (bad syntax in `//`): show error message in the popup, don't add the pattern
- Empty pattern: ignore, don't add
- Duplicate pattern: allow (not worth the complexity to prevent)

## Testing

- Unit test `session_excluded()` with glob and regex patterns
- Unit test pattern compilation (valid regex, invalid regex, glob)
- Action tests for add/delete pattern flow

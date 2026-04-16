# Customizable Keybindings — Design

**Status**: draft
**Date**: 2026-04-17

## Goal

Let users rebind the sidebar keys (and the global `Ctrl+S` focus toggle) via `~/.config/deck/config.json`. Modal keys (rename, confirm kill, context menu, theme picker, exclude editor) and number-jump keys (`1-9`) remain hardcoded.

## Non-goals

- Rebinding modal keys (y/n, Esc in rename, menu j/k, etc.) — these are conventions and rebinding them risks locking users out of modes.
- Rebinding number-jump keys (`1-9`) — the range model doesn't fit a single-key binding.
- In-app editing of bindings — users edit `config.json` directly. A read-only viewer is provided in Settings.
- Live reload — changes take effect after restart.

## Scope

### Bindable commands

The command set is derived from `sidebar_key_to_action` + the global `Ctrl+S` handler in `key_to_action`.

| Command            | Default key(s) | Underlying `Action`    |
| ------------------ | -------------- | ---------------------- |
| `focus_next`       | `j`, `Down`    | `FocusNext`            |
| `focus_prev`       | `k`, `Up`      | `FocusPrev`            |
| `switch_project`   | `Enter`        | `SwitchProject`        |
| `kill_session`     | `x`            | `KillSession`          |
| `reorder_up`       | `A-Up`         | `ReorderSession(-1)`   |
| `reorder_down`     | `A-Down`       | `ReorderSession(1)`    |
| `cycle_filter`     | `f`            | `CycleFilter`          |
| `open_settings`    | `t`            | `OpenSettings`         |
| `toggle_borders`   | `b`            | `ToggleBorders`        |
| `toggle_layout`    | `l`            | `ToggleLayout`         |
| `toggle_view_mode` | `c`            | `ToggleViewMode`       |
| `toggle_help`      | `h`, `?`       | `ToggleHelp`           |
| `focus_main`       | `Esc`          | `SetFocusMain`         |
| `quit`             | `q`            | `Quit`                 |
| `toggle_focus`     | `C-s`          | `ToggleFocus` (global) |

`toggle_focus` is special: it fires in both `FocusMode::Main` and `FocusMode::Sidebar`. All other commands only fire when the sidebar owns focus.

## Config format

New top-level field `keybindings` on `Config`:

```json
{
  "theme": "Catppuccin Mocha",
  "keybindings": {
    "kill_session": "X",
    "toggle_help": ["h", "?", "F1"],
    "toggle_borders": null
  }
}
```

- Field is `#[serde(default)]`; absent → defaults for every command.
- Value types per command:
  - `String` — single binding
  - `[String, ...]` — multiple bindings (both keys invoke the command)
  - `null` — explicit unbind, removes the command's default
- Commands not mentioned keep their defaults (merge semantics — matches Q4a).
- **Replace, not append**: when a user writes `"kill_session": "X"`, this *replaces* the default binding(s) for that command. To keep `x` and add `X`, the user writes `["x", "X"]`. This keeps the mental model simple: the value you write for a command is the complete set of keys for that command.
- Unknown command name: stderr warning, entry ignored.
- Unparseable key string: stderr warning, that specific string ignored.

### Key string grammar

Parsed by `parse_key(s: &str) -> Result<KeyBinding, ParseError>`:

- **Plain characters**: `"j"`, `"1"`, `"?"`, `" "` (space also accepts `"Space"`).
- **Named keys**: `Enter`, `Esc`, `Up`, `Down`, `Left`, `Right`, `Tab`, `Space`, `Backspace`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, `F1`–`F12`. Named keys are matched case-insensitively on input but normalized to TitleCase in `format_key` output.
- **Modifier prefixes** (combinable): `C-` (Ctrl), `A-` (Alt), `S-` (Shift).
- **Order insensitivity**: `"A-C-x"` and `"C-A-x"` parse identically; `format_key` emits canonical order `C-A-S-<key>`.
- **Shift/Case**: `"J"` and `"S-j"` both parse to `KeyCode::Char('J')` with `KeyModifiers::SHIFT` — the two forms are equivalent and hash to the same `KeyBinding`.

### Runtime key normalization

`KeyEvent` → `KeyBinding` conversion at lookup time must match what `parse_key` produces, otherwise a parsed binding won't match an actual keystroke. Two normalization rules apply to both paths:

1. For `KeyCode::Char(c)` where `c` is ASCII uppercase: always set the result to `KeyCode::Char(c), modifiers | SHIFT`. (Some terminals emit `Char('J'), NONE` for Shift+j; others emit `Char('J'), SHIFT`. Normalizing to "always include SHIFT" makes both cases match `parse("J")`.)
2. Strip any `KeyModifiers` bits outside the set `{CTRL, ALT, SHIFT}` — e.g., some terminals include `KEYPAD` or `NONE` flag variants that we don't care about.

Both `parse_key` and `Keybindings::lookup` apply these normalizations before hashing.

## Runtime types

New file `src/keybindings.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    FocusNext, FocusPrev, SwitchProject, KillSession,
    ReorderUp, ReorderDown, CycleFilter, OpenSettings,
    ToggleBorders, ToggleLayout, ToggleViewMode, ToggleHelp,
    FocusMain, Quit, ToggleFocus,
}

pub struct Keybindings {
    map: HashMap<KeyBinding, Command>,
    reverse: HashMap<Command, Vec<KeyBinding>>,  // preserves insertion order for display
}

impl Keybindings {
    pub fn from_config(raw: &KeybindingsConfig, plugins: &[PluginConfig]) -> (Self, Vec<Warning>);
    pub fn lookup(&self, key: &KeyEvent) -> Option<Command>;
    pub fn keys_for(&self, cmd: Command) -> &[KeyBinding];
    pub fn is_global(cmd: Command) -> bool;  // only ToggleFocus today
}

impl Command {
    pub fn name(self) -> &'static str;          // "focus_next"
    pub fn description(self) -> &'static str;   // "navigate"
    pub const ALL: &'static [Command];          // fixed display order
}

pub fn parse_key(s: &str) -> Result<KeyBinding, ParseError>;
pub fn format_key(kb: &KeyBinding) -> String;
```

`KeybindingsConfig` (in `config.rs`) is a thin serde-friendly wrapper:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct KeybindingsConfig(pub HashMap<String, KeyBindingValue>);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyBindingValue {
    Single(String),
    Multi(Vec<String>),
    Unbind,  // serialized as `null` via custom handling or Option
}
```

Implementation detail: `null` in JSON is most naturally handled as `Option<KeyBindingValueInner>` where `None` means unbind. The exact serde shape is a small implementation choice; the observable behavior is: string, array of strings, or `null`.

## Conflict resolution

`from_config` runs these passes in order:

1. **Build default table**: start with the hardcoded default binding for every command.
2. **Apply user overrides**: for each user entry:
   - Unknown command → warn, skip.
   - `null` → remove that command's default bindings from the table.
   - `String`/`[String]` → remove that command's default bindings, then parse each string (bad strings → warn, skip), then add all successful parses for that command.
3. **Detect binding→command collisions**: after all overrides applied, scan the table. If a `KeyBinding` maps to more than one command, keep the lexicographically first command name, drop the rest, and emit one warning per dropped entry.
4. **Detect plugin collisions**: for each `PluginConfig.key`, construct the equivalent `KeyBinding` (`KeyCode::Char(key), NONE`). If present in the table, remove it and warn. Plugins win.

Warnings are accumulated and returned alongside the `Keybindings` struct. The caller (`App::run` / `AppState::new`) prints them to stderr once at startup.

## Integration into `action.rs`

### Sidebar path

```rust
fn sidebar_key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    // modal early-exits (help, confirm_kill) unchanged

    // Hardcoded: number jump 1-9 (non-Alt)
    if let KeyCode::Char(c @ '1'..='9') = key.code {
        if !key.modifiers.contains(KeyModifiers::ALT) {
            let idx = (c as usize) - ('1' as usize);
            if idx < state.filtered.len() {
                return Action::NumberKeyJump(idx);
            }
        }
    }

    // Bindable commands
    if let Some(cmd) = state.keybindings.lookup(key) {
        if let Some(action) = command_to_action(cmd, state) {
            return action;
        }
    }

    // Plugin keys (unchanged fallback)
    if let KeyCode::Char(ch) = key.code {
        if let Some(idx) = state.plugins.iter().position(|p| p.key == ch) {
            return Action::ActivatePlugin(idx);
        }
    }

    Action::None
}

fn command_to_action(cmd: Command, state: &AppState) -> Option<Action> {
    Some(match cmd {
        Command::FocusNext => Action::FocusNext,
        Command::FocusPrev => Action::FocusPrev,
        Command::SwitchProject => Action::SwitchProject,
        Command::KillSession => Action::KillSession,
        Command::ReorderUp => Action::ReorderSession(-1),
        Command::ReorderDown => Action::ReorderSession(1),
        Command::CycleFilter => Action::CycleFilter,
        Command::OpenSettings => Action::OpenSettings,
        Command::ToggleBorders => Action::ToggleBorders,
        Command::ToggleLayout => Action::ToggleLayout,
        Command::ToggleViewMode => Action::ToggleViewMode,
        Command::ToggleHelp => Action::ToggleHelp,
        Command::FocusMain => Action::SetFocusMain,
        Command::Quit => Action::Quit,
        Command::ToggleFocus => Action::ToggleFocus,
    })
}
```

### Global path (replaces current `Ctrl+S` check in `key_to_action`)

Before the `renaming` / `context_menu` / settings branches, check global commands:

```rust
pub fn key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    // Rename / menu / settings branches still take priority (unchanged)
    if state.renaming.is_some() { ... }
    if state.context_menu.is_some() { ... }

    // Global commands (only ToggleFocus today)
    if let Some(cmd) = state.keybindings.lookup(key) {
        if Keybindings::is_global(cmd) {
            return command_to_action(cmd, state).unwrap_or(Action::None);
        }
    }

    if state.main_view == MainView::Settings && state.focus_mode == FocusMode::Main { ... }

    match state.focus_mode { ... }
}
```

Observation: this subtly changes semantics — previously `Ctrl+S` was hardcoded at the top of `key_to_action`, taking priority over everything including settings sub-modes. The new design keeps global-command checking above the settings branch but below modal (rename/menu) branches, which matches the original behavior. If a user rebinds `toggle_focus` to, say, `C-b`, it works globally just like `Ctrl+S` did.

## AppState changes

Add fields:

```rust
pub struct AppState {
    // ... existing fields ...
    pub keybindings: Keybindings,
    pub keybindings_view_open: bool,
    pub keybindings_view_scroll: u16,
}
```

`AppState::new` gains a `keybindings: Keybindings` parameter; `App::run` constructs it from `Config` at startup and passes it in.

## UI changes

### 1. Help page auto-generation (`ui.rs::draw_help`)

Rewrite to iterate `Command::ALL` and call `state.keybindings.keys_for(cmd)`:

- For each command with ≥1 binding: format keys as `"j/Down"`-style joined string via `format_key`, display alongside `cmd.description()`.
- Commands with 0 bindings (unbound via `null`): skip entirely — don't show empty rows.
- Additional fixed lines that aren't in the command table stay hardcoded:
  - `1-9  quick jump`
  - `Mouse  click All / Idle / Working tabs`

### 2. New "Keybindings" settings entry

- `SETTINGS_ITEM_COUNT` goes from 5 → 6.
- New settings row: label `Keybindings`, value `View`, help text `view current bindings`.
- `SettingsAdjust` for `settings_selected == 5` dispatches `Action::OpenKeybindingsView`.

### 3. Keybindings view (read-only)

Mirrors the exclude editor pattern but without add/delete:

- Rendered by a new `draw_keybindings_view(frame, area, theme, state)` in `ui.rs`.
- Two columns: command name (left), formatted bindings joined by `, ` (right). `toggle_focus` gets the suffix `(global)`.
- Empty-bound commands shown with `<unbound>` in dim text (differs from help page — here we want discoverability).
- `j`/`k` / `Up`/`Down` scroll when the list exceeds the visible area.
- `Esc` closes → `Action::CloseKeybindingsView`.
- Footer: `Esc to close · edit ~/.config/deck/config.json to change`.

### 4. New actions

```rust
Action::OpenKeybindingsView,
Action::CloseKeybindingsView,
Action::KeybindingsViewScrollUp,
Action::KeybindingsViewScrollDown,
```

`key_to_action` dispatches to a new `keybindings_view_key_to_action` helper when `state.main_view == MainView::Settings && state.keybindings_view_open`.

## Testing

### `keybindings.rs` unit tests

- `parse_key`:
  - Plain: `"j"`, `"?"`, `" "`, `"Space"`
  - Named: `"Enter"`, `"Esc"`, `"A-Up"`, `"C-s"`, `"F1"`, `"PageDown"`
  - Modifier order insensitivity: `parse("A-C-x") == parse("C-A-x")`
  - Shift normalization: `parse("J") == parse("S-j")`
  - Errors: `""`, `"Foo"`, `"C-"`, `"C-Nope"`
- `format_key`: roundtrip property — `parse(format(parse(s).unwrap())) == parse(s).unwrap()` for all valid inputs; canonical modifier order `C-A-S-`.
- `Keybindings::from_config`:
  - Empty config → all defaults present.
  - Single rebind: `{"kill_session": "X"}` → `x` removed, `X` maps to `KillSession`, other defaults intact.
  - Multi: `{"toggle_help": ["h", "?", "F1"]}` → all three resolve.
  - Unbind: `{"toggle_borders": null}` → `b` no longer maps to any command.
  - Unknown command name → warning present, binding table unchanged.
  - Bad key string → warning present, other bindings for same command still applied.
  - Two commands bind same key → lexicographic winner keeps it, warning for loser.
  - Plugin key collision → plugin wins, keybinding removed, warning present.

### `action.rs` integration tests

Extend existing `mod tests` with a helper that builds `AppState` with a specific `Keybindings`:

- Default bindings: regression — `j` → `FocusNext`, `x` → `KillSession`, etc.
- Rebound `kill_session: "X"`: `x` in sidebar with no matching plugin → `Action::None`; `X` → `KillSession`.
- Unbound `toggle_borders`: `b` → `Action::None` in sidebar; settings path (`SettingsAdjust` with `settings_selected == 2`) still toggles borders — proves the modal/settings paths are independent of keybindings.
- Plugin wins: plugin with `key='l'` shadows `toggle_layout`'s default `l`.
- Global `toggle_focus`: `C-s` works in `FocusMode::Main`; rebound to `C-b`, `C-b` works in `Main`.
- Number jump: `1-9` continues to work regardless of any rebindings.
- Modal paths (`renaming.is_some()`, `context_menu.is_some()`, `confirm_kill`): pressing `x` or `j` routes via modal logic, not keybindings.

### `config.rs` serde tests

- `parse_json_with_keybindings_string`, `parse_json_with_keybindings_array`, `parse_json_with_keybindings_null`.
- `parse_json_without_keybindings_uses_empty` (absent → empty map, which means all defaults apply downstream).
- Roundtrip: `Config` → JSON → `Config` preserves all three value shapes.

### Manual QA checklist

1. No config change → all keys behave as before.
2. `{"kill_session": "X"}` → pressing `x` does nothing (no kill prompt), pressing `X` opens kill confirmation.
3. `{"toggle_borders": null}` → pressing `b` does nothing, but Settings → Layout/Borders/etc. menu still toggles borders.
4. Two commands bound to same key via config → stderr shows one warning line at startup, only one command fires.
5. Plugin with `key='b'` alongside default `toggle_borders: 'b'` → pressing `b` activates plugin, stderr warns, `toggle_borders` has no key (or only its alternate bindings if any).
6. Open Settings → navigate to `Keybindings` → view page lists every command with current bindings, `Esc` returns to Settings.
7. After rebinding `kill_session` to `X`, the help overlay (`h`) shows `X  kill session`.
8. Rebinding `toggle_focus` to `C-b` → `C-b` works in both sidebar focus and main focus; `C-s` no longer toggles.
9. Modal safety: inside rename, `x`/`j`/`Esc`/`Enter`/`Backspace` still do their modal things regardless of any `kill_session`/`focus_next`/`focus_main` rebinding.

## File-level change summary

- **New** `src/keybindings.rs` — parsing, `Command` enum, `Keybindings`, conflict detection.
- `src/config.rs` — add `keybindings: KeybindingsConfig` to `Config`; serde shape for `KeyBindingValue`.
- `src/state.rs` — add `keybindings`, `keybindings_view_open`, `keybindings_view_scroll` to `AppState`; bump `SETTINGS_ITEM_COUNT` to 6.
- `src/action.rs` — new `Action::OpenKeybindingsView` / `Close` / `Scroll*`; global-command check at top of `key_to_action`; `sidebar_key_to_action` routes through `state.keybindings.lookup`; `settings_key_to_action` path for the new view; `SettingsAdjust` dispatches item 5 to the new action.
- `src/ui.rs` — rewrite `draw_help` to read from `state.keybindings`; add `draw_keybindings_view`; extend settings list render to include the new row.
- `src/app.rs` — construct `Keybindings` from `Config` at startup, print accumulated warnings to stderr, pass to `AppState::new`.
- `Cargo.toml` — no new dependencies (crossterm types reused; serde already present).

## Open questions / deferred

- **Live reload**: out of scope. Restart is cheap for this tool.
- **Per-context rebinding** (rebinding modal keys): explicitly deferred. If users ask, revisit with a context-scoped config shape.
- **Binding comments in config**: users may want to annotate their bindings. JSON doesn't support comments; deferred unless we switch to TOML/JSONC later.

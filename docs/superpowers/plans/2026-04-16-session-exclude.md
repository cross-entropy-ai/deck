# Session Exclude Patterns Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow users to configure glob/regex patterns that exclude matching tmux sessions from the sidebar, replacing the hardcoded `_` prefix filter.

**Architecture:** Add `exclude_patterns` to Config for persistence, compile patterns into an enum (`Glob`/`Regex`) stored in AppState for matching, add a Settings UI row with a popup editor for managing patterns. The existing line-based JSON parser gets extended to handle arrays.

**Tech Stack:** Rust, `regex` crate (new dependency), ratatui for popup UI

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `Cargo.toml` | Modify | Add `regex` dependency |
| `src/config.rs` | Modify | `exclude_patterns` field, JSON array parse/serialize, `ExcludePattern` enum, `compile_patterns()`, `session_excluded()`, glob matcher |
| `src/state.rs` | Modify | `ExcludeEditorState` struct, new fields on `AppState`, bump `SETTINGS_ITEM_COUNT` |
| `src/action.rs` | Modify | New `Action` variants, `apply_action` handlers, key routing for exclude editor |
| `src/app.rs` | Modify | Load/store patterns, replace hardcoded filter, pass data to `SettingsView`, save patterns |
| `src/ui.rs` | Modify | 4th settings row, `draw_exclude_editor()` popup, update `SettingsView` |

---

### Task 1: Add `regex` dependency and pattern matching core

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/config.rs`

- [ ] **Step 1: Write failing tests for pattern matching**

Add to the bottom of `src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_star_matches_prefix() {
        let patterns = compile_patterns(&["_*".to_string()]);
        assert!(session_excluded("_hidden", &patterns));
        assert!(!session_excluded("visible", &patterns));
    }

    #[test]
    fn glob_question_mark_matches_single_char() {
        let patterns = compile_patterns(&["t?st".to_string()]);
        assert!(session_excluded("test", &patterns));
        assert!(session_excluded("tast", &patterns));
        assert!(!session_excluded("toast", &patterns));
    }

    #[test]
    fn glob_exact_match() {
        let patterns = compile_patterns(&["scratch".to_string()]);
        assert!(session_excluded("scratch", &patterns));
        assert!(!session_excluded("scratch2", &patterns));
    }

    #[test]
    fn regex_pattern_matches() {
        let patterns = compile_patterns(&["/^test-.+$/".to_string()]);
        assert!(session_excluded("test-abc", &patterns));
        assert!(!session_excluded("test-", &patterns));
        assert!(!session_excluded("my-test-abc", &patterns));
    }

    #[test]
    fn regex_partial_match() {
        let patterns = compile_patterns(&["/scratch/".to_string()]);
        assert!(session_excluded("scratch", &patterns));
        assert!(session_excluded("my-scratch-pad", &patterns));
        assert!(!session_excluded("nothere", &patterns));
    }

    #[test]
    fn invalid_regex_skipped() {
        let patterns = compile_patterns(&["/[invalid/".to_string()]);
        assert!(patterns.is_empty());
    }

    #[test]
    fn multiple_patterns_any_match() {
        let patterns = compile_patterns(&["_*".to_string(), "/^test/".to_string()]);
        assert!(session_excluded("_hidden", &patterns));
        assert!(session_excluded("test-thing", &patterns));
        assert!(!session_excluded("keep-me", &patterns));
    }

    #[test]
    fn empty_patterns_excludes_nothing() {
        let patterns = compile_patterns(&[]);
        assert!(!session_excluded("anything", &patterns));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config::tests`
Expected: compilation errors — `compile_patterns`, `session_excluded`, `ExcludePattern` not defined.

- [ ] **Step 3: Add `regex` to Cargo.toml**

In `Cargo.toml`, add under `[dependencies]`:
```toml
regex = "1"
```

- [ ] **Step 4: Implement ExcludePattern, compile_patterns, session_excluded, and glob matcher**

Add to `src/config.rs`, after the `quote()` function and before the `parse_json()` function:

```rust
/// A compiled exclude pattern — either a glob or a regex.
pub enum ExcludePattern {
    Glob(String),
    Regex(regex::Regex),
}

/// Compile raw pattern strings into ExcludePattern values.
/// Patterns wrapped in `/…/` are treated as regex; others as glob.
/// Invalid regexes are silently skipped.
pub fn compile_patterns(raw: &[String]) -> Vec<ExcludePattern> {
    raw.iter()
        .filter_map(|p| {
            if let Some(inner) = p.strip_prefix('/').and_then(|s| s.strip_suffix('/')) {
                regex::Regex::new(inner).ok().map(ExcludePattern::Regex)
            } else {
                Some(ExcludePattern::Glob(p.clone()))
            }
        })
        .collect()
}

/// Returns true if the session name matches any exclude pattern.
pub fn session_excluded(name: &str, patterns: &[ExcludePattern]) -> bool {
    patterns.iter().any(|p| match p {
        ExcludePattern::Glob(g) => glob_matches(g, name),
        ExcludePattern::Regex(r) => r.is_match(name),
    })
}

/// Minimal glob matcher supporting `*` (any sequence) and `?` (single char).
fn glob_matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (plen, tlen) = (p.len(), t.len());
    // dp[i][j] = pattern[..i] matches text[..j]
    let mut dp = vec![vec![false; tlen + 1]; plen + 1];
    dp[0][0] = true;
    for i in 1..=plen {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=plen {
        for j in 1..=tlen {
            match p[i - 1] {
                '*' => dp[i][j] = dp[i - 1][j] || dp[i][j - 1],
                '?' => dp[i][j] = dp[i - 1][j - 1],
                c => dp[i][j] = c == t[j - 1] && dp[i - 1][j - 1],
            }
        }
    }
    dp[plen][tlen]
}
```

- [ ] **Step 5: Add `use regex;` — not needed, `regex::Regex` is used with full path**

No action needed — the code uses `regex::Regex` directly.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib config::tests`
Expected: all 8 tests pass.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/config.rs
git commit -m "feat: add pattern matching core (glob + regex) for session exclude"
```

---

### Task 2: Extend Config to persist exclude_patterns

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Write failing tests for config parse/serialize**

Add to the `tests` module in `src/config.rs`:

```rust
    #[test]
    fn parse_json_with_exclude_patterns() {
        let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28,
  "exclude_patterns": ["_*", "/^test/"]
}"#;
        let config = parse_json(json).unwrap();
        assert_eq!(config.exclude_patterns, vec!["_*", "/^test/"]);
    }

    #[test]
    fn parse_json_without_exclude_patterns_uses_default() {
        let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28
}"#;
        let config = parse_json(json).unwrap();
        assert_eq!(config.exclude_patterns, vec!["_*"]);
    }

    #[test]
    fn config_save_includes_exclude_patterns() {
        let config = Config {
            exclude_patterns: vec!["_*".to_string(), "/^test/".to_string()],
            ..Config::default()
        };
        let json = config.to_json();
        assert!(json.contains(r#""exclude_patterns": ["_*", "/^test/"]"#));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config::tests`
Expected: compilation errors — `exclude_patterns` field not on Config, `to_json` not defined.

- [ ] **Step 3: Add `exclude_patterns` field to Config struct and Default**

In `src/config.rs`, change the `Config` struct:

```rust
pub struct Config {
    pub theme: String,
    pub layout: String,
    pub show_borders: bool,
    pub sidebar_width: u16,
    pub exclude_patterns: Vec<String>,
}
```

Update `Default`:

```rust
impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "Catppuccin Mocha".to_string(),
            layout: "horizontal".to_string(),
            show_borders: true,
            sidebar_width: 28,
            exclude_patterns: vec!["_*".to_string()],
        }
    }
}
```

- [ ] **Step 4: Update `parse_json` to handle `exclude_patterns` array**

The current line-based parser won't handle JSON arrays spanning multiple lines. We need to detect the `exclude_patterns` key and parse its array value. Replace `parse_json`:

```rust
fn parse_json(s: &str) -> Option<Config> {
    let mut config = Config::default();
    let mut in_exclude = false;
    let mut exclude_patterns: Vec<String> = Vec::new();
    let mut found_exclude = false;

    let reader = io::BufReader::new(s.as_bytes());
    for line in reader.lines() {
        let line = line.ok()?;
        let trimmed = line.trim();

        // Detect start of exclude_patterns array
        if trimmed.starts_with("\"exclude_patterns\"") {
            found_exclude = true;
            // Could be single-line: "exclude_patterns": ["a", "b"]
            if let Some(bracket_start) = trimmed.find('[') {
                let rest = &trimmed[bracket_start..];
                if let Some(bracket_end) = rest.find(']') {
                    // Single-line array
                    let inner = &rest[1..bracket_end];
                    exclude_patterns = parse_string_array(inner);
                } else {
                    in_exclude = true;
                }
            }
            continue;
        }

        if in_exclude {
            if trimmed.starts_with(']') {
                in_exclude = false;
            } else {
                let val = trimmed.trim_matches(|c: char| c == '"' || c == ',' || c.is_whitespace());
                if !val.is_empty() {
                    exclude_patterns.push(val.to_string());
                }
            }
            continue;
        }

        // Parse "key": value
        if !trimmed.starts_with('"') {
            continue;
        }
        let mut parts = trimmed.splitn(2, ':');
        let key = parts.next()?.trim().trim_matches('"');
        let val = parts.next()?.trim().trim_end_matches(',');
        match key {
            "theme" => config.theme = val.trim_matches('"').to_string(),
            "layout" => config.layout = val.trim_matches('"').to_string(),
            "show_borders" => config.show_borders = val == "true",
            "sidebar_width" => {
                if let Ok(w) = val.parse::<u16>() {
                    config.sidebar_width = w;
                }
            }
            _ => {}
        }
    }

    if found_exclude {
        config.exclude_patterns = exclude_patterns;
    }

    Some(config)
}

/// Parse a comma-separated list of quoted strings from inside `[...]`.
fn parse_string_array(s: &str) -> Vec<String> {
    s.split(',')
        .map(|item| item.trim().trim_matches('"').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}
```

- [ ] **Step 5: Extract JSON serialization to `to_json()` and update `save()`**

Add a `to_json` method and update `save` to use it:

```rust
impl Config {
    pub fn to_json(&self) -> String {
        let patterns_json = if self.exclude_patterns.is_empty() {
            "[]".to_string()
        } else {
            let items: Vec<String> = self.exclude_patterns.iter().map(|p| quote(p)).collect();
            format!("[{}]", items.join(", "))
        };
        format!(
            "{{\n  \"theme\": {},\n  \"layout\": {},\n  \"show_borders\": {},\n  \"sidebar_width\": {},\n  \"exclude_patterns\": {}\n}}\n",
            quote(&self.theme),
            quote(&self.layout),
            self.show_borders,
            self.sidebar_width,
            patterns_json,
        )
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, self.to_json());
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib config::tests`
Expected: all 11 tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/config.rs
git commit -m "feat: persist exclude_patterns in config.json"
```

---

### Task 3: Wire patterns into AppState and session filtering

**Files:**
- Modify: `src/state.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Add exclude pattern fields to AppState**

In `src/state.rs`, add the import at top:

```rust
use crate::config::ExcludePattern;
```

Add fields to the `AppState` struct (after `session_order`):

```rust
    pub exclude_patterns: Vec<String>,
    pub compiled_patterns: Vec<ExcludePattern>,
```

Update `AppState::new()` to accept and store patterns. Change the signature:

```rust
    pub fn new(
        theme_index: usize,
        layout_mode: LayoutMode,
        show_borders: bool,
        sidebar_width: u16,
        exclude_patterns: Vec<String>,
        compiled_patterns: Vec<ExcludePattern>,
        term_width: u16,
        term_height: u16,
    ) -> Self {
```

And add these in the `Self { ... }` initializer (after `session_order: Vec::new(),`):

```rust
            exclude_patterns,
            compiled_patterns,
```

- [ ] **Step 2: Update `app.rs` App::new() to load and pass patterns**

In `src/app.rs`, inside `App::new()`, after loading `sidebar_width` from config (around line 65):

```rust
        let exclude_patterns = cfg.exclude_patterns.clone();
        let compiled_patterns = crate::config::compile_patterns(&exclude_patterns);
```

Update the `AppState::new()` call to pass the new args:

```rust
        let state = AppState::new(
            theme_index,
            layout_mode,
            show_borders,
            sidebar_width,
            exclude_patterns,
            compiled_patterns,
            term_width,
            term_height,
        );
```

- [ ] **Step 3: Replace hardcoded `_` filter in `refresh_sessions()`**

In `src/app.rs`, in `refresh_sessions()` (around line 587-589), change:

```rust
        self.state.sessions = sessions
            .into_iter()
            .filter(|s| !s.name.starts_with('_'))
```

to:

```rust
        self.state.sessions = sessions
            .into_iter()
            .filter(|s| !crate::config::session_excluded(&s.name, &self.state.compiled_patterns))
```

- [ ] **Step 4: Update `save_config()` to include exclude_patterns**

In `src/app.rs`, change `save_config()`:

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
            exclude_patterns: self.state.exclude_patterns.clone(),
        }
        .save();
    }
```

- [ ] **Step 5: Fix test helper `make_test_state` in `action.rs` tests**

In `src/action.rs`, update the `make_test_state` helper to pass the new `AppState::new` args:

```rust
    fn make_test_state(n: usize) -> AppState {
        let mut state = AppState::new(0, LayoutMode::Horizontal, true, 28, vec![], vec![], 120, 40);
```

Also update `App::new` test in `app.rs` if there is one — check `app.rs` tests.

- [ ] **Step 6: Build and run all tests**

Run: `cargo test`
Expected: all tests pass. The hardcoded `_` filter is now driven by config defaults.

- [ ] **Step 7: Commit**

```bash
git add src/state.rs src/app.rs src/action.rs
git commit -m "feat: wire exclude patterns into session filtering, replace hardcoded _ filter"
```

---

### Task 4: Add Exclude row to Settings UI

**Files:**
- Modify: `src/state.rs`
- Modify: `src/ui.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Bump SETTINGS_ITEM_COUNT**

In `src/state.rs`, change:

```rust
pub const SETTINGS_ITEM_COUNT: usize = 3;
```

to:

```rust
pub const SETTINGS_ITEM_COUNT: usize = 4;
```

- [ ] **Step 2: Add `exclude_count` to SettingsView**

In `src/ui.rs`, add to the `SettingsView` struct:

```rust
    pub exclude_count: usize,
```

- [ ] **Step 3: Add 4th entry in `draw_settings_page`**

In `src/ui.rs`, in `draw_settings_page()`, change the `entries` array to add the Exclude row. Replace the `entries` definition:

```rust
    let entries: Vec<(&str, String, &str)> = vec![
        (
            "Theme",
            settings.theme_name.to_string(),
            "Enter opens the theme list",
        ),
        (
            "Layout",
            match settings.layout_mode {
                LayoutMode::Horizontal => "Horizontal".to_string(),
                LayoutMode::Vertical => "Vertical".to_string(),
            },
            "Left/right toggles the split direction",
        ),
        (
            "Borders",
            if settings.show_borders { "On" } else { "Off" }.to_string(),
            "Left/right toggles pane borders",
        ),
        (
            "Exclude",
            format!("{} patterns", settings.exclude_count),
            "Enter opens the pattern editor",
        ),
    ];
```

Note: the type changes from fixed-size array to `Vec` because the tuple contains a mix of owned and borrowed strings.

- [ ] **Step 4: Pass `exclude_count` when constructing SettingsView in app.rs**

In `src/app.rs`, where `SettingsView` is constructed (around line 388), add:

```rust
            exclude_count: s.exclude_patterns.len(),
```

- [ ] **Step 5: Handle SettingsAdjust for index 3 (Exclude)**

In `src/action.rs`, in the `Action::SettingsAdjust(direction)` match arm, add a case for index 3:

```rust
            3 => {
                let _ = direction;
                apply_action(state, Action::OpenExcludeEditor);
            }
```

(This will fail to compile until Task 5 adds the action variant — that's expected.)

- [ ] **Step 6: Build to verify settings UI compiles (excluding the new action)**

Run: `cargo check`
Expected: error on `Action::OpenExcludeEditor` not existing — this is fine, will be resolved in Task 5.

- [ ] **Step 7: Commit**

```bash
git add src/state.rs src/ui.rs src/app.rs src/action.rs
git commit -m "feat: add Exclude row to settings page"
```

---

### Task 5: Exclude editor state, actions, and key routing

**Files:**
- Modify: `src/state.rs`
- Modify: `src/action.rs`

- [ ] **Step 1: Add ExcludeEditorState to state.rs**

In `src/state.rs`, add after the `RenameState` struct:

```rust
/// UI state for the exclude pattern editor popup.
#[derive(Debug, Clone)]
pub struct ExcludeEditorState {
    pub selected: usize,
    pub adding: bool,
    pub input: String,
    pub cursor: usize,
    pub error: Option<String>,
}
```

Add to `AppState` struct (after `context_menu`):

```rust
    pub exclude_editor: Option<ExcludeEditorState>,
```

Initialize in `AppState::new()`:

```rust
            exclude_editor: None,
```

- [ ] **Step 2: Add Action variants for exclude editor**

In `src/action.rs`, add to the `Action` enum (after `ConfirmThemePicker`):

```rust
    // Exclude editor
    OpenExcludeEditor,
    CloseExcludeEditor,
    ExcludeEditorNext,
    ExcludeEditorPrev,
    ExcludeEditorStartAdd,
    ExcludeEditorDelete,
    ExcludeEditorInput(char),
    ExcludeEditorBackspace,
    ExcludeEditorConfirm,
```

- [ ] **Step 3: Implement apply_action handlers for exclude editor**

In `src/action.rs`, in `apply_action()`, add after the `Action::ThemePickerPrev` handler (after line ~318):

```rust
        Action::OpenExcludeEditor => {
            state.exclude_editor = Some(ExcludeEditorState {
                selected: 0,
                adding: false,
                input: String::new(),
                cursor: 0,
                error: None,
            });
        }
        Action::CloseExcludeEditor => {
            state.exclude_editor = None;
        }
        Action::ExcludeEditorNext => {
            if let Some(ref mut editor) = state.exclude_editor {
                if !editor.adding && !state.exclude_patterns.is_empty() {
                    editor.selected =
                        (editor.selected + 1).min(state.exclude_patterns.len() - 1);
                }
            }
        }
        Action::ExcludeEditorPrev => {
            if let Some(ref mut editor) = state.exclude_editor {
                if !editor.adding && editor.selected > 0 {
                    editor.selected -= 1;
                }
            }
        }
        Action::ExcludeEditorStartAdd => {
            if let Some(ref mut editor) = state.exclude_editor {
                editor.adding = true;
                editor.input.clear();
                editor.cursor = 0;
                editor.error = None;
            }
        }
        Action::ExcludeEditorDelete => {
            if let Some(ref mut editor) = state.exclude_editor {
                if !editor.adding && !state.exclude_patterns.is_empty() {
                    state.exclude_patterns.remove(editor.selected);
                    state.compiled_patterns =
                        crate::config::compile_patterns(&state.exclude_patterns);
                    if editor.selected > 0
                        && editor.selected >= state.exclude_patterns.len()
                    {
                        editor.selected = state.exclude_patterns.len().saturating_sub(1);
                    }
                    fx.save_config = true;
                    fx.refresh_sessions = true;
                }
            }
        }
        Action::ExcludeEditorInput(ch) => {
            if let Some(ref mut editor) = state.exclude_editor {
                if editor.adding {
                    editor.input.insert(editor.cursor, ch);
                    editor.cursor += 1;
                    editor.error = None;
                }
            }
        }
        Action::ExcludeEditorBackspace => {
            if let Some(ref mut editor) = state.exclude_editor {
                if editor.adding && editor.cursor > 0 {
                    editor.cursor -= 1;
                    editor.input.remove(editor.cursor);
                    editor.error = None;
                }
            }
        }
        Action::ExcludeEditorConfirm => {
            if let Some(ref mut editor) = state.exclude_editor {
                if editor.adding {
                    let pattern = editor.input.trim().to_string();
                    if pattern.is_empty() {
                        editor.adding = false;
                    } else if let Some(inner) =
                        pattern.strip_prefix('/').and_then(|s| s.strip_suffix('/'))
                    {
                        // Validate regex
                        match regex::Regex::new(inner) {
                            Ok(_) => {
                                state.exclude_patterns.push(pattern);
                                state.compiled_patterns =
                                    crate::config::compile_patterns(&state.exclude_patterns);
                                editor.adding = false;
                                editor.input.clear();
                                editor.cursor = 0;
                                editor.error = None;
                                editor.selected =
                                    state.exclude_patterns.len().saturating_sub(1);
                                fx.save_config = true;
                                fx.refresh_sessions = true;
                            }
                            Err(e) => {
                                editor.error = Some(format!("Invalid regex: {}", e));
                            }
                        }
                    } else {
                        state.exclude_patterns.push(pattern);
                        state.compiled_patterns =
                            crate::config::compile_patterns(&state.exclude_patterns);
                        editor.adding = false;
                        editor.input.clear();
                        editor.cursor = 0;
                        editor.error = None;
                        editor.selected = state.exclude_patterns.len().saturating_sub(1);
                        fx.save_config = true;
                        fx.refresh_sessions = true;
                    }
                }
            }
        }
```

Add the import at the top of `action.rs`:

```rust
use crate::state::ExcludeEditorState;
```

- [ ] **Step 4: Add key routing for exclude editor**

In `src/action.rs`, in `key_to_action()`, modify the settings block (around line 530-534). Change:

```rust
    if state.main_view == MainView::Settings && state.focus_mode == FocusMode::Main {
        if state.theme_picker_open {
            return theme_picker_key_to_action(key);
        }
        return settings_key_to_action(key);
    }
```

to:

```rust
    if state.main_view == MainView::Settings && state.focus_mode == FocusMode::Main {
        if state.exclude_editor.is_some() {
            return exclude_editor_key_to_action(key, state);
        }
        if state.theme_picker_open {
            return theme_picker_key_to_action(key);
        }
        return settings_key_to_action(key);
    }
```

Add the new key mapping function after `theme_picker_key_to_action`:

```rust
fn exclude_editor_key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    let adding = state
        .exclude_editor
        .as_ref()
        .map_or(false, |e| e.adding);

    if adding {
        return match key.code {
            KeyCode::Esc => Action::CloseExcludeEditor,
            KeyCode::Enter => Action::ExcludeEditorConfirm,
            KeyCode::Backspace => Action::ExcludeEditorBackspace,
            KeyCode::Char(ch) => Action::ExcludeEditorInput(ch),
            _ => Action::None,
        };
    }

    match key.code {
        KeyCode::Esc => Action::CloseExcludeEditor,
        KeyCode::Char('j') | KeyCode::Down => Action::ExcludeEditorNext,
        KeyCode::Char('k') | KeyCode::Up => Action::ExcludeEditorPrev,
        KeyCode::Char('a') => Action::ExcludeEditorStartAdd,
        KeyCode::Char('d') | KeyCode::Char('x') => Action::ExcludeEditorDelete,
        _ => Action::None,
    }
}
```

- [ ] **Step 5: Write tests for exclude editor actions**

Add to the `tests` module in `src/action.rs`:

```rust
    #[test]
    fn open_close_exclude_editor() {
        let mut state = make_test_state(1);
        state.main_view = MainView::Settings;
        state.settings_selected = 3;
        apply_action(&mut state, Action::OpenExcludeEditor);
        assert!(state.exclude_editor.is_some());
        apply_action(&mut state, Action::CloseExcludeEditor);
        assert!(state.exclude_editor.is_none());
    }

    #[test]
    fn exclude_editor_add_pattern() {
        let mut state = make_test_state(1);
        state.exclude_patterns = vec!["_*".to_string()];
        state.compiled_patterns = crate::config::compile_patterns(&state.exclude_patterns);
        apply_action(&mut state, Action::OpenExcludeEditor);
        apply_action(&mut state, Action::ExcludeEditorStartAdd);
        assert!(state.exclude_editor.as_ref().unwrap().adding);
        apply_action(&mut state, Action::ExcludeEditorInput('t'));
        apply_action(&mut state, Action::ExcludeEditorInput('*'));
        let fx = apply_action(&mut state, Action::ExcludeEditorConfirm);
        assert_eq!(state.exclude_patterns, vec!["_*", "t*"]);
        assert!(fx.save_config);
        assert!(fx.refresh_sessions);
        assert!(!state.exclude_editor.as_ref().unwrap().adding);
    }

    #[test]
    fn exclude_editor_delete_pattern() {
        let mut state = make_test_state(1);
        state.exclude_patterns = vec!["_*".to_string(), "scratch*".to_string()];
        state.compiled_patterns = crate::config::compile_patterns(&state.exclude_patterns);
        apply_action(&mut state, Action::OpenExcludeEditor);
        state.exclude_editor.as_mut().unwrap().selected = 0;
        let fx = apply_action(&mut state, Action::ExcludeEditorDelete);
        assert_eq!(state.exclude_patterns, vec!["scratch*"]);
        assert!(fx.save_config);
        assert!(fx.refresh_sessions);
    }

    #[test]
    fn exclude_editor_invalid_regex_shows_error() {
        let mut state = make_test_state(1);
        state.exclude_patterns = vec![];
        state.compiled_patterns = vec![];
        apply_action(&mut state, Action::OpenExcludeEditor);
        apply_action(&mut state, Action::ExcludeEditorStartAdd);
        for ch in "/[invalid/".chars() {
            apply_action(&mut state, Action::ExcludeEditorInput(ch));
        }
        apply_action(&mut state, Action::ExcludeEditorConfirm);
        let editor = state.exclude_editor.as_ref().unwrap();
        assert!(editor.adding); // still in add mode
        assert!(editor.error.is_some());
        assert!(state.exclude_patterns.is_empty()); // not added
    }
```

- [ ] **Step 6: Run all tests**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/state.rs src/action.rs
git commit -m "feat: add exclude editor state, actions, and key routing"
```

---

### Task 6: Draw exclude editor popup

**Files:**
- Modify: `src/ui.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Add exclude editor data to SettingsView**

In `src/ui.rs`, add to `SettingsView`:

```rust
    pub exclude_editor: Option<ExcludeEditorView<'a>>,
```

Add the new struct before `SettingsView`:

```rust
pub struct ExcludeEditorView<'a> {
    pub patterns: &'a [String],
    pub selected: usize,
    pub adding: bool,
    pub input: &'a str,
    pub cursor: usize,
    pub error: Option<&'a str>,
}
```

- [ ] **Step 2: Pass exclude editor data from app.rs**

In `src/app.rs`, when constructing `SettingsView`, add:

```rust
            exclude_editor: s.exclude_editor.as_ref().map(|e| ui::ExcludeEditorView {
                patterns: &s.exclude_patterns,
                selected: e.selected,
                adding: e.adding,
                input: &e.input,
                cursor: e.cursor,
                error: e.error.as_deref(),
            }),
```

- [ ] **Step 3: Draw the popup in `draw_settings_page`**

In `src/ui.rs`, at the end of `draw_settings_page()`, after the theme picker block, add:

```rust
    if let Some(ref editor) = settings.exclude_editor {
        draw_exclude_editor(frame, area, editor, theme);
    }
```

- [ ] **Step 4: Implement `draw_exclude_editor`**

In `src/ui.rs`, add after `draw_theme_picker`:

```rust
fn draw_exclude_editor(
    frame: &mut Frame,
    area: Rect,
    editor: &ExcludeEditorView,
    theme: &Theme,
) {
    let pattern_count = editor.patterns.len();
    let max_pattern_width = editor
        .patterns
        .iter()
        .map(|p| p.len())
        .max()
        .unwrap_or(0)
        .max(20);

    let content_lines = pattern_count
        + if editor.adding { 1 } else { 0 }
        + if editor.error.is_some() { 1 } else { 0 };
    let height = (content_lines as u16 + 4).min(area.height.saturating_sub(2)).max(5);
    let width = (max_pattern_width as u16 + 8)
        .max(30)
        .min(area.width.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Exclude Patterns ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.bg));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines: Vec<Line> = Vec::new();

    if pattern_count == 0 && !editor.adding {
        lines.push(Line::from(Span::styled(
            "  No patterns defined",
            Style::default().fg(theme.dim),
        )));
    }

    for (i, pattern) in editor.patterns.iter().enumerate() {
        let selected = !editor.adding && i == editor.selected;
        let row_bg = if selected { theme.surface } else { theme.bg };
        let marker = if selected { "▌" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(
                marker,
                Style::default()
                    .fg(if selected { theme.accent } else { theme.bg })
                    .bg(row_bg),
            ),
            Span::styled(format!(" {} ", pattern), Style::default().fg(theme.text).bg(row_bg)),
        ]));
    }

    if editor.adding {
        let display_input = if editor.input.is_empty() {
            "│"
        } else {
            &editor.input
        };
        lines.push(Line::from(vec![
            Span::styled("▌", Style::default().fg(theme.green).bg(theme.surface)),
            Span::styled(
                format!(" {} ", display_input),
                Style::default().fg(theme.text).bg(theme.surface),
            ),
        ]));
    }

    if let Some(err) = editor.error {
        lines.push(Line::from(Span::styled(
            format!("  {}", err),
            Style::default().fg(theme.pink),
        )));
    }

    // Help line
    lines.push(Line::raw(""));
    let help = if editor.adding {
        "  Enter: confirm  Esc: cancel"
    } else {
        "  a: add  d: delete  Esc: close"
    };
    lines.push(Line::from(Span::styled(
        help,
        Style::default().fg(theme.muted),
    )));

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        inner,
    );
}
```

- [ ] **Step 5: Build and check**

Run: `cargo check`
Expected: compiles cleanly.

- [ ] **Step 6: Run all tests**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/ui.rs src/app.rs
git commit -m "feat: add exclude editor popup in settings UI"
```

---

### Task 7: Final integration test and cleanup

**Files:**
- Modify: `src/action.rs` (if needed)

- [ ] **Step 1: Run clippy**

Run: `cargo clippy`
Expected: no warnings related to our changes.

- [ ] **Step 2: Run full test suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 3: Build release**

Run: `cargo build --release`
Expected: builds successfully.

- [ ] **Step 4: Fix any clippy warnings or test failures**

Address any issues found in steps 1-3.

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "chore: fix clippy warnings and cleanup"
```

(Skip if no changes needed.)

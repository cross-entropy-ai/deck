//! State and pure helpers for the new-session working-dir picker
//! overlay. FS access lives in `app::dispatch`; everything here is
//! pure and unit-testable.

use std::path::PathBuf;

use ratatui_textarea::{CursorMove, TextArea};

/// Split `input` into `(parent, leaf)` where `parent` is the directory
/// portion (including any trailing `/`) and `leaf` is the segment
/// being typed.
///
/// - `""` → `("", "")`
/// - `"~/foo/"` → `("~/foo/", "")`
/// - `"~/foo/ba"` → `("~/foo/", "ba")`
/// - `"foo"` → `("", "foo")`
pub fn split_input(input: &str) -> (&str, &str) {
    match input.rfind('/') {
        Some(idx) => (&input[..=idx], &input[idx + 1..]),
        None => ("", input),
    }
}

/// Compute the `filtered` index list from `entries`. Case-insensitive
/// prefix match on `leaf`. Dotfile entries are included iff `leaf`
/// starts with `.`.
pub fn filter_entries(entries: &[String], leaf: &str) -> Vec<usize> {
    let leaf_lc = leaf.to_lowercase();
    let allow_dot = leaf.starts_with('.');
    entries
        .iter()
        .enumerate()
        .filter(|(_, name)| {
            if !allow_dot && name.starts_with('.') {
                return false;
            }
            name.to_lowercase().starts_with(&leaf_lc)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Build a single-line `TextArea` pre-filled with `s`, cursor at end.
pub fn make_textarea(s: &str) -> TextArea<'static> {
    let mut ta = TextArea::new(vec![s.to_string()]);
    ta.move_cursor(CursorMove::End);
    ta
}

/// First (only) line of a single-line `TextArea`, as a borrowed `&str`.
pub fn textarea_line<'a>(ta: &'a TextArea<'a>) -> &'a str {
    ta.lines().first().map(String::as_str).unwrap_or("")
}

/// Resolve a user-typed path to an absolute, normalized `PathBuf`.
///
/// - Leading `~` expands to `$HOME`. `~/foo` → `<home>/foo`. Bare `~`
///   → `<home>`.
/// - Bare relative paths (no leading `/` or `~`) resolve under
///   `$HOME` for predictability.
/// - `..` and redundant `/` are normalized via `Path::components`.
pub fn expand_path(s: &str, home: &std::path::Path) -> PathBuf {
    let mut buf = if let Some(rest) = s.strip_prefix("~/") {
        home.join(rest)
    } else if s == "~" {
        home.to_path_buf()
    } else if s.starts_with('/') {
        PathBuf::from(s)
    } else {
        home.join(s)
    };
    // Normalize `..` and redundant separators.
    let mut normalized = PathBuf::new();
    for comp in buf.components() {
        match comp {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    buf = normalized;
    buf
}

#[derive(Debug, Clone)]
pub struct NewSessionState {
    /// Session name input field. Pre-filled with the next free
    /// `session-N` when the picker opens; user-editable.
    pub name: TextArea<'static>,
    /// Which field has keyboard focus.
    pub focus: PickerFocus,
    /// User-visible path. `~` and `..` preserved verbatim.
    pub input: TextArea<'static>,
    /// All children (directories only) of the parent of `input`.
    /// Written by dispatch after `read_dir`. The reducer never mutates
    /// this directly.
    pub entries: Vec<String>,
    /// Indices into `entries` after leaf-prefix + dotfile filtering.
    /// Recomputed by the reducer whenever `input` changes.
    pub filtered: Vec<usize>,
    /// Index into `filtered`. Reducer clamps to `0..filtered.len()`.
    pub selected: usize,
    /// Last error encountered. Cleared on the next successful mutation.
    pub error: Option<String>,
    /// `Some(host)` when the picker is creating a session on a remote
    /// host: directory entries come from `ssh <host> ls` and the session
    /// is created over ssh. `None` is the local picker.
    pub remote_host: Option<String>,
}

impl Default for NewSessionState {
    fn default() -> Self {
        Self {
            name: make_textarea(""),
            focus: PickerFocus::default(),
            input: make_textarea(""),
            entries: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            error: None,
            remote_host: None,
        }
    }
}

impl NewSessionState {
    /// First line of the `name` textarea.
    pub fn name_str(&self) -> &str {
        textarea_line(&self.name)
    }

    /// First line of the `input` textarea.
    pub fn input_str(&self) -> &str {
        textarea_line(&self.input)
    }

    /// Helper: rebuild `filtered` from current `input` and `entries`,
    /// clamp `selected` to the new range.
    pub fn refilter(&mut self) {
        let (_parent, leaf) = split_input(self.input_str());
        self.filtered = filter_entries(&self.entries, leaf);
        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PickerFocus {
    #[default]
    Name,
    Dir,
}

/// Pick the next free `session-N`, starting the search from `start`
/// (typically `existing.len()`). Used to pre-fill the picker's name
/// field with what was previously generated inline at create time.
pub fn auto_session_name(existing: &[&str], start: usize) -> String {
    let mut idx = start;
    loop {
        let candidate = format!("session-{idx}");
        if !existing.contains(&candidate.as_str()) {
            return candidate;
        }
        idx += 1;
    }
}

#[cfg(test)]
#[path = "../../tests/unit/model/new_session.rs"]
mod tests;

//! State and pure helpers for the new-session working-dir picker
//! overlay. FS access lives in `app::dispatch`; everything here is
//! pure and unit-testable.

use std::path::PathBuf;

use ratatui_textarea::{CursorMove, TextArea};

use crate::picker::FilterPicker;

/// Split `input` into `(parent, leaf)`: `parent` is the directory portion
/// (with any trailing `/`), `leaf` the segment being typed. E.g. `""` →
/// `("", "")`, `"~/foo/"` → `("~/foo/", "")`, `"~/foo/ba"` → `("~/foo/", "ba")`,
/// `"foo"` → `("", "foo")`.
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

/// Resolve a user-typed path to an absolute, normalized `PathBuf`. Leading
/// `~` expands to `$HOME` (`~/foo` → `<home>/foo`, bare `~` → `<home>`); bare
/// relative paths resolve under `$HOME`; `..` and redundant `/` are normalized
/// via `Path::components`.
pub fn expand_path(s: &str, home: &std::path::Path) -> PathBuf {
    let buf = if let Some(rest) = s.strip_prefix("~/") {
        home.join(rest)
    } else if s == "~" {
        home.to_path_buf()
    } else if s.starts_with('/') {
        PathBuf::from(s)
    } else {
        home.join(s)
    };
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
    normalized
}

/// The new-session overlay: a *two-field* picker (session `name` plus a
/// dir-browse field), so not a plain filter-picker. The dir-browse half
/// (path input, listing, filtered/selected, error slot) is delegated to the
/// shared `FilterPicker`; `name`, focus switching, `~`/segment path editing,
/// and `remote_host` stay bespoke since they don't fit the generic shape.
#[derive(Debug, Clone)]
pub struct NewSessionState {
    /// Session name input field. Pre-filled with the next free
    /// `session-N` when the picker opens; user-editable.
    pub name: TextArea<'static>,
    /// Which field has keyboard focus.
    pub focus: PickerFocus,
    /// Path input + directory listing + filtered/selected + error.
    /// `picker.input` is the path (`~`/`..` verbatim); `picker.items` the
    /// directory children (written by dispatch after `read_dir`);
    /// `picker.error` the single error slot, also set by name validation.
    pub picker: FilterPicker,
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
            picker: FilterPicker::new(Vec::new()),
            remote_host: None,
        }
    }
}

impl NewSessionState {
    /// First line of the `name` textarea.
    pub fn name_str(&self) -> &str {
        textarea_line(&self.name)
    }

    /// First line of the path input.
    pub fn input_str(&self) -> &str {
        self.picker.input_str()
    }

    /// Rebuild the listing's `filtered` from the path input's leaf segment
    /// (case-insensitive prefix + dotfile rule) and clamp the selection.
    /// Filters on the leaf, not the whole input, so the predicate is wrapped
    /// for the shared `FilterPicker::refilter`.
    pub fn refilter(&mut self) {
        self.picker
            .refilter(|entries, input| filter_entries(entries, split_input(input).1));
    }

    /// Replace the path input with `path`, refilter the listing, and clear
    /// any pending error. The dir-navigation actions (up, enter, clear,
    /// delete-segment) all rewrite the whole path this way.
    pub fn set_path(&mut self, path: &str) {
        self.picker.input = make_textarea(path);
        self.refilter();
        self.picker.error = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PickerFocus {
    #[default]
    Name,
    Dir,
}

/// Pick the next free `session-N`, starting the search from `start`
/// (typically `existing.len()`). Used to pre-fill the picker's name field.
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

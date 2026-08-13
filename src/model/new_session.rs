//! State and pure helpers for the new-session working-dir picker
//! overlay. FS access lives in `app::dispatch`; everything here is
//! pure and unit-testable.

use std::path::PathBuf;

use ratatui_textarea::{CursorMove, TextArea};

use crate::picker::FilterPicker;

/// Number of directory rows kept visible in the new-session picker.
pub const DIRECTORY_VIEW_ROWS: usize = 8;

/// The synthetic "one level up" row, first in every listing.
///
/// It is shown and clickable, but the keyboard highlight skips it: `←` is the
/// keyboard way up, so the highlight stays on the children that `→` and `⏎`
/// act on. It is the only synthetic row, which is why skipping it is a single
/// step rather than a loop.
pub const PARENT_ENTRY: &str = "..";

/// Validate a tmux session name against deck's supported format and the
/// names already present on the target server. Creation and rename share this
/// boundary so they cannot disagree about valid or duplicate names.
pub(crate) fn validate_unique_session_name<'a>(
    name: &str,
    existing: impl IntoIterator<Item = &'a str>,
) -> Option<&'static str> {
    match name {
        "" => Some("name required"),
        n if n.contains('.') => Some("name cannot contain '.'"),
        n if n.contains(':') => Some("name cannot contain ':'"),
        _ => existing
            .into_iter()
            .any(|session| session == name)
            .then_some("name already in use"),
    }
}

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
            if name.as_str() == PARENT_ENTRY {
                return leaf.is_empty() || name.starts_with(&leaf_lc);
            }
            if !allow_dot && name.starts_with('.') {
                return false;
            }
            name.to_lowercase().starts_with(&leaf_lc)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Prepend the synthetic parent-directory entry to a backend listing.
pub fn with_parent_entry(mut entries: Vec<String>) -> Vec<String> {
    entries.retain(|entry| entry != PARENT_ENTRY);
    entries.insert(0, PARENT_ENTRY.to_string());
    entries
}

/// Parent of the directory represented by a trailing-slash picker path.
/// Normal segments collapse (`~/foo/` -> `~/`); walking above `~/` preserves
/// shell-expandable parent segments (`~/` -> `~/../` -> `~/../../`).
pub fn parent_directory(path: &str) -> String {
    if path == "/" {
        return "/".to_string();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "../".to_string();
    }
    if trimmed == "~" {
        return "~/../".to_string();
    }
    if trimmed == ".." || trimmed.ends_with("/..") {
        return format!("{trimmed}/../");
    }
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => trimmed[..=index].to_string(),
        None => String::new(),
    }
}

/// Build a single-line `TextArea` pre-filled with `s`, cursor at end.
pub fn make_textarea(s: &str) -> TextArea<'static> {
    let mut ta = TextArea::new(vec![s.to_string()]);
    ta.move_cursor(CursorMove::End);
    ta
}

/// First (only) line of a single-line `TextArea`, as a borrowed `&str`.
pub fn textarea_line<'a>(ta: &'a TextArea<'a>) -> &'a str {
    ta.lines().first().map_or("", String::as_str)
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
/// and the target lane stay bespoke since they don't fit the generic shape.
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
    /// First visible selection index in the filtered directory list. Kept as
    /// state so moving within the current viewport moves only the highlight,
    /// rather than re-anchoring the whole list on every key press.
    pub scroll: usize,
    /// Stable lane that owns directory listing and creation operations.
    pub target_lane: Option<crate::lane::LaneId>,
}

impl Default for NewSessionState {
    fn default() -> Self {
        Self {
            name: make_textarea(""),
            focus: PickerFocus::default(),
            picker: FilterPicker::new(Vec::new()),
            scroll: 0,
            target_lane: None,
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
        // A fresh listing clamps the selection to 0, which is the parent row.
        // Push it down onto the first real child so the highlight always sits
        // on something `→`/`⏎` can act on.
        self.skip_parent_row(1);
        self.keep_selection_visible();
    }

    /// Move the directory highlight with wraparound, scrolling only when the
    /// new selection would otherwise leave the current viewport.
    pub fn step_selection(&mut self, direction: i32) {
        self.picker.step_wrapped(direction);
        self.skip_parent_row(direction);
        self.keep_selection_visible();
    }

    /// The listing entry at filtered position `index`.
    pub fn entry_at(&self, index: usize) -> Option<&str> {
        let item = *self.picker.filtered.get(index)?;
        self.picker.items.get(item).map(String::as_str)
    }

    /// Whether filtered position `index` holds the synthetic parent row.
    pub fn is_parent_row(&self, index: usize) -> bool {
        self.entry_at(index) == Some(PARENT_ENTRY)
    }

    /// The path this picker would show after opening filtered position
    /// `index`: a child appends its name, [`PARENT_ENTRY`] walks up. `None`
    /// when `index` is not in the filtered list.
    ///
    /// Shared by every "open that row" path — key, click, and click-to-create
    /// — so they cannot disagree about where a row leads.
    pub fn path_after_entering(&self, index: usize) -> Option<String> {
        let entry = self.entry_at(index)?;
        let (parent, _leaf) = split_input(self.input_str());
        Some(if entry == PARENT_ENTRY {
            parent_directory(parent)
        } else {
            format!("{parent}{entry}/")
        })
    }

    /// Step the highlight off the parent row, continuing in `direction`.
    ///
    /// A no-op when `..` is the only row: with no child to hold the highlight
    /// it stays there, and `→` on it means the same thing `←` does.
    fn skip_parent_row(&mut self, direction: i32) {
        if !self.is_parent_row(self.picker.selected) {
            return;
        }
        if (0..self.picker.filtered.len()).all(|index| self.is_parent_row(index)) {
            return;
        }
        self.picker.step_wrapped(if direction < 0 { -1 } else { 1 });
    }

    /// Clamp the stored viewport and reveal the selection only when needed.
    pub fn keep_selection_visible(&mut self) {
        let len = self.picker.filtered.len();
        let max_scroll = len.saturating_sub(DIRECTORY_VIEW_ROWS);
        self.scroll = self.scroll.min(max_scroll);
        if self.picker.selected < self.scroll {
            self.scroll = self.picker.selected;
        } else if self.picker.selected >= self.scroll + DIRECTORY_VIEW_ROWS {
            self.scroll = (self.picker.selected + 1 - DIRECTORY_VIEW_ROWS).min(max_scroll);
        }
    }

    /// Replace the path input with `path`, refilter the listing, and clear
    /// any pending error. The dir-navigation actions (up, enter, clear,
    /// delete-segment) all rewrite the whole path this way.
    pub fn set_path(&mut self, path: &str) {
        self.picker.input = make_textarea(path);
        self.picker.selected = 0;
        self.scroll = 0;
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

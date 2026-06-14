//! A generic filter-picker: a text input over a fixed list of string
//! items, a derived `filtered` index list, a clamped selection, and a
//! pending error. The new-session dir browser and the add-remote host
//! picker both embed one of these; each supplies its own filter
//! predicate (`filter_entries` / `filter_hosts`) so the filtering stays
//! identical, only the recompute + clamp + step plumbing is shared.
//!
//! The exclude-pattern editor deliberately does *not* use this: it has no
//! live filter (its items live in `Prefs::exclude_patterns`, not here) and
//! an `adding` sub-mode, so it stays bespoke — see `overlay::ExcludeEditorState`.

use ratatui_textarea::TextArea;

use crate::new_session::{make_textarea, textarea_line};
use crate::state::step_clamped;

/// Text input + filtered list of `items`, with a clamped selection.
#[derive(Debug, Clone)]
pub struct FilterPicker {
    /// Free-text input; doubles as the live filter needle.
    pub input: TextArea<'static>,
    /// The full candidate list. Set when the picker opens; the reducer
    /// refills it only via `set_items`, never piecemeal.
    pub items: Vec<String>,
    /// Indices into `items` matching the input. Recomputed by `refilter`.
    pub filtered: Vec<usize>,
    /// Index into `filtered`; clamped to `0..filtered.len()` by `refilter`
    /// and `step`.
    pub selected: usize,
    /// Last error; cleared by callers on the next input edit.
    pub error: Option<String>,
}

impl FilterPicker {
    /// Open over `items` with an empty input; all items visible, first
    /// selected.
    pub fn new(items: Vec<String>) -> Self {
        let filtered = (0..items.len()).collect();
        Self {
            input: make_textarea(""),
            items,
            filtered,
            selected: 0,
            error: None,
        }
    }

    /// First line of the input textarea.
    pub fn input_str(&self) -> &str {
        textarea_line(&self.input)
    }

    /// Recompute `filtered` from the current input via `filter_fn`, then
    /// clamp `selected` into the new range (0 when empty).
    pub fn refilter(&mut self, filter_fn: impl Fn(&[String], &str) -> Vec<usize>) {
        self.filtered = filter_fn(&self.items, self.input_str());
        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        }
    }

    /// Move the selection by `direction` (+1 down / -1 up), clamped within
    /// `0..filtered.len()` (no wrap). A no-op when the list is empty.
    pub fn step(&mut self, direction: i32) {
        self.selected = step_clamped(self.selected, self.filtered.len(), direction);
    }

    /// The currently highlighted item, if any.
    pub fn selected_item(&self) -> Option<&str> {
        let idx = *self.filtered.get(self.selected)?;
        self.items.get(idx).map(String::as_str)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/model/picker.rs"]
mod tests;

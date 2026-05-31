//! State + pure helpers for the "Add Remote Host" picker. Mirrors the
//! new-session picker's split: this module owns the overlay state and the
//! filtering / choice logic; rendering lives in `ui/add_remote.rs`.

// Consumers (reducer/dispatch/UI) arrive in later tasks; the final task removes
// this once everything is wired.
#![allow(dead_code)]

use ratatui_textarea::{CursorMove, TextArea};

#[derive(Debug, Clone)]
pub struct AddRemoteState {
    /// Doubles as a live filter over `hosts` and a free-text hostname.
    pub input: TextArea<'static>,
    /// `~/.ssh/config` candidates minus hosts already in config.remotes.
    /// Set when the picker opens; the reducer never refills it.
    pub hosts: Vec<String>,
    /// Indices into `hosts` matching the input (case-insensitive substring).
    pub filtered: Vec<usize>,
    /// Index into `filtered`; clamped to `0..filtered.len()`.
    pub selected: usize,
    /// Last error (empty / already-added). Cleared on the next input edit.
    pub error: Option<String>,
}

impl AddRemoteState {
    /// Open over the given candidate hosts; all visible initially.
    pub fn new(hosts: Vec<String>) -> Self {
        let filtered = (0..hosts.len()).collect();
        Self {
            input: make_textarea(""),
            hosts,
            filtered,
            selected: 0,
            error: None,
        }
    }

    /// First line of the input textarea.
    pub fn input_str(&self) -> &str {
        self.input.lines().first().map(String::as_str).unwrap_or("")
    }

    /// Rebuild `filtered` from the current input; clamp `selected`.
    pub fn refilter(&mut self) {
        self.filtered = filter_hosts(&self.hosts, self.input_str());
        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        }
    }

    /// The host to add on confirm: the highlighted candidate when the filtered
    /// list is non-empty, otherwise the trimmed free-text input. `None` when
    /// there is nothing to add.
    pub fn chosen_host(&self) -> Option<String> {
        if let Some(&idx) = self.filtered.get(self.selected) {
            return self.hosts.get(idx).cloned();
        }
        let typed = self.input_str().trim();
        if typed.is_empty() {
            None
        } else {
            Some(typed.to_string())
        }
    }
}

pub fn make_textarea(s: &str) -> TextArea<'static> {
    let mut ta = TextArea::new(vec![s.to_string()]);
    ta.move_cursor(CursorMove::End);
    ta
}

/// Indices of `hosts` whose name contains `needle` (case-insensitive). An
/// empty/whitespace needle matches everything.
pub fn filter_hosts(hosts: &[String], needle: &str) -> Vec<usize> {
    let needle = needle.trim().to_ascii_lowercase();
    hosts
        .iter()
        .enumerate()
        .filter(|(_, h)| needle.is_empty() || h.to_ascii_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
#[path = "../../tests/unit/model/add_remote.rs"]
mod tests;

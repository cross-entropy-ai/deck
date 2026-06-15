//! State + pure helpers for the "Add Remote Host" picker. A thin wrapper
//! over the shared `FilterPicker`: the candidate hosts are the
//! picker's items, the input doubles as a live filter and a free-text
//! hostname, and `chosen_host` adds the picker's selection-or-typed-text
//! logic on top. Rendering lives in `ui/add_remote.rs`.

use crate::picker::FilterPicker;

#[derive(Debug, Clone)]
pub struct AddRemoteState {
    /// Input + `~/.ssh/config` candidates (minus hosts already in
    /// config.remotes) + filtered/selected/error. The candidate list is
    /// set when the picker opens; the reducer never refills it.
    pub picker: FilterPicker,
}

impl AddRemoteState {
    /// Open over the given candidate hosts; all visible initially.
    pub fn new(hosts: Vec<String>) -> Self {
        Self {
            picker: FilterPicker::new(hosts),
        }
    }

    /// First line of the input textarea.
    pub fn input_str(&self) -> &str {
        self.picker.input_str()
    }

    /// Rebuild the filtered list from the current input; clamp selection.
    pub fn refilter(&mut self) {
        self.picker.refilter(filter_hosts);
    }

    /// The host to add on confirm: the highlighted candidate when the
    /// filtered list is non-empty, otherwise the trimmed free-text input.
    /// `None` when there is nothing to add.
    pub fn chosen_host(&self) -> Option<String> {
        if let Some(host) = self.picker.selected_item() {
            return Some(host.to_string());
        }
        let typed = self.input_str().trim();
        if typed.is_empty() {
            None
        } else {
            Some(typed.to_string())
        }
    }
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
#[path = "../../../../tests/unit/model/add_remote.rs"]
mod tests;

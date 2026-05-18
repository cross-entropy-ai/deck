//! State and pure helpers for the new-session working-dir picker
//! overlay. FS access lives in `app::dispatch`; everything here is
//! pure and unit-testable.

use std::path::PathBuf;

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

/// Backspace with up-a-level semantics. If `cursor` is at the end of
/// `input` and `input` ends with `/` (and isn't just `/`), drop the
/// trailing `/` plus the previous segment. Otherwise delete one char
/// before the cursor.
pub fn smart_backspace(input: &mut String, cursor: &mut usize) {
    if *cursor == input.len() && input.len() > 1 && input.ends_with('/') {
        // up one level
        input.pop(); // drop trailing /
        let new_end = input.rfind('/').map(|i| i + 1).unwrap_or(0);
        input.truncate(new_end);
        *cursor = input.len();
        return;
    }
    if *cursor > 0 {
        let prev = input[..*cursor]
            .chars()
            .last()
            .map(|c| c.len_utf8())
            .unwrap_or(0);
        *cursor -= prev;
        input.remove(*cursor);
    }
}

/// Replace the trailing leaf segment of `input` with `entry` plus a
/// trailing `/`. Used by Tab completion. Cursor lands at the new end.
pub fn tab_complete(input: &mut String, cursor: &mut usize, entry: &str) {
    let (parent, _leaf) = split_input(input);
    let parent_owned = parent.to_string();
    input.clear();
    input.push_str(&parent_owned);
    input.push_str(entry);
    input.push('/');
    *cursor = input.len();
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

#[derive(Debug, Clone, Default)]
pub struct NewSessionState {
    /// User-visible path. `~` and `..` preserved verbatim.
    pub input: String,
    /// Byte offset into `input`.
    pub cursor: usize,
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
}

impl NewSessionState {
    /// Helper: rebuild `filtered` from current `input` and `entries`,
    /// clamp `selected` to the new range.
    pub fn refilter(&mut self) {
        let (_parent, leaf) = split_input(&self.input);
        self.filtered = filter_entries(&self.entries, leaf);
        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/model/new_session.rs"]
mod tests;

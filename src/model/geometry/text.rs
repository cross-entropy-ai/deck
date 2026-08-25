//! Display-width string helpers and the characters the sidebar is drawn from.
//!
//! Truncation and splitting count *display columns*, not bytes or chars, so a
//! CJK name occupies what it actually occupies on screen. `ui::text`
//! re-exports these; they live down here so `model` never has to reach up into
//! `ui` for them.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate `s` to at most `max_width` display columns, appending an
/// ellipsis on overflow.
pub fn truncate(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        // Only room for the ellipsis itself; no content fits beside it.
        return "…".to_string();
    }
    // One column short of `max_width`, leaving room for the ellipsis.
    let (head, _) = split_at_width(s, max_width - 1);
    format!("{head}…")
}

/// Split `s` into the longest prefix whose display width fits `max` columns
/// and the remainder. The prefix is empty when even the first char is wider
/// than `max` — callers decide whether to overflow or skip.
pub fn split_at_width(s: &str, max: usize) -> (&str, &str) {
    let mut width = 0usize;
    for (i, ch) in s.char_indices() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > max {
            return s.split_at(i);
        }
        width += ch_width;
    }
    (s, "")
}

// --- Tab / banner / header / footer geometry ---

/// Separator glyph rendered between tabs (width 1).
pub const TAB_SEPARATOR: &str = "│";
/// Shared footer/tab-bar menu label. The vertical tab layout reserves its
/// right edge before windowing sessions, so the menu never disappears merely
/// because there are too many tabs.
pub const MENU_LABEL: &str = "≡ menu";
/// Overflow marker shown at either edge of a windowed vertical tab run.
pub const TAB_OVERFLOW_MARKER: &str = "…";

/// Connector prefixed to a nested section's divider label: `TREE_BRANCH` while
/// siblings follow, `TREE_BRANCH_LAST` on the one that closes the run.
///
/// Both are exactly two cells, which is what lets the renderer trade them with
/// the collapse chevron so the connector leads the divider (see
/// `ui::sidebar::sessions::lead_with_branch`). The line is what says this
/// section hangs off the one above; the chevron is a control on it, and reads
/// as one only after the relationship is established.
pub const TREE_BRANCH: &str = "├ ";
/// See [`TREE_BRANCH`].
pub const TREE_BRANCH_LAST: &str = "└ ";
/// The line those connectors hang from, carried down the gutter of every row
/// between a group's divider and the nested one below it.
pub const TREE_TRUNK: &str = "│";
/// Collapse `$HOME` to `~` in a directory path. Pure; lives in the leaf
/// geometry module so the sidebar layout builder (in `model`) can format
/// session rows without reaching up into `ui`. `ui::text` re-exports it.
pub fn shorten_dir(dir: &str) -> String {
    // Resolved once: this runs per visible session row, and `env::var` takes
    // the process-global env lock + allocates each call.
    static HOME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let home = HOME.get_or_init(|| std::env::var("HOME").unwrap_or_default());
    match dir.strip_prefix(home.as_str()) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => dir.to_string(),
    }
}

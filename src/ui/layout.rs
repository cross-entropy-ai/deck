//! Pure geometry helpers shared by the UI (drawing) and state
//! (hit-testing) layers. Keeping them in a neutral module breaks the
//! otherwise-circular "state imports ui for hit-testing" dependency.
//!
//! The tab bar's geometry (leading pad, inner pad, separator) is
//! defined here as the single source of truth. Renderer and hit-tester
//! both read from these constants/helpers, so tweaking the tab visual
//! width automatically keeps click-target math in sync.

use unicode_width::UnicodeWidthStr;

use crate::state::ViewMode;

/// Leading padding (in columns) before the first tab in the tab bar.
pub const TAB_LEADING_PAD: u16 = 1;
/// Padding (in columns) between idx and name, and after name, inside a tab.
pub const TAB_INNER_PAD: u16 = 1;
/// Separator glyph rendered between tabs (width 1).
pub const TAB_SEPARATOR: &str = "│";

/// Minimum sidebar content width before the update banner renders at all.
pub const BANNER_MIN_WIDTH: u16 = 8;

/// Rows the plugin status block takes in the sidebar footer: title +
/// one row per plugin + trailing separator. Zero when no plugins are
/// configured so the sidebar keeps its original layout for users
/// without any extensions. Shared so mouse hit-testing in
/// `AppState::session_at_row` stays in sync with the sidebar renderer.
pub const fn plugin_block_rows(count: usize) -> u16 {
    if count == 0 {
        0
    } else {
        count as u16 + 2
    }
}

pub fn card_height(view_mode: ViewMode) -> usize {
    match view_mode {
        ViewMode::Expanded => 5,
        ViewMode::Compact => 2,
    }
}

fn tab_width(index: usize, name: &str) -> u16 {
    let idx_width = format!("{}", index + 1).len() as u16;
    let name_width = UnicodeWidthStr::width(name) as u16;
    idx_width + TAB_INNER_PAD + name_width + TAB_INNER_PAD
}

/// Column ranges (start, end) for each tab in the vertical/tabs layout,
/// computed from session names alone. Used by the renderer to place
/// tabs and by state to map a click column back to a tab index.
pub fn tab_col_ranges(names: &[&str]) -> Vec<(u16, u16)> {
    let mut ranges = Vec::with_capacity(names.len());
    let mut x: u16 = TAB_LEADING_PAD;
    for (i, name) in names.iter().enumerate() {
        let width = tab_width(i, name);
        ranges.push((x, x + width));
        x += width;
        if i + 1 < names.len() {
            x += TAB_SEPARATOR.chars().count() as u16;
        }
    }
    ranges
}

pub fn context_menu_width(items: &[&str]) -> u16 {
    let max_len = items.iter().map(|s| s.len()).max().unwrap_or(0);
    (max_len as u16) + 4 // 1 border + 1 padding each side + 1 border
}

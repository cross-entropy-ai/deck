//! Pure geometry helpers shared by the UI (drawing) and state
//! (hit-testing) layers. Keeping them in a neutral module breaks the
//! otherwise-circular "state imports ui for hit-testing" dependency.
//!
//! The tab bar's geometry (leading pad, inner pad, separator) is
//! defined here as the single source of truth. Renderer and hit-tester
//! both read from these constants/helpers, so tweaking the tab visual
//! width automatically keeps click-target math in sync.

use unicode_width::UnicodeWidthStr;

use super::text::truncate;
use crate::state::ViewMode;

/// Max display width of each side of a remote tab label. A remote tab
/// reads `host:session`; capping both sides keeps the whole label within
/// 13 columns (6 + ":" + 6), an ellipsis taking over past that.
pub const TAB_REMOTE_SIDE_MAX: usize = 6;

/// Visible label for a session tab in the vertical/tabs layout.
///
/// Local sessions (`host == None`) show their bare name. Remote sessions
/// show `host:session`, each side truncated to `TAB_REMOTE_SIDE_MAX`; a
/// loading placeholder (empty name) shows just the host. Shared by the
/// tab renderer and the click hit-tester so tab widths and click targets
/// can't drift apart.
pub fn tab_label(host: Option<&str>, name: &str) -> String {
    match host {
        None => name.to_string(),
        Some(host) if name.is_empty() => truncate(host, TAB_REMOTE_SIDE_MAX),
        Some(host) => format!(
            "{}:{}",
            truncate(host, TAB_REMOTE_SIDE_MAX),
            truncate(name, TAB_REMOTE_SIDE_MAX),
        ),
    }
}

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
/// `AppState::focus_at_row` stays in sync with the sidebar renderer.
pub const fn plugin_block_rows(count: usize) -> u16 {
    if count == 0 {
        0
    } else {
        count as u16 + 2
    }
}

pub fn card_height(view_mode: ViewMode) -> usize {
    match view_mode {
        // Expanded: name+indicator line, idle+dir line, gutter.
        ViewMode::Expanded => 3,
        // Compact: single line. Name is prefixed with origin
        // (`local:` or `<host>:`) instead of a separate dir line.
        ViewMode::Compact => 1,
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

#[cfg(test)]
#[path = "../../tests/unit/ui/layout.rs"]
mod tests;

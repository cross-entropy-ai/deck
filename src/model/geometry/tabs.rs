//! The vertical layout's tab bar: label composition, per-tab width, and the
//! scrolling window of visible tabs.
//!
//! Pads and separators are the single source of truth for both the renderer
//! and the hit-tester, so changing a tab's width keeps click targets in step
//! by construction.

use unicode_width::UnicodeWidthStr;

use super::text::{truncate, MENU_LABEL, TAB_SEPARATOR};

/// Max display width of each side of a remote tab label. A remote tab
/// reads `host:session`; capping both sides keeps the whole label within
/// 13 columns (6 + ":" + 6), an ellipsis taking over past that.
pub const TAB_REMOTE_SIDE_MAX: usize = 6;

/// Visible label for a session tab in the vertical/tabs layout. Local
/// (`host == None`) shows the bare name; remote shows `host:session`, each
/// side truncated to `TAB_REMOTE_SIDE_MAX`; a loading placeholder (empty
/// name) shows just the host. Shared by tab renderer and hit-tester so
/// widths and click targets can't drift apart.
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
const TAB_OVERFLOW_RUN_WIDTH: u16 = 2; // marker + one separating space

fn tab_width(index: usize, name: &str) -> u16 {
    let idx_width = format!("{}", index + 1).len() as u16;
    let name_width = UnicodeWidthStr::width(name) as u16;
    idx_width
        .saturating_add(TAB_INNER_PAD)
        .saturating_add(name_width)
        .saturating_add(TAB_INNER_PAD)
}

/// One visible tab and its half-open content-column range. `index` is the
/// original flat session index, not its position inside the visible window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleTab {
    pub index: usize,
    pub start: u16,
    pub end: u16,
}

/// Geometry for the single-row vertical tab bar. The renderer and click
/// decoder both build this value, keeping overflow/windowing hit targets exact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabBarLayout {
    pub tabs: Vec<VisibleTab>,
    pub left_clipped: bool,
    pub right_clipped: bool,
    /// Content-relative column where the pinned menu label begins.
    pub menu_x: Option<u16>,
}

fn tab_run_width(labels: &[&str], start: usize, end: usize) -> u32 {
    let tabs = (start..end)
        .map(|i| u32::from(tab_width(i, labels[i])))
        .sum::<u32>();
    let separators = end.saturating_sub(start + 1) as u32;
    u32::from(TAB_LEADING_PAD)
        + if start > 0 {
            u32::from(TAB_OVERFLOW_RUN_WIDTH)
        } else {
            0
        }
        + tabs
        + separators
        + if end < labels.len() {
            u32::from(TAB_OVERFLOW_RUN_WIDTH)
        } else {
            0
        }
}

/// Window vertical session tabs around `focused`, reserving a pinned menu at
/// the right. Whole neighboring tabs are added symmetrically while they fit;
/// an unusually long focused label is truncated by the renderer to its range.
pub fn tab_bar_layout(labels: &[&str], focused: usize, width: u16) -> TabBarLayout {
    let menu_width = MENU_LABEL.width() as u16;
    let menu_x =
        (width >= menu_width.saturating_add(1)).then(|| width.saturating_sub(menu_width + 1));
    // Keep one quiet cell between tabs/overflow and the pinned menu.
    let tabs_limit = menu_x.map_or(width, |x| x.saturating_sub(1));

    if labels.is_empty() || tabs_limit <= TAB_LEADING_PAD {
        return TabBarLayout {
            tabs: Vec::new(),
            left_clipped: false,
            right_clipped: false,
            menu_x,
        };
    }

    let focused = focused.min(labels.len() - 1);
    let mut start = focused;
    let mut end = focused + 1;
    loop {
        let left_width = (start > 0).then(|| tab_run_width(labels, start - 1, end));
        let right_width = (end < labels.len()).then(|| tab_run_width(labels, start, end + 1));
        let left_fits = left_width.is_some_and(|w| w <= u32::from(tabs_limit));
        let right_fits = right_width.is_some_and(|w| w <= u32::from(tabs_limit));

        match (left_fits, right_fits) {
            (false, false) => break,
            (true, false) => start -= 1,
            (false, true) => end += 1,
            (true, true) => {
                let left_count = focused - start;
                let right_count = end - focused - 1;
                if left_count < right_count
                    || (left_count == right_count && left_width <= right_width)
                {
                    start -= 1;
                } else {
                    end += 1;
                }
            }
        }
    }

    let left_clipped = start > 0;
    let right_clipped = end < labels.len();
    let mut cursor = TAB_LEADING_PAD
        + if left_clipped {
            TAB_OVERFLOW_RUN_WIDTH
        } else {
            0
        };
    let right_reserve = if right_clipped {
        TAB_OVERFLOW_RUN_WIDTH
    } else {
        0
    };
    let mut tabs = Vec::with_capacity(end - start);
    for (i, label) in labels.iter().enumerate().take(end).skip(start) {
        let room = tabs_limit
            .saturating_sub(cursor)
            .saturating_sub(right_reserve);
        if room == 0 {
            break;
        }
        let width = tab_width(i, label).min(room);
        tabs.push(VisibleTab {
            index: i,
            start: cursor,
            end: cursor + width,
        });
        cursor += width;
        if i + 1 < end {
            cursor = cursor.saturating_add(TAB_SEPARATOR.width() as u16);
        }
    }

    TabBarLayout {
        tabs,
        left_clipped,
        right_clipped,
        menu_x,
    }
}

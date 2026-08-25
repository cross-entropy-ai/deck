use super::{
    context_menu_rect, context_menu_width, header_button_ranges, pane_areas, sidebar_areas,
    tab_bar_layout, tab_label, MENU_LABEL,
};
use crate::menu::MenuItem;
use crate::state::LayoutMode;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

#[test]
fn horizontal_bordered_panes_share_one_frame_geometry() {
    let full = Rect::new(0, 0, 120, 40);
    let panes = pane_areas(full, LayoutMode::Horizontal, 28, 3, true);

    assert_eq!(panes.shared_frame, Some(full));
    assert_eq!(panes.sidebar, Rect::new(1, 1, 27, 38));
    assert_eq!(panes.sidebar_content, panes.sidebar);
    assert_eq!(panes.sidebar_hit, Rect::new(0, 0, 28, 40));
    assert_eq!(panes.divider, Some(Rect::new(28, 1, 1, 38)));
    assert_eq!(panes.divider_hit, panes.divider);
    assert_eq!(panes.main, Rect::new(29, 1, 90, 38));
    assert_eq!(panes.main_content, panes.main);
}

#[test]
fn horizontal_borderless_panes_keep_wide_divider_hit_target() {
    let panes = pane_areas(
        Rect::new(0, 0, 120, 40),
        LayoutMode::Horizontal,
        28,
        3,
        false,
    );

    assert_eq!(panes.sidebar, Rect::new(0, 0, 28, 40));
    assert_eq!(panes.divider, Some(Rect::new(28, 0, 1, 40)));
    assert_eq!(panes.divider_hit, Some(Rect::new(28, 0, 2, 40)));
    assert_eq!(panes.main_content, Rect::new(29, 0, 91, 40));
    assert_eq!(panes.shared_frame, None);
}

#[test]
fn vertical_bordered_panes_publish_exact_terminal_content() {
    let panes = pane_areas(Rect::new(0, 0, 120, 40), LayoutMode::Vertical, 28, 3, true);

    assert_eq!(panes.sidebar, Rect::new(0, 0, 120, 3));
    assert_eq!(panes.sidebar_content, Rect::new(1, 1, 118, 1));
    assert_eq!(panes.divider, None);
    assert_eq!(panes.main, Rect::new(0, 3, 120, 37));
    assert_eq!(panes.main_content, Rect::new(1, 4, 118, 35));
}

#[test]
fn sidebar_split_pins_summary_above_footer() {
    let areas = sidebar_areas(Rect::new(1, 1, 27, 38), true, 5);

    assert_eq!(areas.header, Rect::new(1, 1, 27, 2));
    assert_eq!(areas.body, Rect::new(1, 3, 27, 33));
    assert_eq!(areas.list, Rect::new(1, 3, 27, 28));
    assert_eq!(areas.summary, Rect::new(1, 31, 27, 5));
    assert_eq!(areas.footer, Rect::new(1, 36, 27, 3));
    assert!(areas.banner_visible);
}

#[test]
fn section_header_buttons_are_right_aligned_by_display_width() {
    let buttons = vec!["[+]".to_string(), "[关闭]".to_string()];
    assert_eq!(header_button_ranges(14, &buttons), vec![4..7, 8..14]);
    assert!(header_button_ranges(4, &buttons).is_empty());
}

#[test]
fn remote_tab_label_truncates_each_side_to_six() {
    // Each side caps at 6 columns (5 chars + ellipsis), joined by ":".
    let label = tab_label(Some("longhostname"), "longsession");
    assert_eq!(label, "longh…:longs…");
    assert_eq!(label.chars().count(), 13);
}

#[test]
fn remote_tab_label_keeps_six_char_sides_whole() {
    // Exactly 6 on each side fits without an ellipsis: 6 + ":" + 6 = 13.
    assert_eq!(tab_label(Some("abcdef"), "ghijkl"), "abcdef:ghijkl");
}

#[test]
fn loading_remote_tab_label_is_host_only() {
    // Placeholder rows have no session name yet — show just the host,
    // no trailing colon.
    assert_eq!(tab_label(Some("box"), ""), "box");
}

#[test]
fn tab_ranges_size_cjk_by_display_width_not_bytes() {
    // "你好" is 2 chars / 6 bytes / 4 display columns. The tab width must
    // reserve 4 columns for the name (not 6), so a byte-length sizing
    // would over-reserve and shift every following tab's click target.
    // Layout: leading pad(1) + idx "1"(1) + inner pad(1) + name + inner pad(1).
    let layout = tab_bar_layout(&["你好"], 0, 40);
    let tab = layout.tabs[0];
    let (start, end) = (tab.start, tab.end);
    assert_eq!(start, 1); // TAB_LEADING_PAD
    assert_eq!(end - start, 1 + 1 + 4 + 1); // idx + pad + name(4 cols) + pad
}

#[test]
fn tab_bar_windows_around_focus_and_pins_menu() {
    let labels = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
    ];
    let width = 32;
    let layout = tab_bar_layout(&labels, 4, width);

    assert!(layout.tabs.iter().any(|tab| tab.index == 4));
    assert!(
        layout.left_clipped,
        "earlier tabs should collapse behind an ellipsis"
    );
    assert!(
        layout.right_clipped,
        "the final tab should remain behind an ellipsis"
    );
    assert_eq!(
        layout.menu_x,
        Some(width - MENU_LABEL.width() as u16 - 1),
        "menu stays pinned even when the tab run overflows"
    );
    assert!(
        layout
            .tabs
            .iter()
            .all(|tab| tab.end < layout.menu_x.unwrap()),
        "tab hit ranges must stop before the menu"
    );
}

#[test]
fn long_focused_tab_is_clamped_before_pinned_menu() {
    let layout = tab_bar_layout(&["a-session-name-that-is-far-too-long"], 0, 20);
    let tab = layout.tabs[0];
    assert_eq!(tab.index, 0);
    assert!(tab.end < layout.menu_x.unwrap());
}

#[test]
fn context_menu_width_uses_display_width() {
    // Menu labels are ASCII today; this guards the display-width sweep so a
    // future wide-char label sizes the popup by columns, not bytes.
    // "Remove from list" is the widest stock label at 16 columns; +4 chrome.
    let items = [MenuItem::RemoveFromList];
    assert_eq!(context_menu_width(&items), 16 + 4);
}

#[test]
fn context_menu_keeps_one_cell_screen_margin() {
    let items = [MenuItem::Rename, MenuItem::Close];

    let left = context_menu_rect(&items, 0, 0, 40, 12);
    assert_eq!(left.x, 1);
    assert_eq!(left.y, 1);

    let right = context_menu_rect(&items, 39, 11, 40, 12);
    assert_eq!(right.right(), 39);
    assert_eq!(right.bottom(), 11);
}

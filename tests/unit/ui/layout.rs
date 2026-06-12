use super::{context_menu_width, tab_col_ranges, tab_label};
use crate::menu::MenuItem;

#[test]
fn local_tab_label_is_bare_name() {
    assert_eq!(tab_label(None, "alpha"), "alpha");
}

#[test]
fn remote_tab_label_joins_host_and_session() {
    assert_eq!(tab_label(Some("box"), "work"), "box:work");
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
    let ranges = tab_col_ranges(&["你好"]);
    let (start, end) = ranges[0];
    assert_eq!(start, 1); // TAB_LEADING_PAD
    assert_eq!(end - start, 1 + 1 + 4 + 1); // idx + pad + name(4 cols) + pad
}

#[test]
fn context_menu_width_uses_display_width() {
    // Menu labels are ASCII today; this guards the display-width sweep so a
    // future wide-char label sizes the popup by columns, not bytes.
    // "Remove from list" is the widest stock label at 16 columns; +4 chrome.
    let items = [MenuItem::RemoveFromList];
    assert_eq!(context_menu_width(&items), 16 + 4);
}

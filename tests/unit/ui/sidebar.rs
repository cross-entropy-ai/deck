use super::*;

#[test]
fn plugin_block_rows_is_zero_without_plugins() {
    assert_eq!(plugin_block_rows(0), 0);
}

#[test]
fn plugin_block_rows_counts_title_and_separator() {
    // N plugins render as: title + N rows + trailing separator = N + 2.
    assert_eq!(plugin_block_rows(3), 5);
}

#[test]
fn reconnect_glyph_color_follows_status() {
    use crate::state::HostStatus;
    let theme = &crate::theme::THEMES[0];
    let accent = theme.teal;
    for (status, expected) in [
        (HostStatus::Connected, theme.teal), // unified with the divider accent
        (HostStatus::Connecting, theme.yellow),
        (HostStatus::Unreachable, theme.pink),
    ] {
        let mut lines = Vec::new();
        super::render_group_header(&mut lines, "@h", accent, status, 40, theme);
        let glyph = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "[\u{27f3}]")
            .expect("reconnect glyph span present");
        assert_eq!(glyph.style.fg, Some(expected), "status {status:?}");
    }
}

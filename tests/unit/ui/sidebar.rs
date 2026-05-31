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
        super::render_group_header(&mut lines, "@h", accent, status, 40, theme, None);
        let glyph = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "[\u{27f3}]")
            .expect("reconnect glyph span present");
        assert_eq!(glyph.style.fg, Some(expected), "status {status:?}");
    }
}

#[test]
fn pf_badge_does_not_shift_right_aligned_buttons() {
    use crate::state::{HostStatus, PfBadge, PfBadgeColor};
    let theme = &crate::theme::THEMES[0];

    let mut without = Vec::new();
    let hits_no =
        super::render_group_header(&mut without, "@h", theme.teal, HostStatus::Connected, 60, theme, None);

    let mut with = Vec::new();
    let hits_yes = super::render_group_header(
        &mut with,
        "@h",
        theme.teal,
        HostStatus::Connected,
        60,
        theme,
        Some(PfBadge { count: 2, color: PfBadgeColor::Healthy }),
    );

    // The badge eats into the dash run, so the buttons stay put.
    assert_eq!(hits_yes.reconnect.start, hits_no.reconnect.start, "reconnect button must not move");
    assert_eq!(hits_yes.more.start, hits_no.more.start, "more button must not move");

    // No forwards => no badge hit; with forwards => a clickable badge region.
    assert!(hits_no.badge.is_none(), "badge hit must be absent with no forwards");
    let badge = hits_yes.badge.expect("badge hit region must be present");

    // And the badge text is actually rendered, within the reported hit range.
    let rendered: String = with[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(rendered.contains("\u{21c4}2"), "badge text missing: {rendered:?}");
    assert!(badge.end <= hits_yes.reconnect.start, "badge must sit left of the buttons");
}

#[test]
fn pf_badge_suppressed_at_narrow_width_keeps_buttons_on_screen() {
    use crate::state::{HostStatus, PfBadge, PfBadgeColor};
    let theme = &crate::theme::THEMES[0];
    let width = 14; // too narrow to fit a badge + dash run
    let mut lines = Vec::new();
    let hits = super::render_group_header(
        &mut lines,
        "@h",
        theme.teal,
        HostStatus::Connected,
        width,
        theme,
        Some(PfBadge { count: 12, color: PfBadgeColor::Degraded }),
    );
    // Both button ranges must stay within the line width.
    assert!(hits.more.end <= width, "more button end {} exceeds width {}", hits.more.end, width);
    // Badge must be suppressed: no hit region and no ⇄ glyph at this width.
    assert!(hits.badge.is_none(), "badge hit must be absent at narrow width");
    let rendered: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(!rendered.contains('\u{21c4}'), "badge should be hidden at narrow width: {rendered:?}");
}

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
fn pf_badge_does_not_shift_right_aligned_buttons() {
    use crate::state::{HostStatus, PfBadge, PfBadgeColor};
    let theme = &crate::theme::THEMES[0];

    let mut without = Vec::new();
    let (recon_no, more_no) =
        super::render_group_header(&mut without, "@h", theme.teal, HostStatus::Connected, 60, theme, None);

    let mut with = Vec::new();
    let (recon_yes, more_yes) = super::render_group_header(
        &mut with,
        "@h",
        theme.teal,
        HostStatus::Connected,
        60,
        theme,
        Some(PfBadge { count: 2, color: PfBadgeColor::Healthy }),
    );

    // The badge eats into the dash run, so the buttons stay put.
    assert_eq!(recon_yes.start, recon_no.start, "reconnect button must not move");
    assert_eq!(more_yes.start, more_no.start, "more button must not move");

    // And the badge text is actually rendered.
    let rendered: String = with[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(rendered.contains("\u{21c4}2"), "badge text missing: {rendered:?}");
}

#[test]
fn pf_badge_suppressed_at_narrow_width_keeps_buttons_on_screen() {
    use crate::state::{HostStatus, PfBadge, PfBadgeColor};
    let theme = &crate::theme::THEMES[0];
    let width = 14; // too narrow to fit a badge + dash run
    let mut lines = Vec::new();
    let (_recon, more) = super::render_group_header(
        &mut lines,
        "@h",
        theme.teal,
        HostStatus::Connected,
        width,
        theme,
        Some(PfBadge { count: 12, color: PfBadgeColor::Degraded }),
    );
    // Both button ranges must stay within the line width.
    assert!(more.end <= width, "more button end {} exceeds width {}", more.end, width);
    // Badge must be suppressed (no ⇄ glyph) at this width.
    let rendered: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(!rendered.contains('\u{21c4}'), "badge should be hidden at narrow width: {rendered:?}");
}

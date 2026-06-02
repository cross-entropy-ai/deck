use super::tab_label;

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

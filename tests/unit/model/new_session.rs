use super::*;
use std::path::PathBuf;

#[test]
fn split_input_trailing_slash() {
    assert_eq!(split_input("~/foo/"), ("~/foo/", ""));
}

#[test]
fn split_input_partial_leaf() {
    assert_eq!(split_input("~/foo/ba"), ("~/foo/", "ba"));
}

#[test]
fn split_input_no_slash() {
    assert_eq!(split_input("foo"), ("", "foo"));
}

#[test]
fn filter_entries_prefix_case_insensitive() {
    let entries = vec!["Documents".into(), "Downloads".into(), "src".into()];
    assert_eq!(filter_entries(&entries, "doc"), vec![0]);
    assert_eq!(filter_entries(&entries, "DO"), vec![0, 1]);
}

#[test]
fn filter_entries_hides_dotfiles_when_leaf_clean() {
    let entries = vec![".git".into(), "src".into()];
    assert_eq!(filter_entries(&entries, ""), vec![1]);
    assert_eq!(filter_entries(&entries, "s"), vec![1]);
}

#[test]
fn filter_entries_shows_dotfiles_when_leaf_starts_with_dot() {
    let entries = vec![".git".into(), ".cargo".into(), "src".into()];
    assert_eq!(filter_entries(&entries, "."), vec![0, 1]);
    assert_eq!(filter_entries(&entries, ".gi"), vec![0]);
}

#[test]
fn parent_entry_is_first_and_visible_without_filter() {
    let entries = with_parent_entry(vec!["src".into(), "target".into()]);
    assert_eq!(entries, vec!["..", "src", "target"]);
    assert_eq!(filter_entries(&entries, ""), vec![0, 1, 2]);
    assert_eq!(filter_entries(&entries, "src"), vec![1]);
}

#[test]
fn parent_directory_collapses_segments_and_can_walk_above_home() {
    assert_eq!(parent_directory("~/foo/"), "~/");
    assert_eq!(parent_directory("~/"), "~/../");
    assert_eq!(parent_directory("~/../"), "~/../../");
    assert_eq!(parent_directory("/foo/"), "/");
    assert_eq!(parent_directory("/"), "/");
}

#[test]
fn expand_path_tilde() {
    let home = PathBuf::from("/home/u");
    assert_eq!(expand_path("~", &home), PathBuf::from("/home/u"));
    assert_eq!(expand_path("~/foo", &home), PathBuf::from("/home/u/foo"));
}

#[test]
fn expand_path_absolute() {
    let home = PathBuf::from("/home/u");
    assert_eq!(
        expand_path("/etc/hosts", &home),
        PathBuf::from("/etc/hosts")
    );
}

#[test]
fn expand_path_relative_resolves_under_home() {
    let home = PathBuf::from("/home/u");
    assert_eq!(
        expand_path("projects/foo", &home),
        PathBuf::from("/home/u/projects/foo")
    );
}

#[test]
fn expand_path_normalizes_parent_dir() {
    let home = PathBuf::from("/home/u");
    assert_eq!(
        expand_path("~/foo/../bar", &home),
        PathBuf::from("/home/u/bar")
    );
    assert_eq!(expand_path("~/./bar", &home), PathBuf::from("/home/u/bar"));
}

#[test]
fn auto_session_name_picks_start_when_free() {
    let names: Vec<&str> = vec![];
    assert_eq!(auto_session_name(&names, 0), "session-0");
}

#[test]
fn auto_session_name_skips_taken_indices() {
    let names = vec!["session-0", "session-1"];
    assert_eq!(auto_session_name(&names, 2), "session-2");
    // Search starts at `start`; it does NOT fill gaps below.
    assert_eq!(auto_session_name(&names, 0), "session-2");
}

#[test]
fn auto_session_name_skips_non_session_collisions_too() {
    let names = vec!["foo", "bar", "session-3"];
    assert_eq!(auto_session_name(&names, 3), "session-4");
}

/// A picker over `~/` listing `entries`, with the synthetic parent row.
fn picker_at_home(entries: Vec<String>) -> NewSessionState {
    let mut ns = NewSessionState {
        picker: crate::picker::FilterPicker::new(with_parent_entry(entries)),
        ..NewSessionState::default()
    };
    ns.picker.input = make_textarea("~/");
    ns.refilter();
    ns
}

#[test]
fn fresh_listing_highlights_the_first_child_not_the_parent_row() {
    let ns = picker_at_home(vec!["src".into(), "target".into()]);
    assert_eq!(ns.picker.selected, 1);
    assert_eq!(ns.entry_at(ns.picker.selected), Some("src"));
}

#[test]
fn stepping_skips_the_parent_row_in_both_directions() {
    let mut ns = picker_at_home(vec!["src".into(), "target".into()]);

    // Down from the last child wraps past `..` onto the first child.
    ns.step_selection(1);
    assert_eq!(ns.entry_at(ns.picker.selected), Some("target"));
    ns.step_selection(1);
    assert_eq!(ns.entry_at(ns.picker.selected), Some("src"));

    // Up from the first child wraps past `..` onto the last child.
    ns.step_selection(-1);
    assert_eq!(ns.entry_at(ns.picker.selected), Some("target"));
}

#[test]
fn parent_row_holds_the_highlight_when_it_is_the_only_row() {
    // An empty directory: there is no child to move to, so the highlight
    // stays put rather than spinning.
    let mut ns = picker_at_home(vec![]);
    assert!(ns.is_parent_row(ns.picker.selected));
    ns.step_selection(1);
    assert!(ns.is_parent_row(ns.picker.selected));
    ns.step_selection(-1);
    assert!(ns.is_parent_row(ns.picker.selected));
}

#[test]
fn path_after_entering_appends_children_and_walks_up_for_the_parent_row() {
    let ns = picker_at_home(vec!["src".into()]);
    assert_eq!(ns.path_after_entering(1).as_deref(), Some("~/src/"));
    assert_eq!(ns.path_after_entering(0).as_deref(), Some("~/../"));
    assert_eq!(ns.path_after_entering(9), None);
}

#[test]
fn path_after_entering_keeps_a_partially_typed_leaf_out_of_the_result() {
    // Typing narrows the list; opening a match must replace the leaf, not
    // append to it.
    let mut ns = picker_at_home(vec!["src".into(), "target".into()]);
    ns.picker.input = make_textarea("~/ta");
    ns.refilter();
    let selected = ns.picker.selected;
    assert_eq!(ns.entry_at(selected), Some("target"));
    assert_eq!(
        ns.path_after_entering(selected).as_deref(),
        Some("~/target/")
    );
}

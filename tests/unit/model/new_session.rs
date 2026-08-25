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

#[test]
fn the_parent_row_is_never_scrolled_out_of_view() {
    let children: Vec<String> = (0..40).map(|index| format!("child-{index:02}")).collect();
    let mut ns = picker_at_home(children);

    // Walk the whole list; the window may move, but never over the pinned row.
    for _ in 0..60 {
        ns.step_selection(1);
        assert_eq!(ns.pinned_rows(), 1);
        assert!(
            ns.scroll >= 1,
            "scroll {} would hide the pinned `..` row",
            ns.scroll
        );
        // The selection stays inside the rows left after pinning.
        if ns.picker.selected >= 1 {
            let rows = DIRECTORY_VIEW_ROWS - 1;
            assert!(
                (ns.scroll..ns.scroll + rows).contains(&ns.picker.selected),
                "selection {} outside window {}..{}",
                ns.picker.selected,
                ns.scroll,
                ns.scroll + rows
            );
        }
    }
}

#[test]
fn filtering_the_parent_row_away_gives_its_row_back_to_the_children() {
    // Typing a leaf that `..` can't match drops it, and then nothing is
    // pinned — the list scrolls as a plain one.
    let mut ns = picker_at_home(vec!["src".into(), "target".into()]);
    assert_eq!(ns.pinned_rows(), 1);
    ns.picker.input = make_textarea("~/s");
    ns.refilter();
    assert_eq!(ns.pinned_rows(), 0);
    assert_eq!(ns.scroll, 0);
    assert_eq!(ns.entry_at(ns.picker.selected), Some("src"));
}

#[test]
fn field_chord_recognises_decks_chords_and_nothing_else() {
    let chord = |code, mods| field_chord(&KeyEvent::new(code, mods));
    assert_eq!(
        chord(KeyCode::Char('u'), KeyModifiers::CONTROL),
        Some(FieldChord::ClearLine)
    );
    assert_eq!(
        chord(KeyCode::Backspace, KeyModifiers::SUPER),
        Some(FieldChord::ClearLine)
    );
    assert_eq!(
        chord(KeyCode::Backspace, KeyModifiers::CONTROL),
        Some(FieldChord::DeleteWordBack)
    );
    assert_eq!(
        chord(KeyCode::Delete, KeyModifiers::CONTROL),
        Some(FieldChord::DeleteWordForward)
    );

    // A plain `u`, plain Backspace/Delete, and Alt-Backspace (the widget's own
    // delete-word) are ordinary edits the widget keeps.
    assert_eq!(chord(KeyCode::Char('u'), KeyModifiers::NONE), None);
    assert_eq!(chord(KeyCode::Backspace, KeyModifiers::NONE), None);
    assert_eq!(chord(KeyCode::Delete, KeyModifiers::NONE), None);
    assert_eq!(chord(KeyCode::Backspace, KeyModifiers::ALT), None);
}

#[test]
fn textarea_input_ctrl_backspace_and_ctrl_delete_remove_one_word() {
    let mut ta = make_textarea("foo bar baz");
    let ctrl_backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL);
    assert!(textarea_input(&mut ta, ctrl_backspace));
    assert_eq!(textarea_line(&ta), "foo bar ");
    assert!(textarea_input(&mut ta, ctrl_backspace));
    assert_eq!(textarea_line(&ta), "foo ");

    let mut ta = make_textarea("foo bar baz");
    ta.move_cursor(CursorMove::Head);
    let ctrl_delete = KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL);
    assert!(textarea_input(&mut ta, ctrl_delete));
    assert_eq!(textarea_line(&ta), " bar baz");
    // Nothing ahead of the cursor: nothing to delete.
    ta.move_cursor(CursorMove::End);
    assert!(!textarea_input(&mut ta, ctrl_delete));
}

#[test]
fn textarea_input_clear_chord_empties_the_line_from_any_cursor_position() {
    let mut ta = make_textarea("hello world");
    ta.move_cursor(CursorMove::Head);
    ta.move_cursor(CursorMove::WordForward);
    assert_ne!(ta.cursor().1, 0, "cursor sits mid-line before the clear");

    let ctrl_u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
    assert!(textarea_input(&mut ta, ctrl_u));
    assert_eq!(textarea_line(&ta), "");
    // Clearing an already empty line changes nothing.
    assert!(!textarea_input(&mut ta, ctrl_u));

    let mut ta = make_textarea("~/projects/deck");
    let cmd_backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::SUPER);
    assert!(textarea_input(&mut ta, cmd_backspace));
    assert_eq!(textarea_line(&ta), "");
}

#[test]
fn textarea_input_defers_every_other_key_to_the_widget() {
    let mut ta = make_textarea("foo bar");
    // Ctrl-W is the widget's delete-word; a plain character inserts.
    textarea_input(
        &mut ta,
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    assert_eq!(textarea_line(&ta), "foo ");
    textarea_input(
        &mut ta,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
    );
    assert_eq!(textarea_line(&ta), "foo x");
}

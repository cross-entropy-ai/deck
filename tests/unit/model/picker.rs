use crate::new_session::make_textarea;
use crate::picker::FilterPicker;

fn items() -> Vec<String> {
    vec!["alpha".into(), "alpine".into(), "beta".into()]
}

/// Case-insensitive substring filter, used to drive `refilter` in tests.
fn substr(items: &[String], needle: &str) -> Vec<usize> {
    let needle = needle.to_ascii_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, s)| needle.is_empty() || s.to_ascii_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn new_shows_all_items() {
    let p = FilterPicker::new(items());
    assert_eq!(p.filtered, vec![0, 1, 2]);
    assert_eq!(p.selected, 0);
    assert_eq!(p.selected_item(), Some("alpha"));
}

#[test]
fn refilter_recomputes_and_clamps_selected() {
    let mut p = FilterPicker::new(items());
    p.selected = 2;
    p.input = make_textarea("alp");
    p.refilter(substr);
    // "alpha" + "alpine" match; selected was past the new end, clamped down.
    assert_eq!(p.filtered, vec![0, 1]);
    assert_eq!(p.selected, 1);
    assert_eq!(p.selected_item(), Some("alpine"));
}

#[test]
fn refilter_empty_match_resets_selection() {
    let mut p = FilterPicker::new(items());
    p.selected = 2;
    p.input = make_textarea("zzz");
    p.refilter(substr);
    assert!(p.filtered.is_empty());
    assert_eq!(p.selected, 0);
    assert_eq!(p.selected_item(), None);
}

#[test]
fn step_is_clamped_at_both_ends() {
    let mut p = FilterPicker::new(items()); // filtered = [0,1,2]
    p.step(-1); // already at 0, stays
    assert_eq!(p.selected, 0);
    p.step(1);
    p.step(1);
    p.step(1); // tries to overrun the last index
    assert_eq!(p.selected, 2);
}

#[test]
fn step_on_empty_list_is_noop() {
    let mut p = FilterPicker::new(vec![]);
    p.step(1);
    p.step(-1);
    assert_eq!(p.selected, 0);
    assert_eq!(p.selected_item(), None);
}

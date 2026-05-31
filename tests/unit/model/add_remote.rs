use crate::add_remote::{filter_hosts, AddRemoteState};

fn hosts() -> Vec<String> {
    vec!["prod-web-1".into(), "prod-web-2".into(), "staging".into()]
}

#[test]
fn filter_empty_matches_all() {
    assert_eq!(filter_hosts(&hosts(), ""), vec![0, 1, 2]);
    assert_eq!(filter_hosts(&hosts(), "   "), vec![0, 1, 2]);
}

#[test]
fn filter_is_case_insensitive_substring() {
    assert_eq!(filter_hosts(&hosts(), "WEB"), vec![0, 1]);
    assert_eq!(filter_hosts(&hosts(), "stag"), vec![2]);
    assert!(filter_hosts(&hosts(), "nope").is_empty());
}

#[test]
fn new_shows_all_and_refilter_clamps_selected() {
    let mut s = AddRemoteState::new(hosts());
    assert_eq!(s.filtered, vec![0, 1, 2]);
    s.selected = 2;
    s.input = crate::add_remote::make_textarea("stag");
    s.refilter();
    assert_eq!(s.filtered, vec![2]);
    assert_eq!(s.selected, 0);
}

#[test]
fn chosen_host_prefers_highlighted_then_typed() {
    let mut s = AddRemoteState::new(hosts());
    s.selected = 1;
    assert_eq!(s.chosen_host().as_deref(), Some("prod-web-2"));

    s.input = crate::add_remote::make_textarea("brand-new-host");
    s.refilter();
    assert!(s.filtered.is_empty());
    assert_eq!(s.chosen_host().as_deref(), Some("brand-new-host"));

    let mut empty = AddRemoteState::new(vec![]);
    assert_eq!(empty.chosen_host(), None);
    empty.input = crate::add_remote::make_textarea("   ");
    empty.refilter();
    assert_eq!(empty.chosen_host(), None);
}

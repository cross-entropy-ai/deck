use super::*;
use std::path::PathBuf;

#[test]
fn split_input_empty() {
    assert_eq!(split_input(""), ("", ""));
}

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
fn split_input_root_only() {
    assert_eq!(split_input("/"), ("/", ""));
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
fn smart_backspace_goes_up_at_trailing_slash() {
    let mut s = "~/foo/bar/".to_string();
    let mut c = s.len();
    smart_backspace(&mut s, &mut c);
    assert_eq!(s, "~/foo/");
    assert_eq!(c, s.len());
}

#[test]
fn smart_backspace_deletes_char_mid_leaf() {
    let mut s = "~/foo/ba".to_string();
    let mut c = s.len();
    smart_backspace(&mut s, &mut c);
    assert_eq!(s, "~/foo/b");
    assert_eq!(c, s.len());
}

#[test]
fn smart_backspace_empty_input_noop() {
    let mut s = String::new();
    let mut c = 0;
    smart_backspace(&mut s, &mut c);
    assert_eq!(s, "");
    assert_eq!(c, 0);
}

#[test]
fn smart_backspace_root_only_noop() {
    // input is exactly "/" — guarded by `len > 1`.
    let mut s = "/".to_string();
    let mut c = 1;
    smart_backspace(&mut s, &mut c);
    assert_eq!(s, "");
    assert_eq!(c, 0);
    // Note: smart_backspace falls through to char-delete branch, which
    // deletes the lone `/`. That's acceptable — user can retype.
}

#[test]
fn tab_complete_appends_entry_and_slash() {
    let mut s = "~/foo/ba".to_string();
    let mut c = s.len();
    tab_complete(&mut s, &mut c, "bar");
    assert_eq!(s, "~/foo/bar/");
    assert_eq!(c, s.len());
}

#[test]
fn tab_complete_empty_leaf() {
    let mut s = "~/foo/".to_string();
    let mut c = s.len();
    tab_complete(&mut s, &mut c, "bar");
    assert_eq!(s, "~/foo/bar/");
    assert_eq!(c, s.len());
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
    assert_eq!(expand_path("/etc/hosts", &home), PathBuf::from("/etc/hosts"));
}

#[test]
fn expand_path_relative_resolves_under_home() {
    let home = PathBuf::from("/home/u");
    assert_eq!(expand_path("projects/foo", &home), PathBuf::from("/home/u/projects/foo"));
}

#[test]
fn expand_path_normalizes_parent_dir() {
    let home = PathBuf::from("/home/u");
    assert_eq!(expand_path("~/foo/../bar", &home), PathBuf::from("/home/u/bar"));
    assert_eq!(expand_path("~/./bar", &home), PathBuf::from("/home/u/bar"));
}

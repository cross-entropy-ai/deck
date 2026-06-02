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

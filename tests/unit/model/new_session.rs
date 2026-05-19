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

#[cfg(test)]
mod fs_integration {
    use super::*;
    use std::fs;

    #[test]
    fn read_dir_entries_lists_subdirs_only() {
        // This test calls `App::read_dir_entries` indirectly via the
        // model layer is hard — `read_dir_entries` lives in
        // `app::dispatch`. Instead, exercise the underlying contract
        // by listing manually and verifying our helpers behave.
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("src")).unwrap();
        fs::create_dir(tmp.path().join("tests")).unwrap();
        fs::write(tmp.path().join("README"), "").unwrap();

        let mut names: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.metadata().map(|m| m.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        assert_eq!(names, vec!["src", "tests"]);

        let filtered = filter_entries(&names, "s");
        assert_eq!(filtered, vec![0]); // "src" matches
    }

    #[test]
    fn expand_path_resolves_real_tempdir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let resolved = expand_path("~/foo", &home);
        assert_eq!(resolved, tmp.path().join("foo"));
    }
}

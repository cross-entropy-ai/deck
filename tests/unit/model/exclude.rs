use crate::exclude::{compile_patterns, session_excluded};

#[test]
fn glob_star_matches_prefix() {
    let patterns = compile_patterns(&["_*".to_string()]);
    assert!(session_excluded("_hidden", &patterns));
    assert!(!session_excluded("visible", &patterns));
}

#[test]
fn glob_question_mark_matches_single_char() {
    let patterns = compile_patterns(&["t?st".to_string()]);
    assert!(session_excluded("test", &patterns));
    assert!(session_excluded("tast", &patterns));
    assert!(!session_excluded("toast", &patterns));
}

#[test]
fn regex_pattern_matches() {
    let patterns = compile_patterns(&["/^test-.+$/".to_string()]);
    assert!(session_excluded("test-abc", &patterns));
    assert!(!session_excluded("test-", &patterns));
    assert!(!session_excluded("my-test-abc", &patterns));
}

#[test]
fn regex_partial_match() {
    let patterns = compile_patterns(&["/scratch/".to_string()]);
    assert!(session_excluded("scratch", &patterns));
    assert!(session_excluded("my-scratch-pad", &patterns));
    assert!(!session_excluded("nothere", &patterns));
}

#[test]
fn invalid_regex_skipped() {
    let patterns = compile_patterns(&["/[invalid/".to_string()]);
    assert!(patterns.is_empty());
}

#[test]
fn multiple_patterns_any_match() {
    let patterns = compile_patterns(&["_*".to_string(), "/^test/".to_string()]);
    assert!(session_excluded("_hidden", &patterns));
    assert!(session_excluded("test-thing", &patterns));
    assert!(!session_excluded("keep-me", &patterns));
}

#[test]
fn empty_patterns_excludes_nothing() {
    let patterns = compile_patterns(&[]);
    assert!(!session_excluded("anything", &patterns));
}

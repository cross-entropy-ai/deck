use super::*;

#[test]
fn wrap_width_zero_returns_empty() {
    assert!(wrap_width("hello", 0).is_empty());
}

#[test]
fn wrap_width_fits_in_single_chunk() {
    assert_eq!(wrap_width("hi", 10), vec!["hi".to_string()]);
}

#[test]
fn wrap_width_splits_on_width_boundary() {
    let chunks = wrap_width("abcdefghijklmnop", 5);
    assert_eq!(chunks.len(), 4);
    for (i, c) in chunks.iter().enumerate() {
        let expected_len = if i < 3 { 5 } else { 1 };
        assert_eq!(c.chars().count(), expected_len, "chunk {i}: {c:?}");
    }
    assert_eq!(chunks.concat(), "abcdefghijklmnop");
}

#[test]
fn reload_row_count_ok_is_one_row() {
    assert_eq!(reload_row_count(Some(&ReloadStatus::Ok), 20), 1);
}

#[test]
fn reload_row_count_none_is_zero_rows() {
    assert_eq!(reload_row_count(None, 20), 0);
}

#[test]
fn reload_row_count_err_grows_with_length_and_caps() {
    let short = ReloadStatus::Err("boom".into());
    assert_eq!(reload_row_count(Some(&short), 30), 1);

    let long = ReloadStatus::Err("x".repeat(200));
    let rows = reload_row_count(Some(&long), 20);
    assert!(rows >= 2, "expected multi-row wrap, got {rows}");
    assert!(
        rows <= RELOAD_MAX_ROWS as u16,
        "row count {rows} exceeds cap {}",
        RELOAD_MAX_ROWS
    );
}

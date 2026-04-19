use super::*;

#[test]
fn plugin_block_rows_is_zero_without_plugins() {
    assert_eq!(plugin_block_rows(0), 0);
}

#[test]
fn plugin_block_rows_counts_title_and_separator() {
    // N plugins render as: title + N rows + trailing separator = N + 2.
    assert_eq!(plugin_block_rows(3), 5);
}

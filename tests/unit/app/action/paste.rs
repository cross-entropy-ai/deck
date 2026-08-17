use super::image_path_from_paste;

#[test]
fn plain_absolute_image_path_is_recognized() {
    assert_eq!(
        image_path_from_paste("/Users/me/shot.png"),
        Some("/Users/me/shot.png".to_string())
    );
}

/// The case the feature exists for: macOS names screenshots with spaces, and
/// terminals paste a dropped one backslash-escaped.
#[test]
fn escaped_spaces_from_a_terminal_drop_are_undone() {
    assert_eq!(
        image_path_from_paste("/Users/me/Screen\\ Shot\\ 2026-08-17\\ at\\ 09.41.02.png"),
        Some("/Users/me/Screen Shot 2026-08-17 at 09.41.02.png".to_string())
    );
}

#[test]
fn surrounding_quotes_and_trailing_space_are_stripped() {
    assert_eq!(
        image_path_from_paste("'/Users/me/a b.png'"),
        Some("/Users/me/a b.png".to_string())
    );
    assert_eq!(
        image_path_from_paste("\"/Users/me/a b.png\""),
        Some("/Users/me/a b.png".to_string())
    );
    // Terminals append one after a drop.
    assert_eq!(
        image_path_from_paste("/Users/me/a.png "),
        Some("/Users/me/a.png".to_string())
    );
}

#[test]
fn home_relative_paths_qualify() {
    assert_eq!(
        image_path_from_paste("~/Desktop/a.jpeg"),
        Some("~/Desktop/a.jpeg".to_string())
    );
}

#[test]
fn extension_match_ignores_case() {
    assert!(image_path_from_paste("/tmp/A.PNG").is_some());
    assert!(image_path_from_paste("/tmp/a.WebP").is_some());
}

/// Everything a paste is far more likely to be. Rewriting any of these into an
/// upload would be worse than doing nothing.
#[test]
fn ordinary_pastes_are_left_alone() {
    for text in [
        "",
        "   ",
        // Prose that merely names a file.
        "see /Users/me/a.png for the colors",
        // Relative: a drop is always absolute, and this could be a word.
        "logo.png",
        "./logo.png",
        // Not an image.
        "/Users/me/notes.md",
        "/Users/me/archive.png.zip",
        // No stem, so `.png` is the whole name — a dotfile, not a screenshot.
        "/Users/me/.png",
        // A URL, not a path.
        "https://example.com/a.png",
        // Several paths at once.
        "/Users/me/a.png\n/Users/me/b.png",
    ] {
        assert_eq!(image_path_from_paste(text), None, "text: {text:?}");
    }
}

/// A shell would read `\t` in an unquoted word as a `t`; so does a filename.
/// Only punctuation is ever escaped by a terminal, so only punctuation is
/// unescaped here.
#[test]
fn escapes_before_letters_are_kept_verbatim() {
    assert_eq!(
        image_path_from_paste("/tmp/a\\tb.png"),
        Some("/tmp/a\\tb.png".to_string())
    );
}

//! What a paste *is*, decided without touching the filesystem.
//!
//! A terminal never hands Deck a file — dropping one on the window pastes its
//! path as ordinary text, and that text is the only evidence there was a file
//! at all. Recognizing it here is what lets the paste path stage the bytes for
//! the lane the pane belongs to (`SessionControl::stage_file`) instead of
//! handing a path on *Deck's* machine to an agent that cannot open it.
//!
//! Deliberately IO-free: whether the path exists, and whether it is a file, is
//! `app::dispatch`'s question — the same split the new-session picker makes for
//! its `fs::metadata` check.

/// Extensions worth staging: what Claude Code and Codex actually read as
/// images. Anything else pastes as the plain text it is.
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// The single image path this paste consists of, or `None` for anything else —
/// prose that merely mentions a file, a multi-line paste, a non-image path.
///
/// Only absolute (or `~`-rooted) paths qualify. A drop always produces one, and
/// requiring it keeps a sentence like `see logo.png for the colors` from being
/// mistaken for a file to upload.
pub fn image_path_from_paste(text: &str) -> Option<String> {
    let trimmed = text.trim();
    // One path, or nothing: a drop of several files, or a path inside a
    // paragraph, is not something to silently rewrite.
    if trimmed.is_empty() || trimmed.contains(['\n', '\r']) {
        return None;
    }

    let path = unquote(trimmed);
    if !(path.starts_with('/') || path.starts_with("~/")) {
        return None;
    }
    has_image_extension(&path).then_some(path)
}

/// Undo the quoting a terminal applies when it pastes a dropped path, so the
/// result is the name the filesystem knows. Terminals differ: some wrap the
/// path in quotes, most backslash-escape the characters a shell would other-
/// wise split on — and a macOS screenshot (`Screen Shot 2026-08-17 at
/// 09.41.02.png`) arrives with three of them.
fn unquote(text: &str) -> String {
    for quote in ['\'', '"'] {
        if let Some(inner) = text
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.to_string();
        }
    }

    // Unescape only punctuation: `\t` in an unquoted paste is a backslash
    // before a `t`, not a tab, and no terminal escapes a letter of a filename.
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(escaped) if !escaped.is_alphanumeric() => out.push(escaped),
            Some(escaped) => {
                out.push('\\');
                out.push(escaped);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn has_image_extension(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && IMAGE_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/app/action/paste.rs"]
mod tests;

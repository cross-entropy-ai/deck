//! Session-name exclusion: compile user patterns (glob or `/regex/`) and
//! test session names against them. Drives `exclude_patterns` from the
//! config, but is pure filtering logic with no persistence concern.

/// A compiled exclude pattern. Globs are translated to an equivalent anchored
/// regex, so matching is one engine for both syntaxes.
pub struct ExcludePattern(regex::Regex);

/// Compile raw pattern strings into ExcludePattern values.
/// Patterns wrapped in `/…/` are treated as regex; others as glob.
/// Invalid regexes are silently skipped.
pub fn compile_patterns(raw: &[String]) -> Vec<ExcludePattern> {
    raw.iter()
        .filter_map(
            |p| match p.strip_prefix('/').and_then(|s| s.strip_suffix('/')) {
                Some(inner) => regex::Regex::new(inner).ok(),
                None => regex::Regex::new(&glob_to_regex(p)).ok(),
            },
        )
        .map(ExcludePattern)
        .collect()
}

/// Returns true if the session name matches any exclude pattern.
pub fn session_excluded(name: &str, patterns: &[ExcludePattern]) -> bool {
    patterns.iter().any(|p| p.0.is_match(name))
}

/// Translate a glob (`*` = any sequence, `?` = single char) into a whole-string
/// regex. `(?s)` keeps `?`/`*` matching any char, and every other char is
/// escaped so it can only match literally.
fn glob_to_regex(glob: &str) -> String {
    let mut out = String::from("(?s)^");
    for c in glob.chars() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    out.push('$');
    out
}

#[cfg(test)]
#[path = "../../tests/unit/model/exclude.rs"]
mod tests;

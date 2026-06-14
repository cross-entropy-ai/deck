//! Parser for `ls -1pA` directory listings used by the remote new-session
//! directory picker. The ssh call lives in `infra::remote_tmux`.

/// Keep only directory lines — those `ls -p` suffixed with `/` — and
/// strip the trailing slash. Non-directory lines (no `/`) are dropped.
pub(crate) fn parse_dir_listing(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|line| line.strip_suffix('/'))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dir_listing_keeps_dirs_drops_files() {
        // `ls -1pA` suffixes directories (incl. dotfile dirs) with `/`.
        let raw = "src/\nmain.rs\ntests/\n.config/\nREADME";
        let mut got = parse_dir_listing(raw);
        got.sort();
        assert_eq!(got, vec![".config", "src", "tests"]);
    }
}

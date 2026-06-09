//! Pure parsers for tmux's tab-separated output.
//!
//! These are shared by `infra::tmux` (local) and `infra::remote_tmux`
//! (SSH). The two callers differ in how they invoke tmux — timeouts,
//! shell quoting for the format string, error semantics — but the
//! bytes they receive back are the same shape, so the parsing lives
//! here.

use std::collections::HashMap;

use crate::infra::tmux::SessionInfo;

/// tmux user-option holding deck's persisted 0-based display rank.
/// Shared by the read format below and each backend's set-option write
/// so the two can't drift.
pub(crate) const DECK_ORDER_OPTION: &str = "@deck_order";

/// `list-sessions -F` format. The `_SSH` variant wraps the same fields
/// in bash/zsh ANSI-C `$'...'` quoting so the remote login shell treats
/// `#` literally and turns `\t` into a tab; quoting is the only intended
/// difference.
pub(crate) const SESSION_LIST_FORMAT: &str = "#{session_name}\t#{session_path}\t#{@deck_order}";
pub(crate) const SESSION_LIST_FORMAT_SSH: &str =
    "$'#{session_name}\\t#{session_path}\\t#{@deck_order}'";

/// `list-windows -a -F` format for per-session activity, local and
/// ssh-quoted variants.
pub(crate) const WINDOW_ACTIVITY_FORMAT: &str = "#{session_name}\t#{window_activity}";
pub(crate) const WINDOW_ACTIVITY_FORMAT_SSH: &str = "$'#{session_name}\\t#{window_activity}'";

/// Parse `tmux list-sessions` output. The local caller's format is
/// `#{session_name}\t#{session_path}\t#{@deck_order}`; the remote caller
/// omits the trailing rank (`#{session_name}\t#{session_path}`), so the
/// order field is optional. `window_activity` provides per-session
/// activity timestamps; sessions absent from the map get `activity = 0`.
pub(crate) fn parse_sessions(
    raw: &str,
    window_activity: &HashMap<String, u64>,
) -> Vec<SessionInfo> {
    raw.lines()
        .filter_map(|line| {
            let (name, after_name) = line.split_once('\t')?;
            // `@deck_order` (if requested) is the last field, a bare
            // integer or empty. Split it off the tail so a dir that
            // somehow contains a tab still parses. With no trailing field
            // (remote listing) there's no rank.
            let (dir, order) = match after_name.rsplit_once('\t') {
                Some((dir, rank)) => (dir, rank.parse::<u32>().ok()),
                None => (after_name, None),
            };
            let activity = window_activity.get(name).copied().unwrap_or(0);
            Some(SessionInfo {
                name: name.to_string(),
                dir: dir.to_string(),
                activity,
                order,
            })
        })
        .collect()
}

/// Parse `tmux list-windows -a -F '#{session_name}\t#{window_activity}'`
/// output, returning the max timestamp per session. Lines whose
/// timestamp doesn't parse contribute a 0 (so a session with only
/// malformed lines still appears, with activity 0).
pub(crate) fn parse_window_activity(raw: &str) -> HashMap<String, u64> {
    let mut map: HashMap<String, u64> = HashMap::new();
    for line in raw.lines() {
        if let Some((name, ts_str)) = line.split_once('\t') {
            let ts: u64 = ts_str.parse().unwrap_or(0);
            let entry = map.entry(name.to_string()).or_insert(0);
            if ts > *entry {
                *entry = ts;
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sessions_handles_normal_output() {
        let mut activity = HashMap::new();
        activity.insert("alpha".to_string(), 100u64);
        let raw = "alpha\t/tmp/alpha\nbeta\t/tmp/beta";
        let got = parse_sessions(raw, &activity);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "alpha");
        assert_eq!(got[0].dir, "/tmp/alpha");
        assert_eq!(got[0].activity, 100);
        assert_eq!(got[1].name, "beta");
        assert_eq!(got[1].activity, 0);
    }

    #[test]
    fn parse_sessions_reads_optional_deck_order() {
        let activity = HashMap::new();
        // Local format: name \t dir \t @deck_order. `beta` has an empty
        // trailing field (never reordered → no rank).
        let raw = "alpha\t/tmp/alpha\t2\nbeta\t/tmp/beta\t";
        let got = parse_sessions(raw, &activity);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].dir, "/tmp/alpha");
        assert_eq!(got[0].order, Some(2));
        assert_eq!(got[1].dir, "/tmp/beta");
        assert_eq!(got[1].order, None);
    }

    #[test]
    fn parse_sessions_two_field_remote_form_has_no_order() {
        let activity = HashMap::new();
        // Remote listing omits the trailing rank field entirely.
        let raw = "alpha\t/tmp/alpha";
        let got = parse_sessions(raw, &activity);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].dir, "/tmp/alpha");
        assert_eq!(got[0].order, None);
    }

    #[test]
    fn parse_sessions_skips_malformed_lines() {
        let activity = HashMap::new();
        let raw = "good\t/dir\nno_tab_here\nalso\tbad\textra";
        let got = parse_sessions(raw, &activity);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "good");
        assert_eq!(got[1].name, "also");
    }

    #[test]
    fn parse_sessions_empty_input() {
        let activity = HashMap::new();
        let got = parse_sessions("", &activity);
        assert!(got.is_empty());
    }

    #[test]
    fn parse_window_activity_takes_max_per_session() {
        let raw = "s1\t100\ns1\t300\ns1\t200\ns2\t50";
        let got = parse_window_activity(raw);
        assert_eq!(got.get("s1").copied(), Some(300));
        assert_eq!(got.get("s2").copied(), Some(50));
    }

    #[test]
    fn parse_window_activity_ignores_malformed_lines() {
        let raw = "s1\tabc\nbroken_line\ns2\t42";
        let got = parse_window_activity(raw);
        assert_eq!(got.get("s1").copied(), Some(0));
        assert_eq!(got.get("s2").copied(), Some(42));
    }
}

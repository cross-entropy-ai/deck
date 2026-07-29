//! Pure parsers for tmux's tab-separated output.
//!
//! Shared by `infra::tmux` (local) and `infra::remote_tmux` (SSH): the
//! callers differ in how they invoke tmux (timeouts, shell quoting, error
//! semantics) but the bytes they get back are the same shape.

use std::collections::HashMap;

use crate::tmux::SessionInfo;

/// tmux user-option holding deck's persisted 0-based display rank.
/// Shared by the read format below and each backend's set-option write
/// so the two can't drift.
pub(crate) const DECK_ORDER_OPTION: &str = "@deck_order";

/// The tmux `-t` target for a bare *session name*, forced to exact match
/// with a leading `=`. Without it tmux resolves `-t name` by exact → prefix
/// → fnmatch, so `kill-session -t work` can hit `workbench` or pick a wrong
/// prefix match.
///
/// Shared by both backends (local passes straight to tmux; remote must also
/// `shell_single_quote` it, which shields the leading `=` from zsh
/// equals-expansion). Use ONLY for bare session names: pane ids (`%N`) are
/// already exact and `-s` names are literals, not lookup targets.
pub(crate) fn exact_target(name: &str) -> String {
    format!("={name}")
}

/// Build the `set-option -t <target> @deck_order <rank>` arg chain that
/// persists display order, `separator`-joined into one tmux invocation.
/// Both backends share the loop; only the separator and target quoting
/// differ — local passes a bare `;` and unquoted targets, remote passes a
/// single-quoted `';'` and `shell_single_quote`s each target so the remote
/// shell forwards both to tmux intact. Empty `order` yields no args.
///
/// Targets here are **bare names, not [`exact_target`]**: tmux's option
/// commands resolve `-t` through a different path than `kill-session` /
/// `rename-session`, and older servers reject the `=` prefix outright
/// (tmux 3.4 answers `set-option -t '=work' …` with `no such session:
/// =work`, while `has-session`/`rename-session -t '=work'` are fine). That
/// silently dropped every rank on such a host, so a remote reorder never
/// stuck. Dropping `=` is safe for this call: tmux tries exact match first
/// and these names come from the live session list, so an existing name
/// always wins over a prefix/fnmatch sibling.
pub(crate) fn order_set_option_args(
    order: &[String],
    separator: &str,
    target: impl Fn(&str) -> String,
) -> Vec<String> {
    let mut args = Vec::with_capacity(order.len() * 6);
    for (rank, name) in order.iter().enumerate() {
        if rank > 0 {
            args.push(separator.to_string());
        }
        args.push("set-option".to_string());
        args.push("-t".to_string());
        args.push(target(name));
        args.push(DECK_ORDER_OPTION.to_string());
        args.push(rank.to_string());
    }
    args
}

/// `list-sessions -F` format. The `_SSH` variant wraps the same fields in
/// bash/zsh ANSI-C `$'...'` quoting (so the remote shell treats `#` literally
/// and turns `\t` into a tab); quoting is the only difference.
pub(crate) const SESSION_LIST_FORMAT: &str = "#{session_name}\t#{session_path}\t#{@deck_order}";
pub(crate) const SESSION_LIST_FORMAT_SSH: &str =
    "$'#{session_name}\\t#{session_path}\\t#{@deck_order}'";

/// `list-windows -a -F` format for per-session activity. Local-only (it
/// drives the most-recently-active attach pick): the remote path skips the
/// activity probe entirely, so there is no ssh-quoted variant.
pub(crate) const WINDOW_ACTIVITY_FORMAT: &str = "#{session_name}\t#{window_activity}";

/// Parse `tmux list-sessions` output (fields
/// `#{session_name}\t#{session_path}\t#{@deck_order}`). The trailing rank is
/// empty when `@deck_order` is unset, so order parses to `None`. Sessions
/// absent from `window_activity` get `activity = 0`.
pub(crate) fn parse_sessions(
    raw: &str,
    window_activity: &HashMap<String, u64>,
) -> Vec<SessionInfo> {
    raw.lines()
        .filter_map(|line| {
            let (name, after_name) = line.split_once('\t')?;
            // `@deck_order` is the last field (bare integer or empty). Split
            // off the tail so a dir containing a tab still parses; a line with
            // no trailing tab yields no rank.
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
/// output into the max timestamp per session. Unparseable timestamps
/// contribute 0 (a session with only malformed lines still appears, activity 0).
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
        // A line with no trailing rank field still parses (unset @deck_order).
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

//! Parser for `tmux list-panes` output. Produces `PaneInfo`s (defined in
//! `infra::agent`, the consumer) shared by the local and ssh gathering paths.

use crate::infra::agent::PaneInfo;

/// The `-F` format string for the pane fields `parse_panes` expects.
/// `window_name` (not `window_index`) is the *display* window field — the
/// Agents tab shows the window's name; switching still targets the stable
/// `pane_id`, so a name (possibly with spaces) here is cosmetic only.
pub const PANE_FORMAT: &str = "#{pane_pid}\t#{session_name}\t#{window_name}\t#{pane_id}";

/// Parse `tmux list-panes -F '#{pane_pid}\t#{session_name}\t#{window_name}\t#{pane_id}'`
/// output into `PaneInfo`s. Shared by the local and ssh gathering paths.
pub fn parse_panes(raw: &str) -> Vec<PaneInfo> {
    raw.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let pid = f.next()?.trim().parse::<u32>().ok()?;
            Some(PaneInfo {
                pid,
                session: f.next()?.to_string(),
                window: f.next()?.to_string(),
                pane_id: f.next()?.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_panes_reads_tab_fields() {
        // The window field now carries `#{window_name}`; names can contain
        // spaces and only `\t` separates fields, so a spaced name parses whole.
        let raw = "56578\tdeck\tnvim\t%240\n74037\ttpu-spot\tbuild server\t%243\nbad_line";
        let got = parse_panes(raw);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].pid, 56578);
        assert_eq!(got[0].session, "deck");
        assert_eq!(got[0].window, "nvim");
        assert_eq!(got[0].pane_id, "%240");
        assert_eq!(got[1].pid, 74037);
        assert_eq!(got[1].window, "build server");
        assert_eq!(got[1].pane_id, "%243");
    }
}

//! Parser for `tmux list-panes` output. Produces `PaneInfo`s (defined in
//! `infra::agent`, the consumer) shared by the local and ssh gathering paths.

use crate::infra::agent::PaneInfo;

/// The `-F` format string for the pane fields `parse_panes` expects.
/// `window_name` (not `window_index`) is the *display* window field — the
/// Agents tab shows the window's name; switching still targets the stable
/// `pane_id`, so a name (possibly with spaces) here is cosmetic only.
///
/// The tail fields feed status classification: `window_activity` +
/// `window_panes` are the activity clock, `@deck_agent_state` is the
/// optional lifecycle-hook report (see `docs/agent-status-plan.md`), and
/// `pane_title` is the OSC-title tier of `classify_verdict`. The title goes
/// LAST because it is arbitrary program-set text that may itself contain
/// tabs — everything after the seventh tab is the title.
pub const PANE_FORMAT: &str = "#{pane_pid}\t#{session_name}\t#{window_name}\t#{pane_id}\t#{window_activity}\t#{window_panes}\t#{@deck_agent_state}\t#{pane_title}";

/// Parse [`PANE_FORMAT`] lines into `PaneInfo`s. Shared by the local and ssh
/// gathering paths. The three tail fields are optional at parse time: a tmux
/// old enough not to know a format variable prints an empty string for it
/// (and a truncated line yields missing fields), either of which degrades to
/// `None`/empty — never a dropped pane.
pub fn parse_panes(raw: &str) -> Vec<PaneInfo> {
    raw.lines()
        .filter_map(|line| {
            // Title last + a bounded split: tabs inside the title stay in it.
            let mut f = line.splitn(8, '\t');
            let pid = f.next()?.trim().parse::<u32>().ok()?;
            Some(PaneInfo {
                pid,
                session: f.next()?.to_string(),
                window: f.next()?.to_string(),
                pane_id: f.next()?.to_string(),
                window_activity: f.next().and_then(|v| v.trim().parse::<u64>().ok()),
                window_panes: f.next().and_then(|v| v.trim().parse::<u32>().ok()),
                hook_state: f.next().unwrap_or_default().to_string(),
                title: f.next().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_panes_reads_tab_fields() {
        // The window field carries `#{window_name}`; names can contain
        // spaces and only `\t` separates fields, so a spaced name parses
        // whole. The title is last and keeps any tabs of its own.
        let raw = "56578\tdeck\tnvim\t%240\t1787567439\t1\tworking@1787567438\t✳ deck cleanup\n\
                   74037\ttpu-spot\tbuild server\t%243\t1787567000\t2\t\ttabby\ttitle\n\
                   bad_line";
        let got = parse_panes(raw);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].pid, 56578);
        assert_eq!(got[0].session, "deck");
        assert_eq!(got[0].window, "nvim");
        assert_eq!(got[0].pane_id, "%240");
        assert_eq!(got[0].window_activity, Some(1787567439));
        assert_eq!(got[0].window_panes, Some(1));
        assert_eq!(got[0].hook_state, "working@1787567438");
        assert_eq!(got[0].title, "✳ deck cleanup");
        assert_eq!(got[1].window, "build server");
        assert_eq!(got[1].window_panes, Some(2));
        assert_eq!(got[1].hook_state, "");
        assert_eq!(got[1].title, "tabby\ttitle", "title keeps its own tabs");
    }

    #[test]
    fn parse_panes_degrades_without_the_tail_fields() {
        // An old remote tmux prints empty strings for unknown format
        // variables (still 7 fields), and a truncated line has fewer —
        // both parse, with the extras degraded instead of the pane dropped.
        let empties = "100\ts\tw\t%1\t\t\t\t";
        let got = parse_panes(empties);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].window_activity, None);
        assert_eq!(got[0].window_panes, None);
        assert_eq!(got[0].hook_state, "");
        assert_eq!(got[0].title, "");

        let truncated = "100\ts\tw\t%1";
        let got = parse_panes(truncated);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].pane_id, "%1");
        assert_eq!(got[0].window_activity, None);
        assert_eq!(got[0].title, "");
    }
}

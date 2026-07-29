use super::*;

fn pane(pid: u32, session: &str, window: &str) -> PaneInfo {
    PaneInfo {
        pid,
        session: session.to_string(),
        window: window.to_string(),
        pane_id: format!("%{pid}"),
    }
}

#[test]
fn excluded_session_agents_are_filtered_out() {
    // `collect_local` runs this exact retain after detection so an agent
    // in a hidden session never reaches the sidebar footer.
    use crate::exclude;
    let panes = [pane(100, "work", "1"), pane(200, "_hidden", "1")];
    let ps = "100 1 -zsh\n150 100 claude\n200 1 -zsh\n250 200 claude";
    let mut agents = detect_agents(&panes, ps);
    assert_eq!(agents.len(), 2, "both detected before filtering");

    let compiled = exclude::compile_patterns(&["_*".to_string()]);
    agents.retain(|a| !exclude::session_excluded(&a.session, &compiled));

    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].session, "work", "the '_hidden' agent is excluded");
}

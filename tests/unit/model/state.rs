use super::*;

#[test]
fn generic_session_and_effect_dtos_do_not_regain_host_sentinels() {
    let state_source = include_str!("../../../src/model/state/mod.rs");
    let effects_source = include_str!("../../../src/model/effects.rs");
    let system_source = include_str!("../../../src/system/mod.rs");
    let actions_source = include_str!("../../../src/app/action/mod.rs");
    let forwards_source = include_str!("../../../src/infra/ssh/model/forwards.rs");
    assert!(!state_source.contains("pub host: Option<String>"));
    assert!(!system_source.contains("runtime_key"));
    assert!(!forwards_source.contains("pub host: String"));
    assert!(!actions_source.contains("TaskResult {\n        host:"));
    for removed in [
        "ShowRemotePlaceholder",
        "RemoveRemoteHost",
        "OpenRemoteNewSessionPicker",
        "SaveRemoteSessionOrder",
        "AddRemoteHost",
    ] {
        assert!(
            !effects_source.contains(removed),
            "legacy effect: {removed}"
        );
    }

    for (name, source) in [
        ("state", state_source),
        ("layout", include_str!("../../../src/model/state/layout.rs")),
        (
            "keyboard",
            include_str!("../../../src/app/action/keyboard.rs"),
        ),
        (
            "menu reducer",
            include_str!("../../../src/app/action/reduce/menu.rs"),
        ),
        (
            "port-forward reducer",
            include_str!("../../../src/app/action/reduce/port_forward.rs"),
        ),
        ("tabs UI", include_str!("../../../src/ui/sidebar/tabs.rs")),
    ] {
        assert!(
            !source.contains(".lane()"),
            "lane payload decoded in {name}"
        );
    }
}

fn make_session(name: &str) -> SessionEntry {
    SessionEntry {
        lane: crate::system::tmux::TmuxSystem::local_lane(),
        name: name.to_string(),
        dir: format!("/tmp/{name}"),
        kind: SessionEntryKind::Live { is_current: false },
    }
}

#[test]
fn unknown_lane_titles_are_neutral_and_distinct() {
    let state = AppState::new(80, 24);
    let first = LaneId::new("fixture", "first");
    let second = LaneId::new("fixture", "second");

    let first_title = state.section_title(&first);
    let second_title = state.section_title(&second);
    assert!(first_title.starts_with("unknown lane ("), "{first_title}");
    assert!(second_title.starts_with("unknown lane ("), "{second_title}");
    assert_ne!(first_title, second_title);
}

fn make_state(
    layout_mode: LayoutMode,
    show_borders: bool,
    term_width: u16,
    term_height: u16,
) -> AppState {
    use crate::system::System;

    let mut state = AppState::new(term_width, term_height);
    state.prefs.layout_mode = layout_mode;
    state.prefs.show_borders = show_borders;
    state.entries = vec![make_session("alpha"), make_session("beta")];
    let system = crate::system::tmux::TmuxSystem::default();
    system.configure(&crate::config::Config::default());
    state.system_sections = system
        .lanes()
        .into_iter()
        .filter_map(|lane| system.section_for(&lane))
        .collect();
    state.session_order = state.entries.iter().map(|s| s.name.clone()).collect();
    state.clamp_projects_focus();
    state
}

fn kind_for(unreachable: bool, loading: bool) -> SessionEntryKind {
    if unreachable {
        SessionEntryKind::Unreachable
    } else if loading {
        SessionEntryKind::Connecting
    } else {
        SessionEntryKind::Live { is_current: false }
    }
}

fn remote_row(host: &str, unreachable: bool, loading: bool) -> SessionEntry {
    SessionEntry {
        lane: crate::system::tmux::TmuxSystem::host_lane(host),
        name: "s".to_string(),
        dir: "/tmp".to_string(),
        kind: kind_for(unreachable, loading),
    }
}

/// Set the remote rows on a freshly-built state, keeping the local block
/// (built by `make_state`) at the front so `entries` stays in flat order.
fn set_remote(state: &mut AppState, rows: Vec<SessionEntry>) {
    let primary = crate::system::tmux::TmuxSystem::local_lane();
    state.entries.retain(|entry| entry.lane == primary);
    state.entries.extend(rows);
}

#[test]
fn mark_lane_reconnecting_only_updates_the_target_lane() {
    let cases = [
        ("target", "h1", SessionEntryKind::Connecting),
        ("other lane", "h2", SessionEntryKind::Unreachable),
    ];
    for (name, row_host, expected) in cases {
        let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
        set_remote(&mut state, vec![remote_row(row_host, true, false)]);
        state.mark_lane_reconnecting(&crate::system::tmux::TmuxSystem::host_lane("h1"));
        let row = state
            .entries
            .iter()
            .find(|entry| entry.lane != crate::system::tmux::TmuxSystem::local_lane())
            .unwrap();
        assert_eq!(row.kind, expected, "{name}");
    }
}

#[test]
fn agent_entries_ordered_local_then_hosts() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    set_remote(&mut state, vec![remote_row("h1", false, false)]);
    state.clamp_projects_focus();
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![detected("local", "%1")],
    );
    state.agents.insert(
        crate::system::tmux::lane(Some("h1")),
        vec![detected("h1s", "%2")],
    );
    state.rebuild_agent_entries();

    // Agent rows appear in section order: local (`None`) then each host.
    let hosts: Vec<Option<&str>> = state
        .agent_entries
        .iter()
        .map(|entry| crate::system::tmux::TmuxSystem::host_of(&entry.lane))
        .collect();
    assert_eq!(hosts, vec![None, Some("h1")]);
}

#[test]
fn agent_cursor_tracks_its_agent_when_the_list_changes() {
    // The Agents-tab cursor is a positional index. A refresh that
    // drops the agent *above* the cursor must keep the cursor on the SAME
    // agent, or the left highlight slides onto a different agent than the
    // pane shown on the right (active_agent).
    let mut state = make_state(LayoutMode::Horizontal, false, 100, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![
            detected("a", "%1"),
            detected("b", "%2"),
            detected("c", "%3"),
        ],
    );
    state.rebuild_agent_entries();
    state.agent_focused = 1; // cursor on agent "b" (%2)

    // Refresh drops "a"; "b" shifts from index 1 to 0.
    let key = state.focused_agent_key();
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![detected("b", "%2"), detected("c", "%3")],
    );
    state.rebuild_agent_entries();
    state.reanchor_agent_focus(key);

    assert_eq!(state.agent_focused, 0, "cursor follows b to its new index");
    assert_eq!(state.focused_agent().unwrap().pane_id, "%2");
}

#[test]
fn agent_cursor_clamps_when_focused_agent_disappears() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![
            detected("a", "%1"),
            detected("b", "%2"),
            detected("c", "%3"),
        ],
    );
    state.rebuild_agent_entries();
    state.agent_focused = 2; // cursor on "c"

    let key = state.focused_agent_key();
    // "c" is gone; only "a" remains.
    state
        .agents
        .insert(crate::system::tmux::lane(None), vec![detected("a", "%1")]);
    state.rebuild_agent_entries();
    state.reanchor_agent_focus(key);

    assert_eq!(
        state.agent_focused, 0,
        "clamps into range when the agent is gone"
    );
}

#[test]
fn steer_marker_follows_the_active_pane() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![detected("a", "%1"), detected("b", "%2")],
    );
    state.rebuild_agent_entries();
    state.agent_focused = 0; // cursor starts on "a"

    // Active pane holds agent "b" → marker lands on it, and the section-list
    // cursor follows the marker onto "b" (index 1).
    let local_lane = crate::system::tmux::TmuxSystem::local_lane();
    state.steer_marker_to_pane(&local_lane, "%2");
    assert_eq!(
        state.active_agent,
        Some(AgentTarget {
            lane: local_lane.clone(),
            session: "b".to_string(),
            pane_id: "%2".to_string(),
        })
    );
    assert_eq!(state.agent_focused, 1, "cursor follows the marker onto b");

    // Switch to a pane with no agent → marker clears, cursor stays put.
    state.steer_marker_to_pane(&local_lane, "%9");
    assert_eq!(state.active_agent, None);
    assert_eq!(
        state.agent_focused, 1,
        "cursor stays when no agent is there"
    );
}

#[test]
fn steer_marker_leaves_marker_when_host_unprobed() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.active_agent = Some(AgentTarget {
        lane: crate::system::tmux::TmuxSystem::local_lane(),
        session: "a".to_string(),
        pane_id: "%1".to_string(),
    });
    // A remote host whose agents were never probed (absent from the map):
    // absence means "not known", so a probe there must not blank the marker.
    state.steer_marker_to_pane(&crate::system::tmux::TmuxSystem::host_lane("box"), "%5");
    assert_eq!(
        state.active_agent,
        Some(AgentTarget {
            lane: crate::system::tmux::TmuxSystem::local_lane(),
            session: "a".to_string(),
            pane_id: "%1".to_string(),
        }),
        "an unprobed host leaves the existing marker untouched"
    );
}

#[test]
fn agents_layout_groups_agents_under_host_dividers() {
    let mut state = make_state(LayoutMode::Horizontal, false, 100, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    set_remote(&mut state, vec![remote_row("h1", false, false)]);
    state.clamp_projects_focus();
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![detected("local", "%1")],
    );
    state.agents.insert(
        crate::system::tmux::lane(Some("h1")),
        vec![detected("h1s", "%2")],
    );
    state.rebuild_agent_entries();

    let built = state.agents_layout(ViewMode::Expanded);
    // Two focusable agent rows (local agent, then h1's), in agent_entries order.
    assert_eq!(built.layout.row_count(), 2);
    // Focusable count on the Agents tab is the agent count, not session count.
    assert_eq!(state.focusable_count(), 2);
}

#[test]
fn current_layout_compacts_agents_without_group_headers() {
    let mut state = make_state(LayoutMode::Horizontal, false, 100, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    state.prefs.view_mode = ViewMode::Compact;
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![detected("agent", "%1")],
    );
    state.rebuild_agent_entries();

    let built = state.current_layout(state.prefs.view_mode);
    assert!(
        built.sections.is_empty(),
        "compact Agents view carries no group dividers",
    );
    assert!(
        built
            .layout
            .items()
            .iter()
            .all(|item| item.kind == ratatui_sectioned_list::ItemKind::Row),
        "compact Agents view is rows only",
    );
    assert!(
        built
            .layout
            .items()
            .iter()
            .any(|item| item.data.title.contains("local:agent:1")),
        "compact agent row retains its origin without a divider",
    );
}

#[test]
fn agents_sections_fold_via_their_own_collapse_set() {
    let mut state = make_state(LayoutMode::Horizontal, false, 100, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    set_remote(&mut state, vec![remote_row("h1", false, false)]);
    state.clamp_projects_focus();
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![detected("local", "%1")],
    );
    state.agents.insert(
        crate::system::tmux::lane(Some("h1")),
        vec![detected("h1s", "%2")],
    );
    state.rebuild_agent_entries();
    // Both sections expanded: neither agent row is hidden from focus.
    assert!(!state.is_focus_collapsed(0));
    assert!(!state.is_focus_collapsed(1));
    let expanded_height = state
        .agents_layout(ViewMode::Expanded)
        .layout
        .total_height();

    // Folding the local section on the Agents tab hides its agent row (focus
    // skips it, layout shrinks), and touches only the Agents collapse set —
    // Projects folds independently.
    state
        .collapsed_agent_sections
        .insert(crate::system::tmux::lane(None));
    assert!(state.is_focus_collapsed(0), "local agent row now hidden");
    assert!(!state.is_focus_collapsed(1), "h1 agent row still visible");
    assert!(
        state
            .agents_layout(ViewMode::Expanded)
            .layout
            .total_height()
            < expanded_height,
        "collapsing a section shrinks the rendered layout",
    );
    assert!(
        state.collapsed_sections.is_empty(),
        "Projects collapse set is untouched by Agents folding",
    );
}

#[test]
fn projects_cursor_reanchors_to_same_session_across_rebuild() {
    // The Projects twin of the agent-cursor reanchor: a refresh that reorders
    // the rows must keep the cursor on the SAME session, not a neighbor.
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.focused = 1; // cursor on "beta"
    let key = state.focused_session_key();
    assert_eq!(
        key,
        Some(crate::model::session::SessionId::new(
            crate::system::tmux::TmuxSystem::local_lane(),
            "beta",
        ))
    );

    // Refresh prepends a session, shifting "beta" from index 1 to 2.
    state.entries = vec![
        make_session("zzz"),
        make_session("alpha"),
        make_session("beta"),
    ];
    state.reanchor_projects_focus(key);
    assert_eq!(state.focused, 2, "cursor follows beta to its new index");

    // When the focused session is gone, fall back to clamping in range.
    let gone = state.focused_session_key();
    state.entries = vec![make_session("alpha")];
    state.reanchor_projects_focus(gone);
    assert_eq!(state.focused, 0, "clamps when the session disappears");
}

#[test]
fn disappeared_session_reanchors_within_the_same_host() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    let remote = crate::system::tmux::TmuxSystem::host_lane("xtras3");
    state.entries = vec![
        make_session("local"),
        SessionEntry {
            lane: remote.clone(),
            name: "closed".to_string(),
            dir: "/tmp/closed".to_string(),
            kind: SessionEntryKind::Live { is_current: false },
        },
        SessionEntry {
            lane: remote.clone(),
            name: "remaining".to_string(),
            dir: "/tmp/remaining".to_string(),
            kind: SessionEntryKind::Live { is_current: false },
        },
    ];
    state.focused = 1;
    let closed = state.focused_session_key();

    state.entries.remove(1);
    state.reanchor_projects_focus(closed);

    assert_eq!(state.focused, 1);
    assert_eq!(state.entries[state.focused].lane, remote);
    assert_eq!(state.entries[state.focused].name, "remaining");
}

#[test]
fn active_session_probe_steers_to_exact_lane_qualified_row() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    let remote = crate::system::tmux::TmuxSystem::host_lane("xtras3");
    state.entries.push(SessionEntry {
        lane: remote.clone(),
        name: "alpha".to_string(),
        dir: "/remote/alpha".to_string(),
        kind: SessionEntryKind::Live { is_current: false },
    });
    state.focused = 0; // local session with the same name

    state.steer_session_to(&remote, "alpha");

    assert_eq!(state.focused, 2);
    assert_eq!(state.entries[state.focused].lane, remote);
}

#[test]
fn summary_card_height_is_fixed_across_states_when_agents_exist() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![detected("agent", "%1")],
    );
    state.rebuild_agent_entries();
    let idle = state.summary_card_height();
    state.summary.state = SummaryState::Generating;
    assert_eq!(state.summary_card_height(), idle);
    state.summary.state = SummaryState::Ready {
        text: "a much longer body".repeat(20),
        generated_at: 0,
    };
    assert_eq!(
        state.summary_card_height(),
        idle,
        "the card is a fixed-size window; long text scrolls, not grows"
    );
}

#[test]
fn idle_summary_card_collapses_until_an_agent_exists() {
    let mut state = make_state(LayoutMode::Horizontal, false, 100, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    state.rebuild_agent_entries();

    assert_eq!(state.summary_card_height(), 3);

    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![detected("agent", "%1")],
    );
    state.rebuild_agent_entries();
    assert_eq!(state.summary_card_height(), 3 + state.prefs.summary_height);
}

#[test]
fn scroll_summary_clamps_to_max() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.hit_regions.summary.max_scroll = 3;
    state.scroll_summary(-5);
    assert_eq!(state.summary.scroll, 0, "can't scroll above the top");
    state.scroll_summary(10);
    assert_eq!(state.summary.scroll, 3, "clamped to max offset");
}

#[test]
fn agents_layout_shows_placeholder_for_empty_section() {
    let mut state = make_state(LayoutMode::Horizontal, false, 100, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    // Local probed but empty -> a "no agents" placeholder. Like a Projects
    // `NoSessions` row it's a focusable row (synthetic `AgentEntry`), so the
    // cursor can land on it; activating it is a guarded no-op.
    state.agents.insert(crate::system::tmux::lane(None), vec![]);
    state.rebuild_agent_entries();
    let built = state.agents_layout(ViewMode::Expanded);
    assert!(
        built
            .layout
            .items()
            .iter()
            .any(|i| i.data.title.trim() == "no agents"),
        "empty probed section shows a no-agents placeholder",
    );
    // The placeholder is a focusable row now: one row, one focus slot, but no
    // switch target (focused_agent guards it to None).
    assert_eq!(built.layout.row_count(), 1);
    assert_eq!(state.focusable_count(), 1);
    state.agent_focused = 0;
    assert!(
        state.focused_agent().is_none(),
        "cursor can sit on the placeholder, but it isn't switchable",
    );
}

#[test]
fn unknown_agent_uses_a_non_color_status_glyph() {
    let mut state = make_state(LayoutMode::Horizontal, false, 100, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![detected("agent", "%1")],
    );
    state.rebuild_agent_entries();

    let built = state.agents_layout(ViewMode::Expanded);
    assert!(built
        .layout
        .items()
        .iter()
        .any(|item| item.data.title.trim_start().starts_with("? ")));
}

fn detected(session: &str, pane_id: &str) -> crate::agent::DetectedAgent {
    crate::agent::DetectedAgent {
        kind: crate::agent::AgentKind::Claude,
        session: session.to_string(),
        window: "1".to_string(),
        pane_id: pane_id.to_string(),
        status: crate::agent::AgentStatus::Unknown,
    }
}

#[test]
fn apply_lane_agents_drops_stale_on_failed_probe() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    // Prior round: local + both hosts had detected agents.
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![detected("local", "%1")],
    );
    state.agents.insert(
        crate::system::tmux::lane(Some("h1")),
        vec![detected("h1old", "%10")],
    );
    state.agents.insert(
        crate::system::tmux::lane(Some("h2")),
        vec![detected("h2old", "%20")],
    );

    // This round queried both hosts; only h1's probe succeeded.
    let covered: std::collections::HashSet<_> = [
        crate::system::tmux::lane(Some("h1")),
        crate::system::tmux::lane(Some("h2")),
    ]
    .into_iter()
    .collect();
    let mut fresh = std::collections::HashMap::new();
    fresh.insert(
        crate::system::tmux::lane(Some("h1")),
        vec![detected("h1new", "%11")],
    );
    let mounted = [
        crate::system::tmux::lane(None),
        crate::system::tmux::lane(Some("h1")),
        crate::system::tmux::lane(Some("h2")),
    ]
    .into_iter()
    .collect();
    state.apply_lane_agents(covered, fresh, &mounted);

    // h1 updated, h2 (failed probe) cleared, local untouched.
    assert_eq!(
        state.agents[crate::system::tmux::lane(Some("h1")).as_str()][0].pane_id,
        "%11"
    );
    assert!(
        !state
            .agents
            .contains_key(crate::system::tmux::lane(Some("h2")).as_str()),
        "stale agents on a failed-probe host must be dropped"
    );
    assert!(
        state
            .agents
            .contains_key(crate::system::tmux::lane(None).as_str()),
        "local entry untouched"
    );
}

#[test]
fn apply_lane_agents_prunes_unmounted_lanes() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    // No remotes configured; a leftover host entry should be pruned.
    state.agents.insert(
        crate::system::tmux::lane(Some("ghost")),
        vec![detected("s", "%1")],
    );
    let mounted = [crate::system::tmux::lane(None)].into_iter().collect();
    state.apply_lane_agents(Default::default(), Default::default(), &mounted);
    assert!(!state
        .agents
        .contains_key(crate::system::tmux::lane(Some("ghost")).as_str()));
}

#[test]
fn sidebar_layout_adds_local_header_in_expanded() {
    let state = make_state(LayoutMode::Horizontal, false, 80, 24);
    let built = state.sidebar_layout(ViewMode::Expanded);

    let local_headers = built
        .layout
        .items()
        .iter()
        .filter(|i| i.data.title == "local")
        .count();
    assert_eq!(local_headers, 1, "one local divider above the local rows");

    // The header is not a row, so the two local sessions are rows 0..2.
    assert_eq!(built.layout.row_count(), 2);
    // The local section carries one direct new-session button.
    assert_eq!(
        built
            .sections
            .first()
            .map(|s| crate::system::tmux::TmuxSystem::host_of(&s.lane)),
        Some(None),
        "first section is local",
    );
}

#[test]
fn sidebar_layout_omits_local_header_in_compact() {
    let state = make_state(LayoutMode::Horizontal, false, 80, 24);
    let compact = state.sidebar_layout(ViewMode::Compact);
    assert!(
        compact.sections.is_empty(),
        "compact view carries no group dividers",
    );
    assert!(
        compact
            .layout
            .items()
            .iter()
            .all(|i| i.kind == ratatui_sectioned_list::ItemKind::Row),
        "compact view is rows only, no headers",
    );
}

#[test]
fn sidebar_layout_keeps_local_divider_when_empty() {
    let mut empty = make_state(LayoutMode::Horizontal, false, 80, 24);
    empty
        .entries
        .retain(|entry| entry.lane != crate::system::tmux::TmuxSystem::local_lane());
    empty.clamp_projects_focus();
    let built = empty.sidebar_layout(ViewMode::Expanded);
    assert!(
        built.layout.items().iter().any(|i| i.data.title == "local"),
        "local divider remains when there are no local sessions",
    );
    assert_eq!(built.layout.row_count(), 0, "no local session rows");
}

#[test]
fn is_divider_at_row_detects_header_not_session() {
    let state = make_state(LayoutMode::Horizontal, false, 100, 24);
    // Header banner is 2 rows (no border); the Summary card strip is pinned
    // to the bottom on both tabs, so the list begins right after the header.
    // The local divider is the first list item (1 row tall); the first
    // session card sits just below it.
    let top = 2;
    assert!(
        state.is_divider_at_row(top),
        "first list row is the local divider"
    );
    assert!(
        !state.is_divider_at_row(top + 1),
        "next row is a session card"
    );
    assert_eq!(state.focus_at_row(top + 1), Some(FocusTarget(0)));
    // Rows in the header banner above the session area aren't dividers.
    assert!(!state.is_divider_at_row(0));
}

#[test]
fn sidebar_footer_height_matches_renderer() {
    // The renderer (ui::sidebar::draw_sidebar) lays the footer out as
    // `2 + banner`; the hit-tester must agree or the bottom visible session
    // row goes click-dead. This locks the two fixed rows (the separator +
    // the menu/version line).
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    assert_eq!(
        state.sidebar_footer_height(),
        2,
        "no banner: just the separator + menu rows"
    );
    // The update banner adds exactly one row, matching the renderer.
    state.update_available = Some(UpdateStatus {
        latest_version: "9.9.9".to_string(),
        current_version: "0.0.0".to_string(),
        checked_at: 0,
    });
    assert_eq!(state.sidebar_footer_height(), 3);
}

#[test]
fn local_divider_menu_greys_remote_only_items() {
    use crate::menu::{ContextMenu, MenuItem, MenuKind};
    let menu = ContextMenu {
        kind: MenuKind::LaneDivider {
            lane: crate::system::tmux::TmuxSystem::local_lane(),
            primary: true,
        },
        x: 0,
        y: 0,
        selected: 1,
    };
    // Same item list as a host divider...
    assert!(menu.items().contains(&MenuItem::NewSession));
    assert!(menu.items().contains(&MenuItem::PortForward));
    assert!(menu.items().contains(&MenuItem::RemoveFromList));
    // ...but the remote-only ones are greyed out, leaving New session live.
    assert!(menu.disabled().contains(&MenuItem::PortForward));
    assert!(menu.disabled().contains(&MenuItem::RemoveFromList));
    assert!(!menu.disabled().contains(&MenuItem::NewSession));
    // The initial highlight lands on the first enabled item.
    assert_eq!(menu.items()[menu.first_enabled()], MenuItem::NewSession);
}

#[test]
fn remote_divider_shows_forward_count() {
    use crate::config::RemoteConfig;
    use crate::forwards::{ForwardMode, ForwardSpec};
    use crate::ssh::divider::cmd;
    use crate::system::tmux::TmuxSystem;
    use crate::system::System;

    // The `⇄N` forward indicator is the leftmost divider button the tmux
    // System supplies; N counts the host's configured forwards. deck no longer
    // probes per-forward liveness, so the count is the only forward feedback.
    let forward_glyph = |state: &AppState, host: &str| -> Option<String> {
        let system = TmuxSystem::default();
        let mut config = crate::config::Config::default();
        config.remotes.clone_from(&state.config_remotes);
        system.configure(&config);
        system
            .section_for(&TmuxSystem::host_lane(host))?
            .buttons
            .into_iter()
            .find(|b| b.action.as_str() == cmd::FORWARDS)
            .map(|b| b.glyph)
    };

    let spec = |port: u16| ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: port,
        target_host: Some("127.0.0.1".into()),
        target_port: Some(port),
    };

    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.config_remotes = vec![
        RemoteConfig {
            host: "h1".into(),
            forwards: vec![spec(8001), spec(8002)],
        },
        RemoteConfig {
            host: "nofwd".into(),
            forwards: vec![],
        },
    ];

    // The label counts every configured forward.
    assert_eq!(forward_glyph(&state, "h1").as_deref(), Some("⇄2"));
    // A host with no forwards has no forward button.
    assert_eq!(forward_glyph(&state, "nofwd"), None);
    // An unknown host (not in config) likewise.
    assert_eq!(forward_glyph(&state, "ghost"), None);
}

#[test]
fn narrow_terminal_forces_vertical_layout() {
    // At or below NARROW_LAYOUT_MAX_WIDTH the Horizontal pref is overridden
    // to Vertical for rendering/sizing, but the stored pref is preserved.
    let narrow = make_state(LayoutMode::Horizontal, false, NARROW_LAYOUT_MAX_WIDTH, 24);
    assert_eq!(narrow.effective_layout_mode(), LayoutMode::Vertical);
    assert_eq!(narrow.prefs.layout_mode, LayoutMode::Horizontal);

    // One column wider, the Horizontal pref takes effect again.
    let wide = make_state(
        LayoutMode::Horizontal,
        false,
        NARROW_LAYOUT_MAX_WIDTH + 1,
        24,
    );
    assert_eq!(wide.effective_layout_mode(), LayoutMode::Horizontal);

    // A Vertical pref is honored at any width.
    let wide_vertical = make_state(LayoutMode::Vertical, false, 120, 24);
    assert_eq!(wide_vertical.effective_layout_mode(), LayoutMode::Vertical);
}

#[test]
fn resize_sidebar_handles_small_terminals() {
    let mut state = make_state(LayoutMode::Horizontal, true, 20, 40);
    assert!(state.resize_sidebar(30));
    assert_eq!(state.prefs.sidebar_width, 10);
}

#[test]
fn collapsed_sidebar_restores_space_without_losing_expanded_width() {
    let mut state = make_state(LayoutMode::Horizontal, true, 120, 40);
    state.prefs.sidebar_width = 32;
    let expanded_pty = state.pty_size();

    state.prefs.sidebar_collapsed = true;
    assert_eq!(state.effective_sidebar_width(), SIDEBAR_COLLAPSED_WIDTH);
    assert_eq!(state.prefs.sidebar_width, 32, "saved width remains intact");
    assert!(state.pty_size().1 > expanded_pty.1);

    state.prefs.sidebar_collapsed = false;
    assert_eq!(state.effective_sidebar_width(), 32);
    assert_eq!(state.pty_size(), expanded_pty);
}

#[test]
fn vertical_sidebar_is_fixed_single_tab_row() {
    // Bordered: one tab row + top/bottom border = 3 rows, and the
    // height is pinned there regardless of any stored sidebar_height.
    let mut state = make_state(LayoutMode::Vertical, true, 120, 40);
    assert_eq!(state.effective_sidebar_height(), 3);

    state.resize_sidebar_height(6);
    assert_eq!(state.effective_sidebar_height(), 3);
    assert_eq!(state.pty_size(), (35, 118));

    // Borderless: just the single tab row.
    let borderless = make_state(LayoutMode::Vertical, false, 120, 40);
    assert_eq!(borderless.effective_sidebar_height(), 1);
}

#[test]
fn vertical_tab_hit_testing_only_uses_tab_row() {
    let state = make_state(LayoutMode::Vertical, true, 120, 40);

    assert_eq!(state.session_at_col(2, 1), Some(0));
    assert_eq!(state.session_at_col(2, 2), None);
}

#[test]
fn vertical_tabs_hit_test_remote_sessions() {
    // Two local tabs ("alpha", "beta") then one remote ("h1:s"). A click
    // landing in the remote tab's column resolves to its flat focus
    // index (local_count + remote_idx == 2), and that index decodes back
    // to the remote row for switch/menu dispatch.
    let mut state = make_state(LayoutMode::Vertical, true, 120, 40);
    set_remote(&mut state, vec![remote_row("h1", false, false)]);

    // Tab columns (bordered, leading pad 1): alpha (1..9), beta (10..17),
    // h1:s (18..25). Probe a column inside the remote tab, offset by the
    // left border.
    let hit = state.session_at_col(1 + 20, 1);
    assert_eq!(hit, Some(2));

    let entry = state.entry_at(FocusTarget(hit.unwrap())).unwrap();
    assert_eq!(
        crate::system::tmux::TmuxSystem::host_of(&entry.lane),
        Some("h1")
    );
    assert_eq!(entry.name, "s");
}

#[test]
fn context_menu_navigation_skips_disabled_items() {
    use crate::menu::{ContextMenu, MenuItem, MenuKind};
    // A placeholder remote menu: every session item disabled.
    let all_disabled = ContextMenu {
        kind: MenuKind::Session {
            focus: FocusTarget(0),
            disabled: &[MenuItem::Rename, MenuItem::Close],
        },
        x: 0,
        y: 0,
        selected: 0,
    };
    assert!(!all_disabled.is_enabled(0));
    assert!(!all_disabled.is_enabled(1));
    // Nothing selectable: first/next/prev stay put.
    assert_eq!(all_disabled.first_enabled(), 0);
    assert_eq!(all_disabled.next_enabled(), 0);

    // One disabled item among enabled ones: navigation stays on Close.
    // Items are the fixed session list: Rename, Close.
    let mixed = ContextMenu {
        kind: MenuKind::Session {
            focus: FocusTarget(0),
            disabled: &[MenuItem::Rename],
        },
        x: 0,
        y: 0,
        selected: 1,
    };
    assert_eq!(mixed.first_enabled(), 1);
    assert_eq!(mixed.next_enabled(), 1);
    assert_eq!(mixed.prev_enabled(), 1);

    assert_eq!(mixed.items().len(), 2);
    assert_eq!(mixed.items()[0].label(), "Rename");
    assert_eq!(mixed.items()[1].label(), "Close");
}

// --- PfAddForm::validate() tests ---

use crate::forwards::ForwardMode;
use crate::forwards::{PfAddForm, PfField, PfFormError};
use ratatui_textarea::TextArea;

fn ta(text: &str) -> TextArea<'static> {
    TextArea::new(vec![text.to_string()])
}

fn blank_form() -> PfAddForm {
    PfAddForm {
        mode: ForwardMode::Local,
        focus: PfField::ListenPort,
        bind_addr: ta(""),
        listen_port: ta(""),
        target_host: ta(""),
        target_port: ta(""),
        submitting: false,
    }
}

#[test]
fn validate_local_ok() {
    let mut f = blank_form();
    f.listen_port = ta("8080");
    f.target_host = ta("localhost");
    f.target_port = ta("80");
    let spec = f.validate().expect("should validate");
    assert_eq!(spec.listen_port, 8080);
    assert_eq!(spec.target_host.as_deref(), Some("localhost"));
    assert_eq!(spec.target_port, Some(80));
    assert_eq!(spec.bind_addr, None);
}

#[test]
fn validate_local_missing_target_host() {
    let mut f = blank_form();
    f.listen_port = ta("8080");
    f.target_port = ta("80");
    assert_eq!(f.validate(), Err(PfFormError::TargetHostRequired));
}

#[test]
fn validate_accepts_port_zero() {
    // SSH treats port 0 as "kernel picks an ephemeral port"; the user
    // asked for 0-65535 to be valid.
    let mut f = blank_form();
    f.listen_port = ta("0");
    f.target_host = ta("h");
    f.target_port = ta("80");
    let spec = f.validate().expect("port 0 should be valid");
    assert_eq!(spec.listen_port, 0);
}

#[test]
fn validate_local_port_non_numeric_rejected() {
    let mut f = blank_form();
    f.listen_port = ta("abc");
    f.target_host = ta("h");
    f.target_port = ta("80");
    assert_eq!(f.validate(), Err(PfFormError::ListenPortRange));
}

#[test]
fn validate_dynamic_clears_target() {
    let mut f = blank_form();
    f.mode = ForwardMode::Dynamic;
    f.listen_port = ta("1080");
    f.target_host = ta("stale");
    f.target_port = ta("999");
    let spec = f.validate().unwrap();
    assert_eq!(spec.target_host, None);
    assert_eq!(spec.target_port, None);
}

#[test]
fn validate_bind_addr_passthrough() {
    let mut f = blank_form();
    f.bind_addr = ta("127.0.0.1");
    f.listen_port = ta("8080");
    f.target_host = ta("h");
    f.target_port = ta("80");
    let spec = f.validate().unwrap();
    assert_eq!(spec.bind_addr.as_deref(), Some("127.0.0.1"));
}

#[test]
fn same_listen_identity_keys_on_mode_bind_and_listen() {
    use crate::forwards::{ForwardMode, ForwardSpec};
    let base = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: Some("127.0.0.1".into()),
        listen_port: 8080,
        target_host: Some("h".into()),
        target_port: Some(80),
    };
    // Same mode + bind + listen port collide even if the target differs — ssh
    // can't bind the same listener twice.
    let same_listener = ForwardSpec {
        target_host: Some("other".into()),
        target_port: Some(443),
        ..base.clone()
    };
    assert!(base.same_listen_identity(&same_listener));
    // A different listen port is a different listener.
    assert!(!base.same_listen_identity(&ForwardSpec {
        listen_port: 9090,
        ..base.clone()
    }));
    // A different mode (an -R sharing the port number) doesn't collide.
    assert!(!base.same_listen_identity(&ForwardSpec {
        mode: ForwardMode::Remote,
        ..base.clone()
    }));
    // A different bind address is a different listener.
    assert!(!base.same_listen_identity(&ForwardSpec {
        bind_addr: Some("0.0.0.0".into()),
        ..base.clone()
    }));
}

#[test]
fn confirm_kill_name_resolves_remote_focused_row() {
    // When killing a remote session, the confirm-kill overlay name must come
    // from whichever store the focused row lives in — not only the local
    // store, or it resolves to None and the dialog never draws.
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    set_remote(&mut state, vec![remote_row("h1", false, false)]); // named "s"
                                                                  // Flat index 2 == local_count(2) + remote_idx(0): the remote row.
    state.focused = 2;
    state.overlay.confirm_kill = true;
    assert_eq!(state.confirm_kill_name().as_deref(), Some("s"));
}

#[test]
fn confirm_kill_name_resolves_local_focused_row() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.focused = 1; // local "beta"
    state.overlay.confirm_kill = true;
    assert_eq!(state.confirm_kill_name().as_deref(), Some("beta"));
}

#[test]
fn confirm_kill_name_none_when_not_pending() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    set_remote(&mut state, vec![remote_row("h1", false, false)]);
    state.focused = 2;
    // No pending kill -> no name regardless of what's focused.
    assert_eq!(state.confirm_kill_name(), None);
}

#[test]
fn collapsed_local_group_hides_rows() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state
        .collapsed_sections
        .insert(crate::system::tmux::lane(None));
    let built = state.sidebar_layout(ViewMode::Expanded);

    assert!(built.layout.is_collapsible());
    // The local header is collapsed; its rows (idx 0,1) are hidden.
    assert!(built.layout.is_row_hidden(0));
    assert!(built.layout.is_row_hidden(1));
}

#[test]
fn focus_skips_collapsed_remote_group() {
    // Layout: 2 local rows (idx 0,1), then host h1 (idx 2). Collapse local.
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    set_remote(&mut state, vec![remote_row("h1", false, false)]);
    state.clamp_projects_focus();
    state
        .collapsed_sections
        .insert(crate::system::tmux::lane(None));

    // Local rows are hidden, the remote row is not.
    assert!(state.is_focus_collapsed(0));
    assert!(state.is_focus_collapsed(1));
    assert!(!state.is_focus_collapsed(2));
}

#[test]
fn agents_probe_interval_cycles_and_labels() {
    assert_eq!(
        normalize_agents_probe_interval(3),
        DEFAULT_AGENTS_PROBE_INTERVAL
    );
    assert_eq!(agents_probe_interval_label(1), "1s (fast)");
    assert_eq!(agents_probe_interval_label(10), "10s (very slow)");

    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.prefs.agents_probe_interval_secs = 2;
    state.cycle_agents_probe_interval(1);
    assert_eq!(state.prefs.agents_probe_interval_secs, 5);
    state.cycle_agents_probe_interval(-1);
    assert_eq!(state.prefs.agents_probe_interval_secs, 2);
    // Wraps at the ends.
    state.prefs.agents_probe_interval_secs = 1;
    state.cycle_agents_probe_interval(-1);
    assert_eq!(state.prefs.agents_probe_interval_secs, 10);
}

#[test]
fn step_clamped_covers_movement_boundaries_and_degenerate_lists() {
    let cases = [
        ("forward", 0, 3, 1, 1),
        ("forward to last", 1, 3, 1, 2),
        ("forward at last", 2, 3, 1, 2),
        ("backward", 2, 3, -1, 1),
        ("backward to first", 1, 3, -1, 0),
        ("backward at first", 0, 3, -1, 0),
        ("empty forward", 0, 0, 1, 0),
        ("empty backward", 0, 0, -1, 0),
        ("single forward", 0, 1, 1, 0),
        ("single backward", 0, 1, -1, 0),
    ];
    for (name, current, len, direction, expected) in cases {
        assert_eq!(step_clamped(current, len, direction), expected, "{name}");
    }
}

#[test]
fn prefs_config_round_trip_is_identity() {
    // Phase-7 invariant: `from_config(to_config(p)) == p` on the prefs.
    // Start from a Config carrying already-normalized, non-default values
    // (so the load-time clamps are no-ops and the comparison is exact), map
    // it into prefs, write it back out, and re-derive — the prefs must match.
    let cfg = crate::config::Config {
        theme: crate::theme::THEMES[2].name.to_string(),
        theme_auto: true,
        dark_theme: crate::theme::THEMES[1].name.to_string(),
        light_theme: crate::theme::THEMES[3].name.to_string(),
        layout: LayoutMode::Vertical,
        show_borders: false,
        sidebar_tab: SidebarTab::Agents,
        sidebar_width: 40,
        sidebar_height: 3,
        sidebar_collapsed: true,
        view_mode: ViewMode::Compact,
        frame_rate_limit: 30,
        exclude_patterns: vec!["foo*".to_string(), "/bar/".to_string()],
        keybindings: std::collections::BTreeMap::new(),
        update_check: crate::update::UpdateCheckMode::Disabled,
        remotes: Vec::new(),
        collapsed_sections: Vec::new(),
        collapsed_agent_sections: Vec::new(),
        summary_prompt: "prompt".to_string(),
        summary_prompt_version: crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION,
        summary_agent: crate::summary_card::SummaryAgent::Codex,
        summary_model: "model".to_string(),
        summary_height: 12,
        summary_language: "English".to_string(),
        agents_probe_interval: 5,
        summary_enabled: false,
        transparent_bg: false,
    };

    let theme_index = 2;
    let prefs = Prefs::from_config(&cfg, theme_index);
    let written = prefs.to_config(
        std::collections::BTreeMap::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    // Re-derive the theme index from the written name rather than reusing the
    // input, so the round trip actually exercises the name<->index mapping
    // that lives outside Prefs (to_config writes THEMES[idx].name).
    let rederived_index = crate::theme::THEMES
        .iter()
        .position(|t| t.name == written.theme)
        .expect("written theme name must resolve to an index");
    let round_tripped = Prefs::from_config(&written, rederived_index);
    assert_eq!(prefs, round_tripped);
    assert_eq!(
        rederived_index, theme_index,
        "theme name<->index round-trips"
    );
}

#[test]
fn session_indexing_matches_direct_storage_after_filtered_removal() {
    // The flat focus index must decode straight into `sessions` (local) then
    // `remote_sessions`.
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.entries = vec![make_session("a"), make_session("b"), make_session("c")];
    state.session_order = state.entries.iter().map(|s| s.name.clone()).collect();
    set_remote(&mut state, vec![remote_row("h1", false, false)]);
    state.clamp_projects_focus();

    // Local rows occupy the front of `entries`; each flat index resolves to
    // the entry at that exact position.
    for (i, expected) in ["a", "b", "c"].iter().enumerate() {
        let entry = state.entry_at(FocusTarget(i)).unwrap();
        assert_eq!(
            entry.lane,
            crate::system::tmux::TmuxSystem::local_lane(),
            "flat index {i} should be local"
        );
        assert_eq!(&entry.name, expected);
        assert_eq!(
            state.focusable_index_for(&crate::system::tmux::TmuxSystem::local_lane(), expected),
            Some(i)
        );
    }
    // Remote rows follow the local block.
    let remote_flat = state.local_count();
    let entry = state.entry_at(FocusTarget(remote_flat)).unwrap();
    assert_eq!(
        crate::system::tmux::TmuxSystem::host_of(&entry.lane),
        Some("h1")
    );
    assert_eq!(
        state.focusable_index_for(&crate::system::tmux::TmuxSystem::host_lane("h1"), "s"),
        Some(remote_flat)
    );
    assert_eq!(state.focusable_count(), 4);
}

#[test]
fn flat_index_decodes_to_host_and_kind() {
    // The unified-store decode: a flat focus index resolves straight to the
    // entry, and host/kind are read off it (no `idx - local_count` math).
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24); // alpha, beta
    set_remote(
        &mut state,
        vec![
            remote_row("h", false, false), // Live
            remote_row("d", true, false),  // Unreachable
            SessionEntry {
                lane: crate::system::tmux::TmuxSystem::host_lane("e"),
                name: String::new(),
                dir: String::new(),
                kind: SessionEntryKind::NoSessions,
            },
        ],
    );

    // 0,1 local; 2 remote Live; 3 remote Unreachable; 4 remote NoSessions.
    let e0 = state.entry_at(FocusTarget(0)).unwrap();
    assert_eq!(e0.lane, crate::system::tmux::TmuxSystem::local_lane());
    assert!(e0.is_attachable());

    let e2 = state.entry_at(FocusTarget(2)).unwrap();
    assert_eq!(
        crate::system::tmux::TmuxSystem::host_of(&e2.lane),
        Some("h")
    );
    assert!(e2.is_attachable());

    let e3 = state.entry_at(FocusTarget(3)).unwrap();
    assert_eq!(
        crate::system::tmux::TmuxSystem::host_of(&e3.lane),
        Some("d")
    );
    assert_eq!(e3.kind, SessionEntryKind::Unreachable);
    assert!(!e3.is_attachable());

    let e4 = state.entry_at(FocusTarget(4)).unwrap();
    assert_eq!(e4.kind, SessionEntryKind::NoSessions);
    assert!(!e4.is_attachable());

    assert!(state.entry_at(FocusTarget(5)).is_none());
    // Section key reads off the entry's host directly.
    assert_eq!(
        state.section_key_of_focus(0),
        Some(crate::system::tmux::TmuxSystem::local_lane())
    );
    assert_eq!(
        state.section_key_of_focus(2),
        Some(crate::system::tmux::TmuxSystem::host_lane("h"))
    );
}

#[test]
fn kill_policy_over_entries_guards_placeholder_and_last_remote() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24); // 2 locals
    set_remote(
        &mut state,
        vec![
            SessionEntry {
                lane: crate::system::tmux::TmuxSystem::host_lane("e"),
                name: String::new(),
                dir: String::new(),
                kind: SessionEntryKind::NoSessions,
            }, // flat 2: placeholder
            remote_row("solo", false, false), // flat 3: host "solo"'s only live session
            remote_row("pair", false, false), // flat 4
            remote_row("pair", false, false), // flat 5 (sibling)
        ],
    );
    // Give the two "pair" rows distinct names so they're two real sessions.
    state.entries[4].name = "p1".into();
    state.entries[5].name = "p2".into();

    let blocked =
        |s: &AppState, i: usize| s.kill_blocked_reason(s.entry_at(FocusTarget(i)).unwrap());
    assert_eq!(blocked(&state, 2), Some("no session to kill")); // placeholder
    assert_eq!(blocked(&state, 3), Some("last session on lane")); // solo
    assert_eq!(blocked(&state, 4), None); // pair has a sibling
    assert_eq!(blocked(&state, 0), None); // two locals: killable
}

#[test]
fn no_sessions_name_is_a_normal_live_session_now() {
    // Sentinel removal: a real session literally named "(no sessions)" is a
    // normal Live entry — attachable, killable, and never treated as a
    // placeholder (the magic-name special-casing is gone).
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24); // 2 locals
    set_remote(
        &mut state,
        vec![
            SessionEntry {
                lane: crate::system::tmux::TmuxSystem::host_lane("h"),
                name: crate::state::NO_SESSIONS_LABEL.to_string(),
                dir: "/tmp".into(),
                kind: SessionEntryKind::Live { is_current: false },
            }, // flat 2
            remote_row("h", false, false), // flat 3 (sibling so it isn't "last on host")
        ],
    );
    let entry = state.entry_at(FocusTarget(2)).unwrap();
    assert_eq!(entry.name, crate::state::NO_SESSIONS_LABEL);
    assert!(entry.is_attachable(), "a real session is attachable");
    assert_eq!(
        state.kill_blocked_reason(entry),
        None,
        "a real session named '(no sessions)' is killable, not a placeholder"
    );
    assert!(state.focused_remote_placeholder().is_none() || state.focused != 2);
}

#[test]
fn project_drag_indicators_wait_for_the_hold_delay() {
    // Every press on a row starts a drag (release decides click vs reorder),
    // so the `↕`/`▸` markers must not flash on an ordinary click-to-switch.
    let mut state = make_state(LayoutMode::Horizontal, false, 100, 24);
    let t0 = Instant::now();
    assert_eq!(
        state.start_project_drag(3, t0),
        Some(0),
        "grabbed first card"
    );
    assert!(
        state.project_drag_indicators().is_none(),
        "no indicators on press"
    );
    assert!(!state.tick_project_drag(t0 + std::time::Duration::from_millis(499)));
    assert!(
        state.project_drag_indicators().is_none(),
        "still hidden just under the delay"
    );

    // Held past the delay: the markers appear, and exactly one tick reports it
    // (the caller redraws on that edge).
    assert!(state.tick_project_drag(t0 + PROJECT_DRAG_INDICATOR_DELAY));
    assert_eq!(state.project_drag_indicators(), Some((0, 0)));
    assert!(
        !state.tick_project_drag(t0 + std::time::Duration::from_secs(5)),
        "edge fires once"
    );
}

#[test]
fn crossing_to_another_row_shows_drag_indicators_immediately() {
    // The pointer leaving the pressed row proves this is a reorder, so the
    // gesture gets its feedback without waiting out the delay.
    let mut state = make_state(LayoutMode::Horizontal, false, 100, 24);
    let t0 = Instant::now();
    assert_eq!(state.start_project_drag(3, t0), Some(0));
    assert!(state.project_drag_indicators().is_none());
    assert_eq!(state.update_project_drag(5), Some(1), "second card");
    assert_eq!(state.project_drag_indicators(), Some((0, 1)));
}

#[test]
fn auto_theme_follows_the_probed_terminal_background() {
    let mut state = AppState::new(80, 24);
    state.prefs.theme_index = 0;
    state.prefs.dark_theme_index = 1;
    state.prefs.light_theme_index = 2;

    // Off: the fixed choice wins no matter what the terminal is.
    state.prefs.theme_auto = false;
    state.terminal_is_dark = false;
    assert_eq!(state.active_theme_index(), 0);

    // On: the slot matching the probed background wins.
    state.prefs.theme_auto = true;
    assert_eq!(state.active_theme_index(), 2);
    state.terminal_is_dark = true;
    assert_eq!(state.active_theme_index(), 1);

    // Picking a fixed theme is how the user leaves auto mode — otherwise the
    // pick would apply to nothing visible.
    state
        .prefs
        .set_theme_slot(crate::theme::ThemeSlot::Fixed, 3);
    assert!(!state.prefs.theme_auto);
    assert_eq!(state.active_theme_index(), 3);
}

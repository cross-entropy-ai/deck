use super::*;

fn make_session(name: &str) -> SessionEntry {
    SessionEntry {
        host: None,
        name: name.to_string(),
        dir: format!("/tmp/{name}"),
        kind: SessionEntryKind::Live { is_current: false },
    }
}

fn make_state(
    layout_mode: LayoutMode,
    show_borders: bool,
    term_width: u16,
    term_height: u16,
) -> AppState {
    let mut state = AppState::new(term_width, term_height);
    state.prefs.layout_mode = layout_mode;
    state.prefs.show_borders = show_borders;
    state.entries = vec![make_session("alpha"), make_session("beta")];
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
        host: Some(host.to_string()),
        name: "s".to_string(),
        dir: "/tmp".to_string(),
        kind: kind_for(unreachable, loading),
    }
}

/// Set the remote rows on a freshly-built state, keeping the local block
/// (built by `make_state`) at the front so `entries` stays in flat order.
fn set_remote(state: &mut AppState, rows: Vec<SessionEntry>) {
    state.entries.retain(|e| e.is_local());
    state.entries.extend(rows);
}

#[test]
fn mark_host_reconnecting_sets_loading_clears_unreachable() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    set_remote(&mut state, vec![remote_row("h1", true, false)]);
    state.mark_host_reconnecting("h1");
    let row = state.entries.iter().find(|e| !e.is_local()).unwrap();
    assert_eq!(row.kind, SessionEntryKind::Connecting);
}

#[test]
fn mark_host_reconnecting_ignores_other_hosts() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    set_remote(&mut state, vec![remote_row("h2", true, false)]);
    state.mark_host_reconnecting("h1");
    let row = state.entries.iter().find(|e| !e.is_local()).unwrap();
    assert_eq!(row.kind, SessionEntryKind::Unreachable);
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
        .map(|r| r.host.as_deref())
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
    state.steer_marker_to_pane(None, "%2");
    assert_eq!(
        state.active_agent,
        Some(AgentTarget {
            host: None,
            session: "b".to_string(),
            pane_id: "%2".to_string(),
        })
    );
    assert_eq!(state.agent_focused, 1, "cursor follows the marker onto b");

    // Switch to a pane with no agent → marker clears, cursor stays put.
    state.steer_marker_to_pane(None, "%9");
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
        host: None,
        session: "a".to_string(),
        pane_id: "%1".to_string(),
    });
    // A remote host whose agents were never probed (absent from the map):
    // absence means "not known", so a probe there must not blank the marker.
    state.steer_marker_to_pane(Some("box"), "%5");
    assert_eq!(
        state.active_agent,
        Some(AgentTarget {
            host: None,
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

    let built = state.agents_layout();
    // Two focusable agent rows (local agent, then h1's), in agent_entries order.
    assert_eq!(built.layout.row_count(), 2);
    // Focusable count on the Agents tab is the agent count, not sessions.
    assert_eq!(state.focusable_count(), 2);
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
    let expanded_height = state.agents_layout().layout.total_height();

    // Folding the local section on the Agents tab hides its agent row (focus
    // skips it, layout shrinks), and touches only the Agents collapse set —
    // Projects folds independently.
    state
        .collapsed_agent_sections
        .insert(crate::system::tmux::lane(None));
    assert!(state.is_focus_collapsed(0), "local agent row now hidden");
    assert!(!state.is_focus_collapsed(1), "h1 agent row still visible");
    assert!(
        state.agents_layout().layout.total_height() < expanded_height,
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
    assert_eq!(key, Some((None, "beta".to_string())));

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
fn summary_card_height_is_fixed_across_states() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
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
    let built = state.agents_layout();
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

fn detected(session: &str, pane_id: &str) -> crate::agent::DetectedAgent {
    crate::agent::DetectedAgent {
        kind: crate::agent::AgentKind::Claude,
        session: session.to_string(),
        window: "1".to_string(),
        pane: "0".to_string(),
        pane_id: pane_id.to_string(),
        status: crate::agent::AgentStatus::Unknown,
    }
}

#[test]
fn apply_remote_agents_drops_stale_on_failed_probe() {
    use crate::config::RemoteConfig;
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.config_remotes = vec![
        RemoteConfig {
            host: "h1".into(),
            forwards: vec![],
        },
        RemoteConfig {
            host: "h2".into(),
            forwards: vec![],
        },
    ];
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
    let covered: std::collections::HashSet<String> =
        ["h1".to_string(), "h2".to_string()].into_iter().collect();
    let mut fresh = std::collections::HashMap::new();
    fresh.insert("h1".to_string(), vec![detected("h1new", "%11")]);
    state.apply_remote_agents(covered, fresh);

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
fn apply_remote_agents_prunes_unconfigured_hosts() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    // No remotes configured; a leftover host entry should be pruned.
    state.agents.insert(
        crate::system::tmux::lane(Some("ghost")),
        vec![detected("s", "%1")],
    );
    state.apply_remote_agents(Default::default(), Default::default());
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
        .filter(|i| i.data.title == "@local")
        .count();
    assert_eq!(local_headers, 1, "one @local divider above the local rows");

    // The header is not a row, so the two local sessions are rows 0..2.
    assert_eq!(built.layout.row_count(), 2);
    // The `@local` section carries the local-divider menu button.
    assert_eq!(
        built
            .sections
            .first()
            .map(|s| crate::system::tmux::TmuxSystem::host_of(&s.lane)),
        Some(None),
        "first section is @local",
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
    empty.entries.retain(|e| !e.is_local());
    empty.clamp_projects_focus();
    let built = empty.sidebar_layout(ViewMode::Expanded);
    assert!(
        built
            .layout
            .items()
            .iter()
            .any(|i| i.data.title == "@local"),
        "@local divider remains when there are no local sessions",
    );
    assert_eq!(built.layout.row_count(), 0, "no local session rows");
}

#[test]
fn is_divider_at_row_detects_header_not_session() {
    let state = make_state(LayoutMode::Horizontal, false, 100, 24);
    // Header banner is 2 rows (no border); the Summary card strip is pinned
    // to the bottom on both tabs, so the list begins right after the header.
    // The @local divider is the first list item (1 row tall); the first
    // session card sits just below it.
    let top = 2;
    assert!(
        state.is_divider_at_row(top),
        "first list row is the @local divider"
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
        release_url: String::new(),
        checked_at: 0,
    });
    assert_eq!(state.sidebar_footer_height(), 3);
}

#[test]
fn local_divider_menu_greys_remote_only_items() {
    use crate::state::{ContextMenu, MenuItem, MenuKind};
    let menu = ContextMenu {
        kind: MenuKind::LocalDivider,
        x: 0,
        y: 0,
        selected: 0,
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
fn sync_remote_forward_health_mirrors_host_status() {
    use crate::config::RemoteConfig;
    use crate::forwards::{ForwardMode, ForwardSpec};
    use crate::state::{ForwardHealth, ForwardKey};

    let r_spec = ForwardSpec {
        mode: ForwardMode::Remote,
        bind_addr: None,
        listen_port: 9090,
        target_host: Some("127.0.0.1".into()),
        target_port: Some(9090),
    };
    let key = ForwardKey::from_spec("h1", &r_spec);

    // connected → Up, unreachable → Down, connecting → Probing.
    let cases = [
        (remote_row("h1", false, false), ForwardHealth::Up),
        (remote_row("h1", true, false), ForwardHealth::Down),
        (remote_row("h1", false, true), ForwardHealth::Probing),
    ];
    for (row, expected) in cases {
        let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
        state.config_remotes = vec![RemoteConfig {
            host: "h1".into(),
            forwards: vec![r_spec.clone()],
        }];
        set_remote(&mut state, vec![row]);
        state.sync_remote_forward_health();
        assert_eq!(state.forward_health.get(&key).copied(), Some(expected));
    }
}

#[test]
fn forward_badge_rolls_up_per_host_health() {
    use crate::config::RemoteConfig;
    use crate::forwards::{ForwardMode, ForwardSpec};
    use crate::geometry::BadgeStatus;
    use crate::state::{ForwardHealth, ForwardKey};
    use crate::system::tmux::TmuxSystem;
    use crate::system::System;

    // The badge now comes from the tmux System, styled per lane.
    let badge = |state: &AppState, host: &str| {
        let ctx = crate::system::SectionCtx {
            remotes: &state.config_remotes,
            forward_health: &state.forward_health,
        };
        TmuxSystem
            .section_for(&TmuxSystem::host_lane(host), &ctx)
            .badge
    };

    let spec = |port: u16| ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: port,
        target_host: Some("127.0.0.1".into()),
        target_port: Some(port),
    };
    let f1 = spec(8001);
    let f2 = spec(8002);

    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.config_remotes = vec![
        RemoteConfig {
            host: "h1".into(),
            forwards: vec![f1.clone(), f2.clone()],
        },
        RemoteConfig {
            host: "nofwd".into(),
            forwards: vec![],
        },
    ];
    let set = |state: &mut AppState, a: ForwardHealth, b: ForwardHealth| {
        state
            .forward_health
            .insert(ForwardKey::from_spec("h1", &f1), a);
        state
            .forward_health
            .insert(ForwardKey::from_spec("h1", &f2), b);
    };

    // A host with no forwards never shows a badge.
    assert_eq!(badge(&state, "nofwd"), None);
    // An unknown host (not in config) likewise.
    assert_eq!(badge(&state, "ghost"), None);

    // The label counts every configured forward; status is the rollup.
    set(&mut state, ForwardHealth::Up, ForwardHealth::Up);
    let b = badge(&state, "h1").unwrap();
    assert_eq!((b.label.as_str(), b.status), ("⇄2", BadgeStatus::Ok));

    set(&mut state, ForwardHealth::Down, ForwardHealth::Down);
    assert_eq!(badge(&state, "h1").unwrap().status, BadgeStatus::Err);

    set(&mut state, ForwardHealth::Up, ForwardHealth::Down);
    assert_eq!(badge(&state, "h1").unwrap().status, BadgeStatus::Warn);

    // A probing forward alongside an up one is still "mixed", not all-up.
    set(&mut state, ForwardHealth::Up, ForwardHealth::Probing);
    assert_eq!(badge(&state, "h1").unwrap().status, BadgeStatus::Warn);

    // Nothing confirmed either way yet → neutral "idle".
    set(&mut state, ForwardHealth::Probing, ForwardHealth::Probing);
    assert_eq!(badge(&state, "h1").unwrap().status, BadgeStatus::Idle);
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
    assert_eq!(entry.host.as_deref(), Some("h1"));
    assert_eq!(entry.name, "s");
}

#[test]
fn context_menu_navigation_skips_disabled_items() {
    use crate::state::{ContextMenu, MenuItem, MenuKind};
    // A placeholder remote menu: every session item disabled.
    let all_disabled = ContextMenu {
        kind: MenuKind::Session {
            focus: FocusTarget(0),
            disabled: &[
                MenuItem::Rename,
                MenuItem::Kill,
                MenuItem::MoveUp,
                MenuItem::MoveDown,
            ],
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

    // One disabled item among enabled ones: navigation hops over it.
    // Items are the fixed session list: Rename, Kill, Move up, Move down.
    let mixed = ContextMenu {
        kind: MenuKind::Session {
            focus: FocusTarget(0),
            disabled: &[MenuItem::Kill],
        },
        x: 0,
        y: 0,
        selected: 0,
    };
    assert_eq!(mixed.first_enabled(), 0);
    // From "Rename" (0), next skips disabled "Kill" (1) to "Move up" (2).
    assert_eq!(mixed.next_enabled(), 2);
    let from_last = ContextMenu {
        selected: 2,
        ..mixed.clone()
    };
    // From "Move up" (2), prev skips "Kill" (1) back to "Rename" (0).
    assert_eq!(from_last.prev_enabled(), 0);
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
fn forward_key_from_spec_uses_mode_bind_and_listen() {
    use crate::forwards::{ForwardMode, ForwardSpec};
    use crate::state::ForwardKey;
    let spec = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: Some("127.0.0.1".into()),
        listen_port: 8080,
        target_host: Some("h".into()),
        target_port: Some(80),
    };
    let key = ForwardKey::from_spec("server-1", &spec);
    assert_eq!(key.host, "server-1");
    assert_eq!(key.mode, ForwardMode::Local);
    assert_eq!(key.bind_addr.as_deref(), Some("127.0.0.1"));
    assert_eq!(key.listen_port, 8080);
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
fn step_clamped_forward_stops_at_last() {
    // Mid-range advances by one; at the last index it stays put.
    assert_eq!(step_clamped(0, 3, 1), 1);
    assert_eq!(step_clamped(1, 3, 1), 2);
    assert_eq!(step_clamped(2, 3, 1), 2);
}

#[test]
fn step_clamped_back_stops_at_zero() {
    assert_eq!(step_clamped(2, 3, -1), 1);
    assert_eq!(step_clamped(1, 3, -1), 0);
    assert_eq!(step_clamped(0, 3, -1), 0);
}

#[test]
fn step_clamped_handles_empty_and_single() {
    // len == 0 always yields 0, either direction.
    assert_eq!(step_clamped(0, 0, 1), 0);
    assert_eq!(step_clamped(0, 0, -1), 0);
    // len == 1 pins to the only index.
    assert_eq!(step_clamped(0, 1, 1), 0);
    assert_eq!(step_clamped(0, 1, -1), 0);
}

#[test]
fn prefs_config_round_trip_is_identity() {
    // Phase-7 invariant: `from_config(to_config(p)) == p` on the prefs.
    // Start from a Config carrying already-normalized, non-default values
    // (so the load-time clamps are no-ops and the comparison is exact), map
    // it into prefs, write it back out, and re-derive — the prefs must match.
    let cfg = crate::config::Config {
        theme: crate::theme::THEMES[2].name.to_string(),
        layout: LayoutMode::Vertical,
        show_borders: false,
        sidebar_tab: SidebarTab::Agents,
        sidebar_width: 40,
        sidebar_height: 3,
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
        summary_prompt_projects: "projects prompt".to_string(),
        summary_prompt_projects_version: crate::summary::DEFAULT_SUMMARY_PROMPT_PROJECTS_VERSION,
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
        assert!(entry.is_local(), "flat index {i} should be local");
        assert_eq!(&entry.name, expected);
        assert_eq!(state.focusable_index_for(None, expected), Some(i));
    }
    // Remote rows follow the local block.
    let remote_flat = state.local_count();
    let entry = state.entry_at(FocusTarget(remote_flat)).unwrap();
    assert_eq!(entry.host.as_deref(), Some("h1"));
    assert_eq!(
        state.focusable_index_for(Some("h1"), "s"),
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
                host: Some("e".into()),
                name: String::new(),
                dir: String::new(),
                kind: SessionEntryKind::NoSessions,
            },
        ],
    );

    // 0,1 local; 2 remote Live; 3 remote Unreachable; 4 remote NoSessions.
    let e0 = state.entry_at(FocusTarget(0)).unwrap();
    assert!(e0.is_local() && e0.is_attachable());

    let e2 = state.entry_at(FocusTarget(2)).unwrap();
    assert_eq!(e2.host.as_deref(), Some("h"));
    assert!(e2.is_attachable());

    let e3 = state.entry_at(FocusTarget(3)).unwrap();
    assert_eq!(e3.host.as_deref(), Some("d"));
    assert_eq!(e3.kind, SessionEntryKind::Unreachable);
    assert!(!e3.is_attachable());

    let e4 = state.entry_at(FocusTarget(4)).unwrap();
    assert_eq!(e4.kind, SessionEntryKind::NoSessions);
    assert!(!e4.is_attachable());

    assert!(state.entry_at(FocusTarget(5)).is_none());
    // Section key reads off the entry's host directly.
    assert_eq!(state.section_key_of_focus(0), None);
    assert_eq!(state.section_key_of_focus(2), Some("h".to_string()));
}

#[test]
fn kill_policy_over_entries_guards_placeholder_and_last_remote() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24); // 2 locals
    set_remote(
        &mut state,
        vec![
            SessionEntry {
                host: Some("e".into()),
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
    assert_eq!(blocked(&state, 3), Some("last session on host")); // solo
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
                host: Some("h".into()),
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

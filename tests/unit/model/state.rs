use super::*;

fn make_session(name: &str) -> SessionRow {
    SessionRow {
        name: name.to_string(),
        dir: format!("/tmp/{name}"),
        is_current: false,
        idle_seconds: 0,
    }
}

fn make_state(
    layout_mode: LayoutMode,
    show_borders: bool,
    term_width: u16,
    term_height: u16,
) -> AppState {
    let mut state = AppState::new(
        0,
        layout_mode,
        ViewMode::Expanded,
        show_borders,
        SidebarTab::Projects,
        28,
        SIDEBAR_HEIGHT,
        5,
        term_width,
        term_height,
        vec![],
        vec![],
        Keybindings::default(),
        UpdateCheckMode::Enabled,
        std::collections::HashSet::new(),
    );
    state.sessions = vec![make_session("alpha"), make_session("beta")];
    state.session_order = state.sessions.iter().map(|s| s.name.clone()).collect();
    state.recompute_filter();
    state
}

fn remote_row(host: &str, unreachable: bool, loading: bool) -> RemoteSessionRow {
    RemoteSessionRow {
        host: host.to_string(),
        name: "s".to_string(),
        dir: "/tmp".to_string(),
        unreachable,
        loading,
    }
}

#[test]
fn mark_host_reconnecting_sets_loading_clears_unreachable() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.remote_sessions = vec![remote_row("h1", true, false)];
    state.mark_host_reconnecting("h1");
    assert!(state.remote_sessions[0].loading);
    assert!(!state.remote_sessions[0].unreachable);
}

#[test]
fn mark_host_reconnecting_ignores_other_hosts() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.remote_sessions = vec![remote_row("h2", true, false)];
    state.mark_host_reconnecting("h1");
    assert!(state.remote_sessions[0].unreachable);
    assert!(!state.remote_sessions[0].loading);
}

#[test]
fn sidebar_header_status_reflects_host_reachability() {
    let cases = [
        (remote_row("h1", true, false), HostStatus::Unreachable),
        (remote_row("h1", false, true), HostStatus::Connecting),
        (remote_row("h1", false, false), HostStatus::Connected),
    ];
    for (row, expected) in cases {
        let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
        state.remote_sessions = vec![row];
        state.recompute_filter();
        let layout = state.sidebar_layout(ViewMode::Expanded);
        let status = layout.items().iter().find_map(|item| match &item.data {
            SidebarItemData::Header { status, .. } => Some(*status),
            _ => None,
        });
        assert_eq!(status, Some(expected));
    }
}

#[test]
fn agent_rows_ordered_local_then_hosts() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.remote_sessions = vec![remote_row("h1", false, false)];
    state.recompute_filter();
    state.agents.insert(None, vec![detected("local", "%1")]);
    state
        .agents
        .insert(Some("h1".into()), vec![detected("h1s", "%2")]);

    // Agent rows appear in section order: local (`None`) then each host.
    let hosts: Vec<Option<String>> = state.agent_rows().iter().map(|r| r.host.clone()).collect();
    assert_eq!(hosts, vec![None, Some("h1".to_string())]);
}

#[test]
fn agents_layout_groups_agents_under_host_dividers() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.sidebar_tab = SidebarTab::Agents;
    state.remote_sessions = vec![remote_row("h1", false, false)];
    state.recompute_filter();
    state.agents.insert(None, vec![detected("local", "%1")]);
    state
        .agents
        .insert(Some("h1".into()), vec![detected("h1s", "%2")]);

    let layout = state.agents_layout();
    // Two focusable Agent rows, indexed 0 and 1 in agent_rows order.
    let agent_idxs: Vec<usize> = layout
        .items()
        .iter()
        .filter_map(|i| match &i.data {
            SidebarItemData::Agent { row_idx } => Some(*row_idx),
            _ => None,
        })
        .collect();
    assert_eq!(agent_idxs, vec![0, 1]);
    // Focusable count on the Agents tab is the agent count, not sessions.
    assert_eq!(state.focusable_count(), 2);
}

#[test]
fn agents_layout_pins_summary_card_at_top() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.sidebar_tab = SidebarTab::Agents;
    let layout = state.agents_layout();
    // First item is the Summary card; it sits above the @local divider.
    let first = &layout.items()[0].data;
    assert!(matches!(first, SidebarItemData::SummaryCard));
    let local_pos = layout
        .items()
        .iter()
        .position(|i| matches!(i.data, SidebarItemData::LocalHeader))
        .unwrap();
    assert!(local_pos > 0);
}

#[test]
fn summary_card_height_is_fixed_across_states() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    let idle = state.summary_card_height();
    state.summary = SummaryState::Generating;
    assert_eq!(state.summary_card_height(), idle);
    state.summary = SummaryState::Ready {
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
    state.summary_max_scroll = 3;
    state.scroll_summary(-5);
    assert_eq!(state.summary_scroll, 0, "can't scroll above the top");
    state.scroll_summary(10);
    assert_eq!(state.summary_scroll, 3, "clamped to max offset");
}

#[test]
fn agents_layout_shows_placeholder_for_empty_section() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.sidebar_tab = SidebarTab::Agents;
    // Local probed but empty -> "no agents"; never probed -> "detecting".
    state.agents.insert(None, vec![]);
    let layout = state.agents_layout();
    let placeholders: Vec<bool> = layout
        .items()
        .iter()
        .filter_map(|i| match &i.data {
            SidebarItemData::AgentsPlaceholder { detecting } => Some(*detecting),
            _ => None,
        })
        .collect();
    assert_eq!(placeholders, vec![false]);
}

fn detected(session: &str, pane_id: &str) -> crate::agent::DetectedAgent {
    crate::agent::DetectedAgent {
        kind: crate::agent::AgentKind::Claude,
        session: session.to_string(),
        window: "1".to_string(),
        pane: "0".to_string(),
        pane_id: pane_id.to_string(),
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
    state.agents.insert(None, vec![detected("local", "%1")]);
    state
        .agents
        .insert(Some("h1".into()), vec![detected("h1old", "%10")]);
    state
        .agents
        .insert(Some("h2".into()), vec![detected("h2old", "%20")]);

    // This round queried both hosts; only h1's probe succeeded.
    let covered: std::collections::HashSet<String> =
        ["h1".to_string(), "h2".to_string()].into_iter().collect();
    let mut fresh = std::collections::HashMap::new();
    fresh.insert("h1".to_string(), vec![detected("h1new", "%11")]);
    state.apply_remote_agents(covered, fresh);

    // h1 updated, h2 (failed probe) cleared, local untouched.
    assert_eq!(state.agents[&Some("h1".to_string())][0].pane_id, "%11");
    assert!(
        !state.agents.contains_key(&Some("h2".to_string())),
        "stale agents on a failed-probe host must be dropped"
    );
    assert!(state.agents.contains_key(&None), "local entry untouched");
}

#[test]
fn apply_remote_agents_prunes_unconfigured_hosts() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    // No remotes configured; a leftover host entry should be pruned.
    state
        .agents
        .insert(Some("ghost".into()), vec![detected("s", "%1")]);
    state.apply_remote_agents(Default::default(), Default::default());
    assert!(!state.agents.contains_key(&Some("ghost".to_string())));
}

#[test]
fn sidebar_layout_adds_local_header_in_expanded() {
    let state = make_state(LayoutMode::Horizontal, false, 80, 24);
    let layout = state.sidebar_layout(ViewMode::Expanded);

    let local_headers = layout
        .items()
        .iter()
        .filter(|i| matches!(i.data, SidebarItemData::LocalHeader))
        .count();
    assert_eq!(local_headers, 1, "one @local divider above the local rows");

    // The header is not a row, so local flat indices stay 0..N.
    let session_idxs: Vec<usize> = layout
        .items()
        .iter()
        .filter_map(|i| match &i.data {
            SidebarItemData::Session { session_idx } => Some(*session_idx),
            _ => None,
        })
        .collect();
    assert_eq!(session_idxs, vec![0, 1]);
}

#[test]
fn sidebar_layout_omits_local_header_in_compact() {
    let state = make_state(LayoutMode::Horizontal, false, 80, 24);
    let compact = state.sidebar_layout(ViewMode::Compact);
    assert!(
        !compact
            .items()
            .iter()
            .any(|i| matches!(i.data, SidebarItemData::LocalHeader)),
        "compact view carries no group dividers",
    );
}

#[test]
fn sidebar_layout_keeps_local_header_and_placeholder_when_empty() {
    let mut empty = make_state(LayoutMode::Horizontal, false, 80, 24);
    empty.sessions.clear();
    empty.recompute_filter();
    let layout = empty.sidebar_layout(ViewMode::Expanded);
    assert!(
        layout
            .items()
            .iter()
            .any(|i| matches!(i.data, SidebarItemData::LocalHeader)),
        "@local divider remains when there are no local sessions",
    );
    assert!(
        layout
            .items()
            .iter()
            .any(|i| matches!(i.data, SidebarItemData::LocalEmpty)),
        "empty local group shows a no-sessions placeholder",
    );
}

#[test]
fn is_divider_at_row_detects_header_not_session() {
    let state = make_state(LayoutMode::Horizontal, false, 80, 24);
    // Session area starts at row 2 (banner is 2 rows, no border). The
    // @local divider is the first item (1 row tall); the first session
    // card sits just below it.
    assert!(state.is_divider_at_row(2), "row 2 is the @local divider");
    assert!(!state.is_divider_at_row(3), "row 3 is a session card");
    assert_eq!(state.focus_at_row(3), Some(FocusTarget(0)));
    // Rows in the header banner above the session area aren't dividers.
    assert!(!state.is_divider_at_row(0));
}

#[test]
fn local_divider_menu_greys_remote_only_items() {
    use crate::state::{ContextMenu, MenuKind};
    let menu = ContextMenu {
        kind: MenuKind::LocalDivider,
        x: 0,
        y: 0,
        selected: 0,
    };
    // Same item list as a host divider...
    assert!(menu.items().contains(&"New session"));
    assert!(menu.items().contains(&"Port Forward"));
    assert!(menu.items().contains(&"Remove from list"));
    // ...but the remote-only ones are greyed out, leaving New session live.
    assert!(menu.disabled().contains(&"Port Forward"));
    assert!(menu.disabled().contains(&"Remove from list"));
    assert!(!menu.disabled().contains(&"New session"));
    // The initial highlight lands on the first enabled item.
    assert_eq!(menu.items()[menu.first_enabled()], "New session");
}

#[test]
fn sync_remote_forward_health_mirrors_host_status() {
    use crate::config::{ForwardMode, ForwardSpec, RemoteConfig};
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
        state.remote_sessions = vec![row];
        state.sync_remote_forward_health();
        assert_eq!(state.forward_health.get(&key).copied(), Some(expected));
    }
}

#[test]
fn resize_sidebar_handles_small_terminals() {
    let mut state = make_state(LayoutMode::Horizontal, true, 20, 40);
    assert!(state.resize_sidebar(30));
    assert_eq!(state.sidebar_width, 10);
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
    state.remote_sessions = vec![remote_row("h1", false, false)];

    // Tab columns (bordered, leading pad 1): alpha (1..9), beta (10..17),
    // h1:s (18..25). Probe a column inside the remote tab, offset by the
    // left border.
    let hit = state.session_at_col(1 + 20, 1);
    assert_eq!(hit, Some(2));

    match state.session_target(FocusTarget(hit.unwrap())) {
        Some(SessionTargetRef::Remote(row)) => {
            assert_eq!(row.host, "h1");
            assert_eq!(row.name, "s");
        }
        other => panic!("expected remote target, got {other:?}"),
    }
}

#[test]
fn context_menu_navigation_skips_disabled_items() {
    use crate::state::{ContextMenu, MenuKind};
    // A placeholder remote menu: both items disabled.
    let all_disabled = ContextMenu {
        kind: MenuKind::Session {
            focus: FocusTarget(0),
            items: &["Rename", "Kill"],
            disabled: &["Rename", "Kill"],
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
    let mixed = ContextMenu {
        kind: MenuKind::Session {
            focus: FocusTarget(0),
            items: &["Rename", "Kill", "Move up"],
            disabled: &["Kill"],
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

use crate::config::ForwardMode;
use crate::state::{PfAddForm, PfField, PfFormError};
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
fn rollup_down_dominates() {
    use crate::state::{rollup_color, ForwardHealth, PfBadgeColor};
    let healths = [
        ForwardHealth::Up,
        ForwardHealth::Down,
        ForwardHealth::Probing,
    ];
    assert_eq!(rollup_color(&healths), PfBadgeColor::Degraded);
}

#[test]
fn rollup_probing_when_no_down() {
    use crate::state::{rollup_color, ForwardHealth, PfBadgeColor};
    let healths = [ForwardHealth::Up, ForwardHealth::Probing];
    assert_eq!(rollup_color(&healths), PfBadgeColor::Probing);
}

#[test]
fn rollup_healthy_when_all_up() {
    use crate::state::{rollup_color, ForwardHealth, PfBadgeColor};
    let healths = [ForwardHealth::Up, ForwardHealth::Up];
    assert_eq!(rollup_color(&healths), PfBadgeColor::Healthy);
}

#[test]
fn forward_key_from_spec_uses_mode_bind_and_listen() {
    use crate::config::{ForwardMode, ForwardSpec};
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
    // Issue #41: killing a remote session set overlay.confirm_kill but the
    // overlay name was looked up only in the local store, so it resolved to
    // None and the confirm dialog never drew (then leaked onto the next
    // local row). The name must come from whichever store the focused row
    // lives in.
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.remote_sessions = vec![remote_row("h1", false, false)]; // named "s"
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
    state.remote_sessions = vec![remote_row("h1", false, false)];
    state.focused = 2;
    // No pending kill -> no name regardless of what's focused.
    assert_eq!(state.confirm_kill_name(), None);
}

#[test]
fn collapsed_local_group_hides_rows() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.collapsed_sections.insert(None);
    let layout = state.sidebar_layout(ViewMode::Expanded);

    assert!(layout.is_collapsible());
    // The local header is collapsed; its rows (idx 0,1) are hidden.
    assert!(layout.is_row_hidden(0));
    assert!(layout.is_row_hidden(1));
}

#[test]
fn expanded_local_group_shows_rows() {
    let state = make_state(LayoutMode::Horizontal, false, 80, 24);
    let layout = state.sidebar_layout(ViewMode::Expanded);
    assert!(!layout.is_row_hidden(0));
}

#[test]
fn focus_skips_collapsed_remote_group() {
    // Layout: 2 local rows (idx 0,1), then host h1 (idx 2). Collapse local.
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.remote_sessions = vec![remote_row("h1", false, false)];
    state.recompute_filter();
    state.collapsed_sections.insert(None);

    // Local rows are hidden, the remote row is not.
    assert!(state.is_focus_collapsed(0));
    assert!(state.is_focus_collapsed(1));
    assert!(!state.is_focus_collapsed(2));
}

#[test]
fn section_key_of_focus_maps_local_and_remote() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.remote_sessions = vec![remote_row("h1", false, false)];
    state.recompute_filter();
    assert_eq!(state.section_key_of_focus(0), None); // local
    assert_eq!(state.section_key_of_focus(1), None); // local
    assert_eq!(state.section_key_of_focus(2), Some("h1".to_string())); // remote
}

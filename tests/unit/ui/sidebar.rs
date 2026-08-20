use super::*;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::geometry::{banner_visible, sidebar_footer_height, SIDEBAR_HEADER_HEIGHT};
use crate::geometry::{HitKind, HitRegions};
use crate::state::{SessionHighlight, SidebarTab};
use crate::summary_card::SummaryState;
use crate::update::UpdateStatus;

static IDLE_SUMMARY: SummaryState = SummaryState::Idle;

fn sidebar_props<'a>(
    sessions: &'a [crate::state::SessionEntry],
    built: &'a BuiltLayout,
    theme: &'a crate::theme::Theme,
    keybindings: &'a Keybindings,
) -> SidebarProps<'a> {
    SidebarProps {
        sessions,
        built,
        // Overridden by the tabs-mode tests, which are the only ones that draw
        // a tab bar; every other layout ignores it.
        tab_labels: &[],
        focus_target: None,
        project_drag: None,
        sidebar_active: true,
        theme,
        show_help: false,
        confirm_kill: None,
        rename_input: None,
        show_borders: true,
        sidebar_tab: SidebarTab::Projects,
        session_highlight: SessionHighlight::Solid,
        agent_entries: &[],
        summary: &IDLE_SUMMARY,
        summary_age: None,
        spinner_idx: 0,
        summary_scroll: 0,
        summary_card_height: 0,
        tabs_mode: false,
        keybindings,
        update_available: None,
    }
}

fn render_sidebar(
    width: u16,
    height: u16,
    area: Option<Rect>,
    props: SidebarProps<'_>,
) -> (Terminal<TestBackend>, HitRegions) {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut captured = HitRegions::default();
    terminal
        .draw(|frame| {
            captured = super::draw_sidebar(frame, area.unwrap_or_else(|| frame.area()), props);
        })
        .unwrap();
    (terminal, captured)
}

fn mount_tmux_sections(state: &mut crate::state::AppState) {
    use crate::system::System;

    // Configuring a system rewrites a process-wide table other tests assert on
    // — see `crate::system::serial`.
    let _serial = crate::system::serial::configure_lock();
    let system = crate::system::tmux::TmuxSystem::default();
    let config = crate::config::Config::default();
    system.configure(&config, &state.config_remotes);
    state.system_sections = system
        .lanes()
        .into_iter()
        .filter_map(|lane| system.section_for(&lane))
        .collect();
}

#[test]
fn confirm_kill_renders_clickable_in_tabs_mode() {
    // In tabs mode the confirm-kill prompt must render and publish hit
    // regions; otherwise the mouse guard swallows every click while
    // confirm_kill is set and the clickable buttons never work.
    let theme = &crate::theme::THEMES[0];
    let built = BuiltLayout::default();
    let keybindings = Keybindings::default();
    let sessions: Vec<crate::state::SessionEntry> = Vec::new();
    let props = SidebarProps {
        confirm_kill: Some("victim"),
        tabs_mode: true,
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    let (terminal, captured) = render_sidebar(30, 12, None, props);

    let hits = captured
        .kill
        .expect("kill prompt must publish hit regions in tabs mode");
    assert_eq!(hits.no.y, hits.yes.y, "buttons share the button row");
    assert!(
        hits.no.x + hits.no.width <= hits.yes.x,
        "No/Yes buttons must not overlap"
    );

    // The prompt text is actually painted, not just hit regions reported.
    let buf = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            text.push_str(buf[(x, y)].symbol());
        }
    }
    assert!(
        text.contains("Close victim"),
        "prompt text missing: {text:?}"
    );
}

#[test]
fn rename_popup_is_visible_for_vertical_layout() {
    let theme = &crate::theme::THEMES[0];
    let backend = TestBackend::new(60, 16);
    let mut terminal = Terminal::new(backend).unwrap();
    let input = ratatui_textarea::TextArea::new(vec!["renamed-session".to_string()]);

    terminal
        .draw(|frame| super::draw_rename_popup(frame, frame.area(), theme, &input))
        .unwrap();

    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Rename session"));
    assert!(text.contains("renamed-session"));
    assert!(text.contains("Enter confirm / Esc cancel"));
}

#[test]
fn collapsed_sidebar_draws_a_clickable_expand_rail() {
    let theme = &crate::theme::THEMES[0];
    let backend = TestBackend::new(20, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut hits = HitRegions::default();
    terminal
        .draw(|frame| {
            hits = super::draw_collapsed_sidebar(
                frame,
                Rect::new(0, 0, crate::state::SIDEBAR_COLLAPSED_WIDTH, 10),
                theme,
                true,
            );
        })
        .unwrap();

    let toggle = hits.sidebar_toggle.expect("expand control");
    assert_eq!(toggle.x, 1, "expand control is centered between borders");
    assert_eq!(hits.hit(toggle.x, toggle.y), Some(HitKind::SidebarToggle));
    assert_eq!(
        terminal.backend().buffer()[(toggle.x, toggle.y)].symbol(),
        "›"
    );
}

#[test]
fn collapsed_borderless_sidebar_centers_expand_control() {
    let theme = &crate::theme::THEMES[0];
    let backend = TestBackend::new(20, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut hits = HitRegions::default();
    terminal
        .draw(|frame| {
            hits = super::draw_collapsed_sidebar(
                frame,
                Rect::new(4, 2, crate::state::SIDEBAR_COLLAPSED_WIDTH, 8),
                theme,
                false,
            );
        })
        .unwrap();

    let toggle = hits.sidebar_toggle.expect("expand control");
    assert_eq!(toggle, Rect::new(5, 2, 1, 1));
    assert_eq!(hits.hit(toggle.x, toggle.y), Some(HitKind::SidebarToggle));
    assert_eq!(
        terminal.backend().buffer()[(toggle.x, toggle.y)].symbol(),
        "›"
    );
}

#[test]
fn idle_summary_without_agents_is_compact_and_disabled() {
    use crate::state::AppState;

    let theme = &crate::theme::THEMES[0];
    let keybindings = Keybindings::default();
    let mut state = AppState::new(100, 20);
    state.prefs.show_borders = false;
    state.prefs.sidebar_tab = SidebarTab::Agents;
    state
        .agents
        .insert(crate::system::tmux::lane(None), Vec::new());
    state.rebuild_agent_entries();
    let built = state.agents_layout(crate::state::ViewMode::Expanded);
    let sessions: Vec<crate::state::SessionEntry> = Vec::new();

    let props = SidebarProps {
        focus_target: state.focus_target(),
        show_borders: false,
        sidebar_tab: SidebarTab::Agents,
        agent_entries: &state.agent_entries,
        summary_card_height: state.summary_card_height(),
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    let (terminal, captured) = render_sidebar(30, 20, None, props);

    assert_eq!(captured.summary.card.unwrap().height, 3);
    assert!(captured.summary.button.is_none());
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("No agents detected"));
}

#[test]
fn overflowing_vertical_tabs_keep_focus_and_menu_visible() {
    use crate::state::{AppState, SessionEntry, SessionEntryKind};

    let theme = &crate::theme::THEMES[0];
    let keybindings = Keybindings::default();
    let mut state = AppState::new(40, 12);
    state.prefs.show_borders = true;
    state.entries = (0..10)
        .map(|i| SessionEntry {
            lane: crate::system::tmux::TmuxSystem::local_lane(),
            name: format!("session-{i}"),
            dir: String::new(),
            kind: SessionEntryKind::Live { is_current: false },
        })
        .collect();
    state.focused = 6;
    let built = state.sidebar_layout(state.prefs.view_mode);
    let sessions = state.entries.clone();

    let labels = state.tab_labels();
    let props = SidebarProps {
        focus_target: state.focus_target(),
        tabs_mode: true,
        tab_labels: &labels,
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    let (terminal, captured) = render_sidebar(40, 12, Some(Rect::new(0, 0, 40, 3)), props);

    let menu = captured.menu.expect("menu remains pinned on overflow");
    assert_eq!(
        menu.x + menu.width + 1,
        39,
        "one trailing pad before border"
    );
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains('…'));
    assert!(text.contains("≡ menu"));
    assert!(text.contains("7 session-6"));
}

// --- one geometry, one hit-test ---

/// Render the sidebar at `width` x `height` on the Projects tab and return
/// the captured hit registry. `has_update` toggles the footer banner.
fn render_hits(width: u16, height: u16, has_update: bool) -> HitRegions {
    let theme = &crate::theme::THEMES[0];
    let built = BuiltLayout::default();
    let keybindings = Keybindings::default();
    let sessions: Vec<crate::state::SessionEntry> = Vec::new();
    let update = UpdateStatus {
        latest_version: "9.9.9".to_string(),
        current_version: "0.0.1".to_string(),
        checked_at: 0,
    };

    let props = SidebarProps {
        update_available: has_update.then_some(&update),
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    render_sidebar(width, height, None, props).1
}

#[test]
fn header_shows_live_counts_without_duplicate_new_action() {
    use crate::state::{AppState, SessionEntry, SessionEntryKind};

    let theme = &crate::theme::THEMES[0];
    let keybindings = Keybindings::default();
    let mut state = AppState::new(50, 20);
    state.prefs.view_mode = crate::state::ViewMode::Expanded;
    state.entries = vec![
        SessionEntry {
            lane: crate::system::tmux::TmuxSystem::local_lane(),
            name: "work".to_string(),
            dir: "/tmp/work".to_string(),
            kind: SessionEntryKind::Live { is_current: true },
        },
        SessionEntry {
            lane: crate::system::tmux::TmuxSystem::host_lane("offline"),
            name: String::new(),
            dir: String::new(),
            kind: SessionEntryKind::Unreachable,
        },
    ];
    mount_tmux_sections(&mut state);
    state.agents.insert(
        crate::system::tmux::lane(None),
        vec![crate::agent::DetectedAgent {
            kind: crate::agent::AgentKind::Claude,
            session: "work".to_string(),
            window: "1".to_string(),
            pane_id: "%1".to_string(),
            status: crate::agent::AgentStatus::Working,
        }],
    );
    state.rebuild_agent_entries();
    let built = state.sidebar_layout(state.prefs.view_mode);
    let sessions = state.entries.clone();

    let props = SidebarProps {
        focus_target: state.focus_target(),
        show_borders: false,
        agent_entries: &state.agent_entries,
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    let (terminal, captured) = render_sidebar(50, 20, None, props);

    let first_row: String = (0..50)
        .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
        .collect();
    assert!(first_row.contains("Sessions 1"));
    assert!(first_row.contains("Agents 1"));
    assert!(!first_row.contains("+ New"));
    assert!(!first_row.contains(" + "));
    assert!(captured.tabs.is_some());

    let local_new = captured
        .dividers
        .iter()
        .find(|hit| {
            crate::system::tmux::TmuxSystem::host_of(&hit.lane).is_none()
                && hit.action.as_str() == "new-session"
        })
        .expect("local divider keeps its direct new-session button");
    assert_eq!(
        terminal.backend().buffer()[(local_new.rect.x + 1, local_new.rect.y)].symbol(),
        "+"
    );
}

#[test]
fn footer_is_contextual_and_drops_persistent_version_text() {
    let render = |sidebar_active: bool| {
        let theme = &crate::theme::THEMES[0];
        let built = BuiltLayout::default();
        let keybindings = Keybindings::default();
        let sessions: Vec<crate::state::SessionEntry> = Vec::new();
        let props = SidebarProps {
            sidebar_active,
            show_borders: false,
            ..sidebar_props(&sessions, &built, theme, &keybindings)
        };
        let (terminal, _) = render_sidebar(50, 12, None, props);
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };

    let sidebar = render(true);
    assert!(sidebar.contains("Sidebar"));
    assert!(sidebar.contains("n new"));
    assert!(sidebar.contains("h/? help"));
    assert!(sidebar.contains("≡ menu"));
    assert!(!sidebar.contains("deck v"));

    let terminal = render(false);
    assert!(terminal.contains("Terminal"));
    assert!(terminal.contains("ctrl-s sidebar"));
    assert!(!terminal.contains("n new"));
}

#[test]
fn agents_tab_publishes_clickable_agent_entries() {
    use crate::state::{AppState, SidebarTab};
    let theme = &crate::theme::THEMES[0];
    let keybindings = Keybindings::default();

    let mk = |pane_id: &str| crate::agent::DetectedAgent {
        kind: crate::agent::AgentKind::Claude,
        session: "sess".to_string(),
        window: "1".to_string(),
        pane_id: pane_id.to_string(),
        status: crate::agent::AgentStatus::Idle,
    };

    let mut state = AppState::new(100, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    state.config_remotes = vec![crate::config::RemoteConfig {
        host: "h1".into(),
        containers: vec![],
        forward_agent: true,
        forwards: vec![],
    }];
    mount_tmux_sections(&mut state);
    // Two local agents and one remote, so the click→pane mapping has to
    // survive dividers/margins between sections (the "specific pane" path).
    state.entries.push(crate::state::SessionEntry {
        lane: crate::system::tmux::TmuxSystem::host_lane("h1"),
        name: "s".to_string(),
        dir: String::new(),
        kind: crate::state::SessionEntryKind::Live { is_current: false },
    });
    state.clamp_projects_focus();
    state
        .agents
        .insert(crate::system::tmux::lane(None), vec![mk("%7"), mk("%8")]);
    state
        .agents
        .insert(crate::system::tmux::lane(Some("h1")), vec![mk("%9")]);
    state.rebuild_agent_entries();
    let built = state.agents_layout(crate::state::ViewMode::Expanded);
    let agent_entries = state.agent_entries.clone();
    let sessions: Vec<crate::state::SessionEntry> = Vec::new();

    let props = SidebarProps {
        focus_target: state.focus_target(),
        sidebar_tab: SidebarTab::Agents,
        agent_entries: &agent_entries,
        summary_card_height: state.summary_card_height(),
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    let (_, captured) = render_sidebar(40, 24, None, props);

    // Each agent row publishes a hit, in agent_entries order, with its pane.
    let panes: Vec<&str> = captured
        .agents
        .iter()
        .map(|h| h.target.pane_id.as_str())
        .collect();
    assert_eq!(panes, vec!["%7", "%8", "%9"], "rows map to their agents");

    // End-to-end: clicking the LAST agent row (across the host divider)
    // yields a switch to *that* agent's pane, not a neighbor's.
    state.hit_regions = captured;
    let last = state.hit_regions.agents.last().unwrap().rect;
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: last.x,
        row: last.y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    match crate::action::mouse_to_action(&click, &state) {
        crate::action::Action::SwitchToAgentPane(t) => {
            assert_eq!(t.pane_id, "%9");
            assert_eq!(t.lane, crate::system::tmux::TmuxSystem::host_lane("h1"));
        }
        other => panic!("expected SwitchToAgentPane, got {other:?}"),
    }
}

#[test]
fn remote_divider_buttons_register_below_their_top_margin() {
    // A remote host divider carries a 1-row top margin (section spacing),
    // so its bar — and its `[⟳]`/`[…]` buttons — paint one row *below* the
    // header block's top. The hit rects must follow the bar to that row;
    // earlier they were published at the block top (the inert margin row), so
    // the bar's buttons resolved to nothing and a click fell through to
    // collapse the section instead of reconnecting / opening the menu.
    use crate::state::{AppState, SidebarTab, ViewMode};
    use crate::system::tmux::TmuxSystem;

    let theme = &crate::theme::THEMES[0];
    let keybindings = Keybindings::default();

    let mut state = AppState::new(80, 24);
    state.prefs.sidebar_tab = SidebarTab::Projects;
    state.config_remotes = vec![crate::config::RemoteConfig {
        host: "h1".into(),
        containers: vec![],
        forward_agent: true,
        forwards: vec![],
    }];
    mount_tmux_sections(&mut state);
    // One remote host so the layout has a margined `@h1` divider.
    state.entries.push(crate::state::SessionEntry {
        lane: crate::system::tmux::TmuxSystem::host_lane("h1"),
        name: "s".to_string(),
        dir: String::new(),
        kind: crate::state::SessionEntryKind::Live { is_current: false },
    });
    state.clamp_projects_focus();
    let built = state.sidebar_layout(ViewMode::Expanded);
    let sessions = state.entries.clone();

    let props = SidebarProps {
        focus_target: state.focus_target(),
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    let (terminal, captured) = render_sidebar(40, 24, None, props);

    // The remote divider publishes both of its buttons, in order.
    let h1: Vec<&crate::geometry::DividerHit> = captured
        .dividers
        .iter()
        .filter(|h| TmuxSystem::host_of(&h.lane) == Some("h1"))
        .collect();
    let cmds: Vec<&str> = h1.iter().map(|h| h.action.as_str()).collect();
    assert_eq!(
        cmds,
        vec!["reconnect", "menu"],
        "remote `@h1` divider must register its reconnect + more buttons"
    );

    // Each button rect resolves back through the priority resolver (so a
    // click there is a button, not a collapse) and lands on a painted `[`
    // of the rendered bar — proving the rect tracks the bar row, not the
    // blank margin row above it.
    let buf = terminal.backend().buffer();
    for h in &h1 {
        let pos = (h.rect.x, h.rect.y);
        assert!(
            matches!(captured.hit(h.rect.x, h.rect.y), Some(HitKind::Divider(_))),
            "{:?} button rect {:?} must resolve to a divider hit",
            h.action,
            h.rect
        );
        assert_eq!(
            buf[pos].symbol(),
            "[",
            "{:?} button rect {:?} must sit on the painted `[icon]`",
            h.action,
            h.rect
        );
    }
}

#[test]
fn remote_divider_shows_forward_count() {
    // A remote host with configured forwards grows a leftmost `[⇄N]` button on
    // its divider: N counts the forwards (deck no longer probes per-forward
    // health), and a click opens that host's port-forward overlay (not a
    // collapse).
    use crate::config::RemoteConfig;
    use crate::forwards::{ForwardMode, ForwardSpec};
    use crate::state::{AppState, SidebarTab, ViewMode};
    use crate::system::tmux::TmuxSystem;

    let theme = &crate::theme::THEMES[0];
    let keybindings = Keybindings::default();

    let spec = |port: u16| ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: port,
        target_host: Some("127.0.0.1".into()),
        target_port: Some(port),
    };
    let (f1, f2) = (spec(8001), spec(8002));

    let mut state = AppState::new(100, 24);
    state.prefs.sidebar_tab = SidebarTab::Projects;
    state.prefs.sidebar_width = 40;
    state.config_remotes = vec![RemoteConfig {
        host: "h1".into(),
        containers: vec![],
        forward_agent: true,
        forwards: vec![f1.clone(), f2.clone()],
    }];
    mount_tmux_sections(&mut state);
    state.entries.push(crate::state::SessionEntry {
        lane: crate::system::tmux::TmuxSystem::host_lane("h1"),
        name: "s".to_string(),
        dir: String::new(),
        kind: crate::state::SessionEntryKind::Live { is_current: false },
    });
    state.clamp_projects_focus();

    let built = state.sidebar_layout(ViewMode::Expanded);
    let sessions = state.entries.clone();

    let props = SidebarProps {
        focus_target: state.focus_target(),
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    let (terminal, captured) = render_sidebar(40, 24, None, props);

    // Buttons register left→right as badge, reconnect, more.
    let h1: Vec<&crate::geometry::DividerHit> = captured
        .dividers
        .iter()
        .filter(|h| TmuxSystem::host_of(&h.lane) == Some("h1"))
        .collect();
    let cmds: Vec<&str> = h1.iter().map(|h| h.action.as_str()).collect();
    assert_eq!(
        cmds,
        vec!["forwards", "reconnect", "menu"],
        "the forward badge must be the leftmost divider button"
    );
    let badge = h1[0];
    assert!(
        badge.rect.x < h1[1].rect.x,
        "badge sits to the left of the reconnect button"
    );

    // The button renders `[⇄2]` — the count of configured forwards.
    let buf = terminal.backend().buffer();
    assert_eq!(buf[(badge.rect.x, badge.rect.y)].symbol(), "[");
    assert_eq!(buf[(badge.rect.x + 1, badge.rect.y)].symbol(), "⇄");
    assert_eq!(buf[(badge.rect.x + 2, badge.rect.y)].symbol(), "2");

    // Clicking the badge opens the host's port-forward overlay.
    let badge_rect = badge.rect;
    state.hit_regions = captured;
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: badge_rect.x,
        row: badge_rect.y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    // The click yields a typed lane action carrying the lane + backend id;
    // the tmux System turns "forwards" into the
    // port-forward overlay (verified in the system's own tests).
    match crate::action::mouse_to_action(&click, &state) {
        crate::action::Action::InvokeLane { lane, action, .. } => {
            assert_eq!(action.as_str(), "forwards");
            assert_eq!(TmuxSystem::host_of(&lane), Some("h1"));
        }
        other => panic!("expected InvokeLane, got {other:?}"),
    }
}

/// The content area `draw_sidebar` lays out within for a horizontal
/// sidebar with borders: the bordered container insets the full area by
/// one cell on each side.
fn content_area(width: u16, height: u16) -> Rect {
    Rect::new(1, 1, width.saturating_sub(2), height.saturating_sub(2))
}

#[test]
fn footer_allocation_matches_shared_formula() {
    // The renderer must split off exactly `sidebar_footer_height` rows for
    // the footer, with or without the update banner — the same formula
    // the mouse hit-tester uses (`AppState::sidebar_footer_height`), so a
    // click can't land a row off from what was drawn.
    let width = 30;
    let height = 24;
    let content = content_area(width, height);

    for has_update in [false, true] {
        let hits = render_hits(width, height, has_update);
        let banner_shown = banner_visible(has_update, content.width);
        let footer_height = sidebar_footer_height(banner_shown);
        // The footer occupies the bottom `footer_height` rows of the
        // content area; the tab labels sit on the header's first row.
        let tabs = hits.tabs.expect("Projects tab has a header");
        assert_eq!(
            tabs.projects.y, content.y,
            "tab row must be the header's first row"
        );
        // The header is `SIDEBAR_HEADER_HEIGHT` rows; sessions fill the
        // middle; the footer is the formula's height — so the sessions
        // area height is content - header - footer, and must be >= 1.
        let sessions_h = content
            .height
            .saturating_sub(SIDEBAR_HEADER_HEIGHT)
            .saturating_sub(footer_height);
        assert!(
            sessions_h >= 1,
            "sessions area underflowed (banner {banner_shown})"
        );
        // The banner hit, when shown, lands inside the footer band.
        if banner_shown {
            let banner = hits.banner.expect("banner hit present when shown");
            let footer_top = content.bottom() - footer_height;
            assert!(
                banner.y >= footer_top && banner.y < content.bottom(),
                "banner row {} outside footer band [{footer_top}, {})",
                banner.y,
                content.bottom()
            );
        }
    }
}

#[test]
fn captured_rects_stay_within_sidebar_area() {
    // Every published rect must be a subset of the sidebar content area —
    // nothing may reach into the PTY pane, at any width including very
    // narrow ones.
    for width in [14u16, 15, 16, 20, 30, 48] {
        let height = 24;
        let content = content_area(width, height);
        let right = content.x + content.width;
        let bottom = content.y + content.height;
        let hits = render_hits(width, height, true);

        let mut rects: Vec<Rect> = Vec::new();
        rects.extend(hits.banner);
        rects.extend(hits.menu);
        rects.extend(hits.sidebar_toggle);
        rects.extend(hits.summary.button);
        rects.extend(hits.summary.popup);
        rects.extend(hits.summary.card);
        if let Some(t) = hits.tabs {
            // A zero-width (clamped-away) tab rect is fine; only non-empty
            // rects must sit inside the area.
            if t.projects.width > 0 {
                rects.push(t.projects);
            }
            if t.agents.width > 0 {
                rects.push(t.agents);
            }
        }
        rects.extend(hits.dividers.iter().map(|h| h.rect));
        rects.extend(hits.agents.iter().map(|h| h.rect));
        if let Some(k) = hits.kill {
            rects.push(k.yes);
            rects.push(k.no);
        }

        for r in rects {
            assert!(
                r.x >= content.x && r.x + r.width <= right,
                "rect {r:?} escapes horizontally at width {width} (content {content:?})"
            );
            assert!(
                r.y >= content.y && r.y + r.height <= bottom,
                "rect {r:?} escapes vertically at width {width} (content {content:?})"
            );
        }
    }
}

#[test]
fn narrow_agents_tab_does_not_leak_into_pty() {
    // At a narrow sidebar width the un-clamped `Agents` tab label
    // overflows the sidebar. A click in a column beyond the sidebar must
    // resolve to `None`, never `Tab(Agents)`.
    let width = 16;
    let height = 24;
    let content = content_area(width, height);
    let hits = render_hits(width, height, false);

    // A column at/just past the sidebar's right edge is outside the area.
    let beyond = content.x + content.width;
    for row in content.y..content.y + SIDEBAR_HEADER_HEIGHT {
        assert_ne!(
            hits.hit(beyond, row),
            Some(HitKind::Tab(SidebarTab::Agents)),
            "click at col {beyond}, row {row} leaked into a tab hit"
        );
        assert_eq!(
            hits.hit(beyond, row),
            None,
            "click past the sidebar must be inert"
        );
    }
}

#[test]
fn container_dividers_render_as_a_branch_under_their_host() {
    // The sidebar is the only place the nesting is visible, so this pins what
    // it draws: the host's own divider, then its containers indented beneath
    // it with no blank row between, and the host name spent only once.
    use crate::config::{ContainerConfig, RemoteConfig};
    use crate::state::{AppState, SidebarTab, ViewMode};
    use crate::system::tmux::TmuxSystem;

    let theme = &crate::theme::THEMES[0];
    let keybindings = Keybindings::default();

    let mut state = AppState::new(100, 24);
    state.prefs.sidebar_tab = SidebarTab::Projects;
    state.prefs.sidebar_width = 32;
    state.config_remotes = vec![RemoteConfig {
        host: "devbox".into(),
        forward_agent: true,
        forwards: vec![],
        containers: ["dev", "build"]
            .into_iter()
            .map(|name| ContainerConfig {
                name: name.into(),
                engine: "docker".into(),
                agent_sock: None,
                forwards: vec![],
            })
            .collect(),
    }];
    mount_tmux_sections(&mut state);
    for lane in [
        TmuxSystem::host_lane("devbox"),
        TmuxSystem::container_lane("devbox", "dev"),
        TmuxSystem::container_lane("devbox", "build"),
    ] {
        state.entries.push(crate::state::SessionEntry {
            lane,
            name: "s".to_string(),
            dir: String::new(),
            kind: crate::state::SessionEntryKind::Live { is_current: false },
        });
    }
    state.clamp_projects_focus();

    let built = state.sidebar_layout(ViewMode::Expanded);
    let sessions = state.entries.clone();
    // `Subtle`, so the focused row still carries its own gutter mark: this
    // test is about the run being continuous, and the other candidate answers
    // the focused row differently (see
    // `a_solid_selection_block_occludes_the_trunk_it_covers`).
    let props = SidebarProps {
        focus_target: state.focus_target(),
        session_highlight: SessionHighlight::Subtle,
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    let (terminal, _) = render_sidebar(32, 24, None, props);

    let buf = terminal.backend().buffer();
    let lines: Vec<String> = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    let row_of = |needle: &str| {
        lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle} not drawn in {lines:#?}"))
    };
    let host = row_of(" devbox ");
    // Drawn under the host, in mount order, each naming only itself — the
    // machine is on the divider above and does not need repeating. The
    // connector comes first and the collapse chevron after it: the line
    // establishes what the section belongs to, the chevron acts on it.
    assert!(host < row_of("├ ▾ dev "));
    assert!(row_of("├ ▾ dev ") < row_of("└ ▾ build "));
    // And the host half is spent once: no `devbox/dev` eating the width the
    // container name needs.
    assert_eq!(
        lines.iter().filter(|line| line.contains("devbox")).count(),
        1,
        "{lines:#?}"
    );

    // The line reaches the elbows: every row between a divider and the nested
    // one below it carries the trunk down its gutter, so the connector joins
    // the group it came from instead of dangling. A focus/drag marker holds
    // that same cell and reads as an emphasized segment of the run.
    let gutter = |row: usize| lines[row].chars().nth(1).unwrap_or(' ');
    let dev = row_of("├ ▾ dev ");
    let build = row_of("└ ▾ build ");
    for row in (host + 1..dev).chain(dev + 1..build) {
        assert!(
            matches!(gutter(row), '│' | '▌'),
            "gap in the trunk at row {row}: {lines:#?}"
        );
    }
    // And it stops at the last child: nothing below belongs to the branch.
    for row in build + 1..lines.len() {
        assert_ne!(gutter(row), '│', "trunk outlives the branch: {lines:#?}");
    }
}

#[test]
fn a_solid_selection_block_occludes_the_trunk_it_covers() {
    // The `Solid` candidate paints the focused row as one filled block, and
    // anything drawn in its gutter — the trunk included — would be a dark mark
    // punched out of that block. So the line passes behind the selection: the
    // focused row's gutter is blank, and the run resumes on the row below.
    use crate::config::{ContainerConfig, RemoteConfig};
    use crate::state::{AppState, SidebarTab, ViewMode};
    use crate::system::tmux::TmuxSystem;

    let theme = &crate::theme::THEMES[0];
    let keybindings = Keybindings::default();

    let mut state = AppState::new(100, 24);
    state.prefs.sidebar_tab = SidebarTab::Projects;
    state.prefs.sidebar_width = 32;
    state.config_remotes = vec![RemoteConfig {
        host: "devbox".into(),
        forward_agent: true,
        forwards: vec![],
        containers: vec![ContainerConfig {
            name: "dev".into(),
            engine: "docker".into(),
            agent_sock: None,
            forwards: vec![],
        }],
    }];
    mount_tmux_sections(&mut state);
    for lane in [
        TmuxSystem::host_lane("devbox"),
        TmuxSystem::container_lane("devbox", "dev"),
    ] {
        state.entries.push(crate::state::SessionEntry {
            lane,
            name: "s".to_string(),
            dir: String::new(),
            kind: crate::state::SessionEntryKind::Live { is_current: false },
        });
    }
    state.clamp_projects_focus();

    let built = state.sidebar_layout(ViewMode::Expanded);
    let sessions = state.entries.clone();
    let focus_target = state.focus_target();
    let props = SidebarProps {
        focus_target,
        session_highlight: SessionHighlight::Solid,
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    let (terminal, _) = render_sidebar(32, 24, None, props);

    let buf = terminal.backend().buffer();
    let lines: Vec<String> = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect();
    let row_of = |needle: &str| {
        lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle} not drawn in {lines:#?}"))
    };
    // The focused row is the host's own session, between the `devbox` divider
    // and the container's elbow — exactly the stretch the trunk runs down.
    let host = row_of(" devbox ");
    let container = row_of("└ ▾ dev ");
    // Both of the Expanded row's lines are covered, so the whole stretch
    // between the two dividers is block, not line.
    let focused = host + 1;
    assert!(focused < container, "{lines:#?}");
    for y in focused..container {
        for x in 1..3 {
            let cell = &buf[(x as u16, y as u16)];
            assert_eq!(
                cell.symbol(),
                " ",
                "gutter cell ({x}, {y}) must be blank: {lines:#?}"
            );
            assert_eq!(cell.bg, theme.selection_bg);
        }
    }
    // The elbow the trunk was joining is still drawn: the line is hidden by
    // the block, not dropped from the layout.
    assert_eq!(lines[container].chars().nth(1), Some('└'), "{lines:#?}");
}

#[test]
fn the_tree_line_reaches_a_container_on_the_agents_tab_too() {
    // The Agents tab draws the same tree, and its rows go through an extra
    // transform (the status dot recolor) that rebuilds their spans — the
    // gutter has to survive it, or the line breaks on exactly the rows that
    // have an agent in them.
    use crate::agent::{AgentKind, AgentStatus, DetectedAgent};
    use crate::config::{ContainerConfig, RemoteConfig};
    use crate::state::{AppState, SidebarTab, ViewMode};
    use crate::system::tmux::TmuxSystem;

    let theme = &crate::theme::THEMES[0];
    let keybindings = Keybindings::default();

    let mut state = AppState::new(100, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    state.config_remotes = vec![RemoteConfig {
        host: "devbox".into(),
        forward_agent: true,
        forwards: vec![],
        containers: vec![ContainerConfig {
            name: "dev".into(),
            engine: "docker".into(),
            agent_sock: None,
            forwards: vec![],
        }],
    }];
    mount_tmux_sections(&mut state);
    state.agents.insert(
        TmuxSystem::host_lane("devbox"),
        vec![DetectedAgent {
            kind: AgentKind::Claude,
            session: "work".into(),
            window: "1".into(),
            pane_id: "%1".into(),
            status: AgentStatus::Working,
        }],
    );
    state.rebuild_agent_entries();

    let built = state.agents_layout(ViewMode::Expanded);
    let agent_entries = state.agent_entries.clone();
    let sessions: Vec<crate::state::SessionEntry> = Vec::new();
    let props = SidebarProps {
        sidebar_tab: SidebarTab::Agents,
        agent_entries: &agent_entries,
        focus_target: None,
        ..sidebar_props(&sessions, &built, theme, &keybindings)
    };
    let (terminal, _) = render_sidebar(38, 20, None, props);

    let buf = terminal.backend().buffer();
    let lines: Vec<String> = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect();
    let row_of = |needle: &str| {
        lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle} not drawn in {lines:#?}"))
    };
    let host = row_of(" devbox ");
    let container = row_of("└ ▾ dev ");
    assert!(host < container);
    for row in host + 1..container {
        assert_eq!(
            lines[row].chars().nth(1),
            Some('│'),
            "gap in the trunk at row {row}: {lines:#?}"
        );
    }
}

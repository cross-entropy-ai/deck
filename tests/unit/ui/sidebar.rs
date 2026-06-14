use super::*;

use crate::geometry::plugin_block_rows;

#[test]
fn plugin_block_rows_counts_title_and_separator() {
    // No plugins → no block; N plugins render as title + N rows + trailing
    // separator = N + 2.
    assert_eq!(plugin_block_rows(0), 0);
    assert_eq!(plugin_block_rows(3), 5);
}

#[test]
fn confirm_kill_renders_clickable_in_tabs_mode() {
    // In tabs mode the confirm-kill prompt must render and publish hit
    // regions; otherwise the mouse guard swallows every click while
    // confirm_kill is set and the clickable buttons never work.
    use ratatui::{backend::TestBackend, Terminal};

    let theme = &crate::theme::THEMES[0];
    let built = BuiltLayout::default();
    let keybindings = Keybindings::default();
    let sessions: Vec<&dyn SidebarSession> = Vec::new();

    let backend = TestBackend::new(30, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut kill_hits = None;
    terminal
        .draw(|frame| {
            let area = frame.area();
            let hits = super::draw_sidebar(
                frame,
                area,
                SidebarProps {
                    sessions: &sessions,
                    local_count: 0,
                    built: &built,
                    focus_target: None,
                    sidebar_active: true,
                    theme,
                    show_help: false,
                    confirm_kill: Some("victim"),
                    rename_input: None,
                    show_borders: true,
                    sidebar_tab: crate::state::SidebarTab::Projects,
                    agent_entries: &[],
                    summary: &crate::state::SummaryState::Idle,
                    summary_age: None,
                    spinner_idx: 0,
                    summary_scroll: 0,
                    summary_card_height: 0,
                    tabs_mode: true,
                    plugins: &[],
                    blink_on: false,
                    keybindings: &keybindings,
                    update_available: None,
                },
            );
            kill_hits = hits.kill;
        })
        .unwrap();

    let hits = kill_hits.expect("kill prompt must publish hit regions in tabs mode");
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
        text.contains("Kill victim"),
        "prompt text missing: {text:?}"
    );
}

// --- one geometry, one hit-test ---

use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::geometry::{banner_visible, sidebar_footer_height, SIDEBAR_HEADER_HEIGHT};
use crate::state::{HitKind, HitRegions, SidebarTab, SummaryState};
use crate::update::UpdateStatus;

/// Render the sidebar at `width` x `height` on the Projects tab and return
/// the captured hit registry. `has_update` toggles the footer banner;
/// `plugins` adds plugin rows so the footer block varies.
fn render_hits(
    width: u16,
    height: u16,
    has_update: bool,
    plugins: &[PluginView<'_>],
) -> HitRegions {
    let theme = &crate::theme::THEMES[0];
    let built = BuiltLayout::default();
    let keybindings = Keybindings::default();
    let sessions: Vec<&dyn SidebarSession> = Vec::new();
    let update = UpdateStatus {
        latest_version: "9.9.9".to_string(),
        current_version: "0.0.1".to_string(),
        release_url: "https://example.invalid".to_string(),
        checked_at: 0,
    };

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut captured = HitRegions::default();
    terminal
        .draw(|frame| {
            let area = frame.area();
            captured = super::draw_sidebar(
                frame,
                area,
                SidebarProps {
                    sessions: &sessions,
                    local_count: 0,
                    built: &built,
                    focus_target: None,
                    sidebar_active: true,
                    theme,
                    show_help: false,
                    confirm_kill: None,
                    rename_input: None,
                    show_borders: true,
                    sidebar_tab: SidebarTab::Projects,
                    agent_entries: &[],
                    summary: &SummaryState::Idle,
                    summary_age: None,
                    spinner_idx: 0,
                    summary_scroll: 0,
                    summary_card_height: 0,
                    tabs_mode: false,
                    plugins,
                    blink_on: false,
                    keybindings: &keybindings,
                    update_available: has_update.then_some(&update),
                },
            );
        })
        .unwrap();
    captured
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
        pane: "0".to_string(),
        pane_id: pane_id.to_string(),
        status: crate::agent::AgentStatus::Idle,
    };

    let mut state = AppState::new(80, 24);
    state.prefs.sidebar_tab = SidebarTab::Agents;
    // Two local agents and one remote, so the click→pane mapping has to
    // survive dividers/margins between sections (the "specific pane" path).
    state.entries.push(crate::state::SessionEntry {
        host: Some("h1".to_string()),
        name: "s".to_string(),
        dir: String::new(),
        kind: crate::state::SessionEntryKind::Live { is_current: false },
    });
    state.clamp_projects_focus();
    state
        .agents
        .insert(crate::host_key::HostKey::local(), vec![mk("%7"), mk("%8")]);
    state
        .agents
        .insert(crate::host_key::HostKey::remote("h1"), vec![mk("%9")]);
    state.rebuild_agent_entries();
    let built = state.agents_layout();
    let agent_entries = state.agent_entries.clone();
    let sessions: Vec<&dyn SidebarSession> = Vec::new();

    let backend = TestBackend::new(40, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut captured = HitRegions::default();
    terminal
        .draw(|frame| {
            captured = super::draw_sidebar(
                frame,
                frame.area(),
                SidebarProps {
                    sessions: &sessions,
                    local_count: 0,
                    built: &built,
                    focus_target: state.focus_target(),
                    sidebar_active: true,
                    theme,
                    show_help: false,
                    confirm_kill: None,
                    rename_input: None,
                    show_borders: true,
                    sidebar_tab: SidebarTab::Agents,
                    agent_entries: &agent_entries,
                    summary: &SummaryState::Idle,
                    summary_age: None,
                    spinner_idx: 0,
                    summary_scroll: 0,
                    summary_card_height: state.summary_card_height(),
                    tabs_mode: false,
                    plugins: &[],
                    blink_on: false,
                    keybindings: &keybindings,
                    update_available: None,
                },
            );
        })
        .unwrap();

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
            assert_eq!(t.host.as_deref(), Some("h1"));
        }
        other => panic!("expected SwitchToAgentPane, got {other:?}"),
    }
}

#[test]
fn remote_divider_buttons_register_below_their_top_margin() {
    // A remote `@host` divider carries a 1-row top margin (section spacing),
    // so its bar — and its `[⟳]`/`[…]` buttons — paint one row *below* the
    // header block's top. The hit rects must follow the bar to that row;
    // earlier they were published at the block top (the inert margin row), so
    // the bar's buttons resolved to nothing and a click fell through to
    // collapse the section instead of reconnecting / opening the menu.
    use crate::state::{AppState, DividerButton, SidebarTab, ViewMode};

    let theme = &crate::theme::THEMES[0];
    let keybindings = Keybindings::default();

    let mut state = AppState::new(80, 24);
    state.prefs.sidebar_tab = SidebarTab::Projects;
    // One remote host so the layout has a margined `@h1` divider.
    state.entries.push(crate::state::SessionEntry {
        host: Some("h1".to_string()),
        name: "s".to_string(),
        dir: String::new(),
        kind: crate::state::SessionEntryKind::Live { is_current: false },
    });
    state.clamp_projects_focus();
    let built = state.sidebar_layout(ViewMode::Expanded);
    let sessions: Vec<&dyn SidebarSession> = state
        .entries
        .iter()
        .map(|e| e as &dyn SidebarSession)
        .collect();

    let backend = TestBackend::new(40, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut captured = HitRegions::default();
    terminal
        .draw(|frame| {
            captured = super::draw_sidebar(
                frame,
                frame.area(),
                SidebarProps {
                    sessions: &sessions,
                    local_count: state.local_count(),
                    built: &built,
                    focus_target: state.focus_target(),
                    sidebar_active: true,
                    theme,
                    show_help: false,
                    confirm_kill: None,
                    rename_input: None,
                    show_borders: true,
                    sidebar_tab: SidebarTab::Projects,
                    agent_entries: &[],
                    summary: &SummaryState::Idle,
                    summary_age: None,
                    spinner_idx: 0,
                    summary_scroll: 0,
                    summary_card_height: 0,
                    tabs_mode: false,
                    plugins: &[],
                    blink_on: false,
                    keybindings: &keybindings,
                    update_available: None,
                },
            );
        })
        .unwrap();

    // The remote divider publishes both of its buttons, in order.
    let h1: Vec<&crate::state::DividerHit> = captured
        .dividers
        .iter()
        .filter(|h| h.host == "h1")
        .collect();
    let kinds: Vec<DividerButton> = h1.iter().map(|h| h.kind).collect();
    assert_eq!(
        kinds,
        vec![DividerButton::Reconnect, DividerButton::More],
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
            h.kind,
            h.rect
        );
        assert_eq!(
            buf[pos].symbol(),
            "[",
            "{:?} button rect {:?} must sit on the painted `[icon]`",
            h.kind,
            h.rect
        );
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
    // the footer, for every banner/plugin combination — the same formula
    // the mouse hit-tester uses (`AppState::sidebar_footer_height`), so a
    // click can't land a row off from what was drawn.
    let width = 30;
    let height = 24;
    let content = content_area(width, height);

    let mk_plugin = || PluginView {
        key: 'a',
        name: "demo",
        status: crate::ui::PluginStatus::Background,
    };
    let one = [mk_plugin()];
    for (has_update, plugins) in [
        (false, &[][..]),
        (true, &[][..]),
        (false, &one[..]),
        (true, &one[..]),
    ] {
        let hits = render_hits(width, height, has_update, plugins);
        let banner_shown = banner_visible(has_update, content.width);
        let footer_height = sidebar_footer_height(banner_shown, plugins.len());
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
            "sessions area underflowed (banner {banner_shown}, plugins {})",
            plugins.len()
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
        let hits = render_hits(width, height, true, &[]);

        let mut rects: Vec<Rect> = Vec::new();
        rects.extend(hits.banner);
        rects.extend(hits.menu);
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
    let hits = render_hits(width, height, false, &[]);

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

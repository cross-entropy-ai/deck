use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::geometry::{sidebar_areas, AgentEntry, BuiltLayout, HitRegions, KillConfirmHits};
use crate::keybindings::Keybindings;
use crate::state::{FocusTarget, SessionHighlight, SidebarTab};
use crate::theme::Theme;
use crate::update::UpdateStatus;
use ratatui_textarea::TextArea;

use super::overlays::{draw_confirm_kill, draw_help, draw_rename_input};
use super::widgets::ModalFrame;

mod container;
mod footer;
mod header;
mod sessions;
mod tabs;

use container::draw_sidebar_container;
use footer::{draw_footer, FooterHits, FooterProps};
use header::draw_header;
use sessions::{draw_sessions, SessionsProps};
use summary::{draw_summary_card, SummaryCardProps};
use tabs::{draw_sidebar_tabs, TabsProps};

mod row_style;
mod summary;

/// Inputs to draw the sidebar, grouped into one props object.
///
/// `sessions` is one slice of trait objects: all rows in flat layout order
/// (local first, then remote). The renderer never branches on concrete types —
/// per-row data comes straight off `SessionEntry`.
pub struct SidebarListProps<'a> {
    pub sessions: &'a [crate::state::SessionEntry],
    pub built: &'a BuiltLayout,
    /// Tab-bar label per session, in `sessions` order. Built by the model
    /// (`AppState::tab_labels`) so the bar and its hit-test measure the same
    /// strings.
    pub tab_labels: &'a [String],
    pub focus_target: Option<FocusTarget>,
    /// Active Projects drag as `(source row, current drop target)`.
    pub project_drag: Option<(usize, usize)>,
    /// Which focused-row highlight style the session list paints.
    pub session_highlight: SessionHighlight,
    /// Flattened agent list for the Agents tab (see `AppState::agent_entries`).
    pub agent_entries: &'a [AgentEntry],
}

pub struct SidebarSummaryProps<'a> {
    /// State of the Agents-tab Summary card.
    pub summary: &'a crate::summary_card::SummaryState,
    /// Precomputed "Xm ago" age of the Ready summary, `None` otherwise.
    pub summary_age: Option<&'a str>,
    /// Current braille spinner frame index for the card's generating state.
    pub spinner_idx: usize,
    /// Scroll offset into the Ready summary text.
    pub summary_scroll: usize,
    /// Height of the Summary card strip pinned above the Agents-tab list.
    pub summary_card_height: u16,
}

#[derive(Default)]
pub enum SidebarOverlay<'a> {
    #[default]
    None,
    Help,
    ConfirmKill(&'a str),
    Rename(&'a TextArea<'static>),
}

pub struct SidebarProps<'a> {
    pub list: SidebarListProps<'a>,
    pub summary: SidebarSummaryProps<'a>,
    pub sidebar_active: bool,
    pub theme: &'a Theme,
    pub overlay: SidebarOverlay<'a>,
    pub show_borders: bool,
    pub sidebar_tab: SidebarTab,
    pub tabs_mode: bool,
    pub keybindings: &'a Keybindings,
    pub update_available: Option<&'a UpdateStatus>,
}

/// Re-export the model-owned label so footer/tabs styling stays local while
/// the vertical layout and hit-testing reserve the exact same display width.
pub(super) use crate::geometry::MENU_LABEL;

pub(super) fn menu_span(theme: &Theme) -> Span<'static> {
    Span::styled(
        MENU_LABEL,
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )
}

/// Clamp a captured rect to `area`, `None` if the intersection is empty.
/// Belt-and-suspenders with the per-rect clamps in `header.rs`/`sessions.rs`:
/// published rects can't extend past the sidebar into the PTY pane (bug
/// #16/#17), so a click outside the sidebar can't hit a sidebar button.
fn clamp_rect(rect: Rect, area: Rect) -> Option<Rect> {
    let r = rect.intersection(area);
    (r.width > 0 && r.height > 0).then_some(r)
}

/// Clamp every published rect in `hits` to the sidebar content `area`.
fn clamp_hits(hits: &mut HitRegions, area: Rect) {
    hits.banner = hits.banner.and_then(|r| clamp_rect(r, area));
    hits.menu = hits.menu.and_then(|r| clamp_rect(r, area));
    hits.sidebar_toggle = hits.sidebar_toggle.and_then(|r| clamp_rect(r, area));
    if let Some(tabs) = hits.tabs.as_mut() {
        tabs.projects = clamp_rect(tabs.projects, area).unwrap_or(Rect {
            width: 0,
            ..tabs.projects
        });
        tabs.agents = clamp_rect(tabs.agents, area).unwrap_or(Rect {
            width: 0,
            ..tabs.agents
        });
    }
    hits.summary.button = hits.summary.button.and_then(|r| clamp_rect(r, area));
    hits.summary.popup = hits.summary.popup.and_then(|r| clamp_rect(r, area));
    hits.summary.card = hits.summary.card.and_then(|r| clamp_rect(r, area));
    hits.dividers
        .retain_mut(|h| match clamp_rect(h.rect, area) {
            Some(r) => {
                h.rect = r;
                true
            }
            None => false,
        });
    hits.agents.retain_mut(|h| match clamp_rect(h.rect, area) {
        Some(r) => {
            h.rect = r;
            true
        }
        None => false,
    });
    if let Some(kill) = hits.kill.as_mut() {
        // The kill buttons live inside the sidebar by construction; clamp
        // for symmetry without dropping a button (a zero-width Yes/No would
        // be worse than an over-wide one, and the prompt owns the sidebar).
        if let Some(r) = clamp_rect(kill.yes, area) {
            kill.yes = r;
        }
        if let Some(r) = clamp_rect(kill.no, area) {
            kill.no = r;
        }
    }
}

/// Draw the narrow horizontal rail left behind when the sidebar is collapsed.
/// Keeping a visible, clickable affordance means the sidebar can always be
/// restored without remembering a shortcut.
pub fn draw_collapsed_sidebar(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    show_borders: bool,
) -> HitRegions {
    let content = draw_sidebar_container(frame, area, theme, false, show_borders);
    if content.width == 0 || content.height == 0 {
        return HitRegions::default();
    }
    let rect = Rect {
        x: content.x + content.width.saturating_sub(1) / 2,
        y: content.y,
        width: content.width.min(1),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "›",
            Style::default()
                .fg(theme.accent)
                .bg(theme.bg)
                .add_modifier(Modifier::BOLD),
        ))),
        rect,
    );
    HitRegions {
        sidebar_toggle: Some(rect),
        ..HitRegions::default()
    }
}

/// Draw the rename editor as a centered overlay for the vertical tab layout,
/// where the one-row sidebar cannot contain the normal inline editor.
pub fn draw_rename_popup(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    textarea: &TextArea<'static>,
) {
    let width = area.width.clamp(1, 48);
    let height = area.height.clamp(1, 8);
    let inner = ModalFrame::centered(width, height, Some("Rename session"), theme)
        .render(frame.buffer_mut(), area);
    draw_rename_input(frame, inner, theme, textarea, theme.elevated, false);
}

/// Draw sidebar-only help as a centered modal when the vertical one-row tab
/// bar cannot host it.
pub fn draw_help_popup(frame: &mut Frame, area: Rect, theme: &Theme, keybindings: &Keybindings) {
    let height = (crate::keybindings::Command::ALL.len() as u16 + 7)
        .min(area.height.saturating_sub(2))
        .max(5);
    let inner =
        ModalFrame::centered(64, height, Some("Help"), theme).render(frame.buffer_mut(), area);
    draw_help(frame, inner, theme, keybindings, theme.elevated);
}

/// Draw the destructive close confirmation as a centered modal in vertical
/// layout and return its absolute button hit regions.
pub fn draw_confirm_kill_popup(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    name: &str,
) -> Option<KillConfirmHits> {
    let inner = ModalFrame::warning_centered(48, 9, Some("Close session"), theme)
        .render(frame.buffer_mut(), area);
    draw_confirm_kill(frame, inner, theme, name, theme.elevated)
}

/// Draw the sidebar and return the frame's clickable regions for mouse
/// dispatch.
pub fn draw_sidebar(frame: &mut Frame, area: Rect, props: SidebarProps<'_>) -> HitRegions {
    let content = draw_sidebar_container(
        frame,
        area,
        props.theme,
        props.sidebar_active,
        props.show_borders,
    );

    if props.tabs_mode {
        // Tabs mode shows the unified session list (local rows first,
        // then remotes) so the flat focus index maps straight through —
        // remotes render as `host:session` tabs alongside local ones.
        let focused = props.list.focus_target.map_or(0, |t| t.0);
        let menu_bounds = draw_sidebar_tabs(
            frame,
            content,
            props.theme,
            TabsProps {
                sessions: props.list.sessions,
                labels: props.list.tab_labels,
                focused,
            },
        );
        // A pending kill confirmation must render and stay clickable in tabs
        // mode too: the mouse guard swallows clicks while `confirm_kill` is set,
        // so without drawing it the user faces an invisible modal only y/n can
        // clear. Overlay it on the inner content (covering the tab row).
        let kill_hits = match props.overlay {
            SidebarOverlay::ConfirmKill(name) => {
                draw_confirm_kill(frame, content, props.theme, name, props.theme.bg)
            }
            _ => None,
        };
        // Vertical/tabs layout has no sidebar header (no tab labels), no
        // banner, and no session-area hit regions.
        let mut hits = HitRegions {
            kill: kill_hits,
            menu: menu_bounds,
            ..HitRegions::default()
        };
        clamp_hits(&mut hits, area);
        return hits;
    }
    let areas = sidebar_areas(
        content,
        props.update_available.is_some(),
        props.summary.summary_card_height,
    );

    let project_count = props
        .list
        .sessions
        .iter()
        .filter(|session| session.is_attachable())
        .count();
    let agent_count = props
        .list
        .agent_entries
        .iter()
        .filter(|entry| entry.agent().is_some())
        .count();
    let header_hits = draw_header(
        frame,
        areas.header,
        props.sidebar_tab,
        project_count,
        agent_count,
        props.theme,
    );
    let agents_tab = matches!(props.sidebar_tab, SidebarTab::Agents);
    let mut hits = HitRegions::default();
    match props.overlay {
        SidebarOverlay::Help => {
            draw_help(
                frame,
                areas.body,
                props.theme,
                props.keybindings,
                props.theme.bg,
            );
        }
        SidebarOverlay::ConfirmKill(name) => {
            hits.kill = draw_confirm_kill(frame, areas.body, props.theme, name, props.theme.bg);
        }
        SidebarOverlay::Rename(textarea) => {
            draw_rename_input(
                frame,
                areas.body,
                props.theme,
                textarea,
                props.theme.bg,
                true,
            );
        }
        SidebarOverlay::None => {
            if areas.summary.height > 0 {
                hits.summary = draw_summary_card(
                    frame,
                    areas.summary,
                    props.theme,
                    SummaryCardProps {
                        summary: props.summary.summary,
                        summary_age: props.summary.summary_age,
                        spinner_idx: props.summary.spinner_idx,
                        summary_scroll: props.summary.summary_scroll,
                        can_generate: props
                            .list
                            .agent_entries
                            .iter()
                            .any(|entry| entry.agent().is_some()),
                    },
                );
            }
            (hits.dividers, hits.agents) = draw_sessions(
                frame,
                areas.list,
                props.theme,
                SessionsProps {
                    built: props.list.built,
                    focus_target: props.list.focus_target,
                    sidebar_active: props.sidebar_active,
                    project_drag: props.list.project_drag,
                    agents_tab,
                    agent_entries: props.list.agent_entries,
                    highlight: props.list.session_highlight,
                },
            );
        }
    }
    let FooterHits {
        upgrade: banner_bounds,
        menu: menu_bounds,
    } = draw_footer(
        frame,
        areas.footer,
        props.theme,
        FooterProps {
            update_available: if areas.banner_visible {
                props.update_available
            } else {
                None
            },
            sidebar_active: props.sidebar_active,
            show_borders: props.show_borders,
            sidebar_tab: props.sidebar_tab,
            keybindings: props.keybindings,
        },
    );
    hits.banner = banner_bounds;
    hits.tabs = Some(header_hits.tabs);
    hits.sidebar_toggle = Some(header_hits.sidebar_toggle);
    hits.menu = menu_bounds;
    // Belt-and-suspenders: every rect was captured against its own drawing
    // area, but clamp the whole registry to the sidebar content area so no
    // published rect can ever reach into the PTY pane.
    clamp_hits(&mut hits, content);
    hits
}

#[cfg(test)]
#[path = "../../../tests/unit/ui/sidebar.rs"]
mod tests;

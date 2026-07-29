use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::Frame;

use crate::geometry::{banner_visible, sidebar_footer_height, SIDEBAR_HEADER_HEIGHT};
use crate::keybindings::Keybindings;
use crate::state::{
    AgentEntry, BuiltLayout, FocusTarget, HitRegions, KillConfirmHits, SidebarTab, SummaryHits,
};
use crate::theme::Theme;
use crate::update::UpdateStatus;
use ratatui_textarea::TextArea;

use super::overlays::{draw_confirm_kill, draw_help, draw_rename_input};
use super::SidebarSession;

mod container;
mod footer;
mod header;
mod sessions;
mod tabs;

use container::draw_sidebar_container;
use footer::{draw_footer, FooterHits, FooterProps};
use header::draw_header;
use sessions::{draw_sessions, draw_summary_card, SessionsProps, SummaryCardProps};
use tabs::{draw_sidebar_tabs, TabsProps};

/// Inputs to draw the sidebar, grouped into one props object.
///
/// `sessions` is one slice of trait objects: all rows in flat layout order
/// (local first, then remote). The renderer never branches on concrete types —
/// per-row data goes through `SidebarSession`.
pub struct SidebarProps<'a> {
    pub sessions: &'a [&'a dyn SidebarSession],
    pub built: &'a BuiltLayout,
    pub focus_target: Option<FocusTarget>,
    /// Active Projects drag as `(source row, current drop target)`.
    pub project_drag: Option<(usize, usize)>,
    pub sidebar_active: bool,
    pub theme: &'a Theme,
    pub show_help: bool,
    pub confirm_kill: Option<&'a str>,
    pub rename_input: Option<&'a TextArea<'static>>,
    pub show_borders: bool,
    pub sidebar_tab: SidebarTab,
    /// Flattened agent list for the Agents tab (see `AppState::agent_entries`).
    pub agent_entries: &'a [AgentEntry],
    /// State of the Agents-tab Summary card.
    pub summary: &'a crate::state::SummaryState,
    /// Precomputed "Xm ago" age of the Ready summary, `None` otherwise.
    pub summary_age: Option<&'a str>,
    /// Current braille spinner frame index for the card's generating state.
    pub spinner_idx: usize,
    /// Scroll offset into the Ready summary text.
    pub summary_scroll: usize,
    /// Height of the Summary card strip pinned above the Agents-tab list.
    pub summary_card_height: u16,
    pub tabs_mode: bool,
    pub keybindings: &'a Keybindings,
    pub update_available: Option<&'a UpdateStatus>,
}

#[derive(Clone, Copy)]
struct SidebarRenderCtx<'a> {
    theme: &'a Theme,
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
    hits.new_session = hits.new_session.and_then(|r| clamp_rect(r, area));
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

/// Draw the sidebar and return the frame's clickable regions for mouse
/// dispatch.
pub fn draw_sidebar(frame: &mut Frame, area: Rect, props: SidebarProps<'_>) -> HitRegions {
    let ctx = SidebarRenderCtx { theme: props.theme };

    if props.tabs_mode {
        // Tabs mode shows the unified session list (local rows first,
        // then remotes) so the flat focus index maps straight through —
        // remotes render as `host:session` tabs alongside local ones.
        let focused = props.focus_target.map_or(0, |t| t.0);
        let menu_bounds = draw_sidebar_tabs(
            frame,
            area,
            &ctx,
            TabsProps {
                sessions: props.sessions,
                focused,
                sidebar_active: props.sidebar_active,
                show_borders: props.show_borders,
            },
        );
        // A pending kill confirmation must render and stay clickable in tabs
        // mode too: the mouse guard swallows clicks while `confirm_kill` is set,
        // so without drawing it the user faces an invisible modal only y/n can
        // clear. Overlay it on the inner content (covering the tab row).
        let kill_hits = props.confirm_kill.and_then(|name| {
            let content = draw_sidebar_container(
                frame,
                area,
                props.theme,
                props.sidebar_active,
                props.show_borders,
            );
            draw_confirm_kill(frame, content, props.theme, name)
        });
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
    let content = draw_sidebar_container(
        frame,
        area,
        props.theme,
        props.sidebar_active,
        props.show_borders,
    );

    // Footer geometry shared with `AppState::sidebar_footer_height` so
    // mouse hit-testing can't drift from what is drawn.
    let banner_visible = banner_visible(props.update_available.is_some(), content.width);
    let footer_height = sidebar_footer_height(banner_visible);

    let [header_area, sessions_area, footer_area] = Layout::vertical([
        Constraint::Length(SIDEBAR_HEADER_HEIGHT),
        Constraint::Min(1),
        Constraint::Length(footer_height),
    ])
    .areas(content);

    let project_count = props
        .sessions
        .iter()
        .filter(|session| session.is_attachable())
        .count();
    let agent_count = props
        .agent_entries
        .iter()
        .filter(|entry| entry.agent().is_some())
        .count();
    let header_hits = draw_header(
        frame,
        header_area,
        props.sidebar_tab,
        project_count,
        agent_count,
        props.theme,
    );
    let agents_tab = matches!(props.sidebar_tab, SidebarTab::Agents);
    let mut kill_hits: Option<KillConfirmHits> = None;
    let (divider_hits, agent_hits, summary_hits) = if props.show_help {
        draw_help(frame, sessions_area, props.theme, props.keybindings);
        (Vec::new(), Vec::new(), SummaryHits::default())
    } else if let Some(name) = props.confirm_kill {
        kill_hits = draw_confirm_kill(frame, sessions_area, props.theme, name);
        (Vec::new(), Vec::new(), SummaryHits::default())
    } else if let Some(textarea) = props.rename_input {
        draw_rename_input(frame, sessions_area, props.theme, textarea);
        (Vec::new(), Vec::new(), SummaryHits::default())
    } else {
        // The Summary card is pinned at the bottom of the Agents tab, between
        // list and footer/menu. Carve it off the bottom of the session area;
        // rows above hold the list. The hit-tester (`session_row_hit`) reserves
        // the same strip. `summary_card_height` is 0 off the Agents tab, so the
        // strip is empty there.
        let (summary_strip, list_area) = {
            let h = props.summary_card_height.min(sessions_area.height);
            (
                Rect {
                    y: sessions_area.y + (sessions_area.height - h),
                    height: h,
                    ..sessions_area
                },
                Rect {
                    height: sessions_area.height - h,
                    ..sessions_area
                },
            )
        };
        let summary_hits = if summary_strip.height > 0 {
            draw_summary_card(
                frame,
                summary_strip,
                &ctx,
                SummaryCardProps {
                    summary: props.summary,
                    summary_age: props.summary_age,
                    spinner_idx: props.spinner_idx,
                    summary_scroll: props.summary_scroll,
                    can_generate: props
                        .agent_entries
                        .iter()
                        .any(|entry| entry.agent().is_some()),
                },
            )
        } else {
            SummaryHits::default()
        };
        let (divider_hits, agent_hits) = draw_sessions(
            frame,
            list_area,
            &ctx,
            SessionsProps {
                built: props.built,
                focus_target: props.focus_target,
                project_drag: props.project_drag,
                agents_tab,
                agent_entries: props.agent_entries,
            },
        );
        (divider_hits, agent_hits, summary_hits)
    };
    let FooterHits {
        upgrade: banner_bounds,
        menu: menu_bounds,
    } = draw_footer(
        frame,
        footer_area,
        &ctx,
        FooterProps {
            update_available: if banner_visible {
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
    let mut hits = HitRegions {
        banner: banner_bounds,
        dividers: divider_hits,
        kill: kill_hits,
        agents: agent_hits,
        tabs: Some(header_hits.tabs),
        new_session: header_hits.new_session,
        summary: summary_hits,
        menu: menu_bounds,
    };
    // Belt-and-suspenders: every rect was captured against its own drawing
    // area, but clamp the whole registry to the sidebar content area so no
    // published rect can ever reach into the PTY pane.
    clamp_hits(&mut hits, content);
    hits
}

#[cfg(test)]
#[path = "../../../tests/unit/ui/sidebar.rs"]
mod tests;

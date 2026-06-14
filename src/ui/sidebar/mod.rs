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
use super::{PluginView, SidebarSession};

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

/// Inputs needed to draw the sidebar. Grouping these into one props
/// object keeps the public API readable as the sidebar gains display
/// modes and optional adornments.
///
/// `sessions` is a single slice of trait objects: all rows the sidebar
/// will render, in flat layout order (local first, then remote). The
/// renderer never branches on concrete types — anything per-row goes
/// through `SidebarSession`. `local_count` exists only for callers
/// that need the local-only subset (the header banner, tabs mode).
pub struct SidebarProps<'a> {
    pub sessions: &'a [&'a dyn SidebarSession],
    pub local_count: usize,
    pub built: &'a BuiltLayout,
    pub focus_target: Option<FocusTarget>,
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
    pub plugins: &'a [PluginView<'a>],
    pub blink_on: bool,
    pub keybindings: &'a Keybindings,
    pub update_available: Option<&'a UpdateStatus>,
}

#[derive(Clone, Copy)]
struct SidebarRenderCtx<'a> {
    theme: &'a Theme,
    blink_on: bool,
}

/// The footer/tabs "≡ menu" button label and its accent-bold span. Shared
/// by `footer.rs` and `tabs.rs` so the two can't drift on glyph or style.
pub(super) const MENU_LABEL: &str = "\u{2261} menu";

pub(super) fn menu_span(theme: &Theme) -> Span<'static> {
    Span::styled(
        MENU_LABEL,
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )
}

/// Clamp a captured rect to `area`, returning `None` if the intersection
/// is empty. Belt-and-suspenders with the per-rect clamps in `header.rs`
/// and `sessions.rs`: published rects can never extend past the sidebar
/// into the PTY pane (bug #16/#17), so a click outside the sidebar can't
/// resolve to a sidebar button.
fn clamp_rect(rect: Rect, area: Rect) -> Option<Rect> {
    let r = rect.intersection(area);
    (r.width > 0 && r.height > 0).then_some(r)
}

/// Clamp every published rect in `hits` to the sidebar content `area`.
fn clamp_hits(hits: &mut HitRegions, area: Rect) {
    hits.banner = hits.banner.and_then(|r| clamp_rect(r, area));
    hits.menu = hits.menu.and_then(|r| clamp_rect(r, area));
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
    let ctx = SidebarRenderCtx {
        theme: props.theme,
        blink_on: props.blink_on,
    };

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
        // A pending kill confirmation must render and stay clickable in
        // tabs mode too: the mouse guard swallows every click while
        // `confirm_kill` is set, so without drawing the prompt here the
        // user would face an invisible modal that only y/n could clear.
        // Overlay it on the inner content (covering the tab row), which
        // matches the modal "kill prompt owns the sidebar" behavior.
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
    let footer_height = sidebar_footer_height(banner_visible, props.plugins.len());

    let [header_area, sessions_area, footer_area] = Layout::vertical([
        Constraint::Length(SIDEBAR_HEADER_HEIGHT),
        Constraint::Min(1),
        Constraint::Length(footer_height),
    ])
    .areas(content);

    // The header counts *detected* agents, not entries: an empty section
    // contributes a focusable placeholder entry but no agent to the `(N)` tally.
    let agent_total = props
        .agent_entries
        .iter()
        .filter(|e| e.agent().is_some())
        .count();
    let tab_rects = draw_header(
        frame,
        header_area,
        props.local_count,
        agent_total,
        props.sidebar_tab,
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
        // The Summary card is pinned at the bottom of both tabs, between the
        // list and the footer/menu. Carve it off the bottom of the session
        // area; the rows above hold the sectioned list. The hit-tester
        // (`session_row_hit`) reserves the same strip. The Agents tab
        // summarizes agent panes, Projects sessions.
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
            plugins: props.plugins,
            update_available: if banner_visible {
                props.update_available
            } else {
                None
            },
        },
    );
    let mut hits = HitRegions {
        banner: banner_bounds,
        dividers: divider_hits,
        kill: kill_hits,
        agents: agent_hits,
        tabs: Some(tab_rects),
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

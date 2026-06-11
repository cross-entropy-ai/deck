use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::Frame;

use crate::keybindings::Keybindings;
use crate::layout::{banner_visible, sidebar_footer_height, SIDEBAR_HEADER_HEIGHT};
use crate::state::{
    AgentHit, AgentRow, AgentTarget, DividerHit, FocusTarget, KillConfirmHits, SidebarLayout,
    SidebarTab, ViewMode,
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
pub use header::TabRects;
use sessions::{draw_sessions, SessionsProps};
use tabs::{draw_sidebar_tabs, TabsProps};

#[cfg(test)]
use sessions::render_group_header;

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
    pub layout: &'a SidebarLayout,
    pub focus_target: Option<FocusTarget>,
    pub sidebar_active: bool,
    pub theme: &'a Theme,
    pub show_help: bool,
    pub confirm_kill: Option<&'a str>,
    pub rename_input: Option<&'a TextArea<'static>>,
    pub show_borders: bool,
    pub sidebar_tab: SidebarTab,
    /// Flattened agent list for the Agents tab (see `AppState::agent_rows`).
    pub agent_rows: &'a [AgentRow],
    /// State of the Agents-tab Summary card.
    pub summary: &'a crate::state::SummaryState,
    /// Precomputed "Xm ago" age of the Ready summary, `None` otherwise.
    pub summary_age: Option<&'a str>,
    /// Current braille spinner frame index for the card's generating state.
    pub spinner_idx: usize,
    /// Scroll offset into the Ready summary text.
    pub summary_scroll: usize,
    pub tabs_mode: bool,
    pub view_mode: ViewMode,
    pub plugins: &'a [PluginView<'a>],
    pub blink_on: bool,
    pub keybindings: &'a Keybindings,
    pub update_available: Option<&'a UpdateStatus>,
    /// The agent deck switched to, highlighted in its footer line.
    pub active_agent: Option<&'a AgentTarget>,
}

/// Click/scroll regions the Agents-tab Summary card publishes each frame.
#[derive(Default)]
pub struct SummaryHits {
    /// The "Generate" button, for click hit-testing.
    pub button: Option<Rect>,
    /// The "popup" (big view) button; `None` unless the summary is Ready.
    pub popup: Option<Rect>,
    /// The card's full rect, for routing wheel events to text scrolling.
    pub card: Option<Rect>,
    /// Max scroll offset for the Ready text at this width (0 = no overflow).
    pub max_scroll: usize,
}

/// Every clickable region the sidebar publishes for one frame, captured
/// by the render loop and written back into `AppState` for mouse
/// dispatch. Same pattern as `FooterHits`/`SummaryHits`/`TabRects`, one
/// level up.
#[derive(Default)]
pub struct SidebarHits {
    /// The footer banner's clickable "upgrade" span.
    pub banner: Option<Rect>,
    /// Divider `[⟳]` / `[…]` / pf-badge buttons.
    pub dividers: Vec<DividerHit>,
    /// The kill-confirmation `[No]` / `[Yes]` buttons, while shown.
    pub kill: Option<KillConfirmHits>,
    /// Agent rows in the Agents tab.
    pub agents: Vec<AgentHit>,
    /// The `Projects` / `Agents` header tab labels (`None` in tabs mode,
    /// which has no header).
    pub tabs: Option<TabRects>,
    /// The Summary card's buttons/card/scroll bound.
    pub summary: SummaryHits,
    /// The footer's "menu" button.
    pub menu: Option<Rect>,
}

#[derive(Clone, Copy)]
struct SidebarRenderCtx<'a> {
    theme: &'a Theme,
    blink_on: bool,
}

/// Draw the sidebar and return the frame's clickable regions for mouse
/// dispatch.
pub fn draw_sidebar(frame: &mut Frame, area: Rect, props: SidebarProps<'_>) -> SidebarHits {
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
        return SidebarHits {
            kill: kill_hits,
            menu: menu_bounds,
            ..SidebarHits::default()
        };
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

    let tab_rects = draw_header(
        frame,
        header_area,
        props.local_count,
        props.agent_rows.len(),
        props.sidebar_tab,
        props.theme,
    );
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
        draw_sessions(
            frame,
            sessions_area,
            &ctx,
            SessionsProps {
                sessions: props.sessions,
                layout: props.layout,
                focus_target: props.focus_target,
                view_mode: props.view_mode,
                active_agent: props.active_agent,
                agent_rows: props.agent_rows,
                summary: props.summary,
                summary_age: props.summary_age,
                spinner_idx: props.spinner_idx,
                summary_scroll: props.summary_scroll,
            },
        )
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
    SidebarHits {
        banner: banner_bounds,
        dividers: divider_hits,
        kill: kill_hits,
        agents: agent_hits,
        tabs: Some(tab_rects),
        summary: summary_hits,
        menu: menu_bounds,
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/ui/sidebar.rs"]
mod tests;

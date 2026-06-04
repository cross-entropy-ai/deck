use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::Frame;

use crate::keybindings::Keybindings;
use crate::layout::{plugin_block_rows, BANNER_MIN_WIDTH};
use crate::state::{
    AgentHit, AgentTarget, DividerHit, FocusTarget, KillConfirmHits, SidebarLayout, ViewMode,
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
use footer::{draw_footer, FooterProps};
use header::draw_header;
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
    pub show_agents: bool,
    pub tabs_mode: bool,
    pub view_mode: ViewMode,
    pub plugins: &'a [PluginView<'a>],
    pub blink_on: bool,
    pub keybindings: &'a Keybindings,
    pub update_available: Option<&'a UpdateStatus>,
    /// The agent deck switched to, highlighted in its footer line.
    pub active_agent: Option<&'a AgentTarget>,
}

#[derive(Clone, Copy)]
struct SidebarRenderCtx<'a> {
    theme: &'a Theme,
    blink_on: bool,
    keybindings: &'a Keybindings,
}

// Returns the frame's clickable regions for mouse dispatch: banner
// bounds, divider buttons, kill-prompt buttons, agent footer lines, and
// the "Show Agents" checkbox. A struct would only add ceremony for a
// single internal caller.
#[allow(clippy::type_complexity)]
pub fn draw_sidebar(
    frame: &mut Frame,
    area: Rect,
    props: SidebarProps<'_>,
) -> (
    Option<Rect>,
    Vec<DividerHit>,
    Option<KillConfirmHits>,
    Vec<AgentHit>,
    Option<Rect>,
) {
    let ctx = SidebarRenderCtx {
        theme: props.theme,
        blink_on: props.blink_on,
        keybindings: props.keybindings,
    };

    if props.tabs_mode {
        // Tabs mode shows the unified session list (local rows first,
        // then remotes) so the flat focus index maps straight through —
        // remotes render as `host:session` tabs alongside local ones.
        let focused = props.focus_target.map_or(0, |t| t.0);
        let banner_bounds = draw_sidebar_tabs(
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
        // Tabs mode has no "Projects" header, hence no checkbox.
        return (banner_bounds, Vec::new(), kill_hits, Vec::new(), None);
    }
    let content = draw_sidebar_container(
        frame,
        area,
        props.theme,
        props.sidebar_active,
        props.show_borders,
    );

    let banner_visible = props.update_available.is_some() && content.width >= BANNER_MIN_WIDTH;
    let plugin_rows = plugin_block_rows(props.plugins.len());
    let footer_height: u16 = 3 + banner_visible as u16 + plugin_rows;

    let [header_area, sessions_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(footer_height),
    ])
    .areas(content);

    let agents_checkbox = draw_header(
        frame,
        header_area,
        props.local_count,
        props.show_agents,
        props.theme,
    );
    let mut kill_hits: Option<KillConfirmHits> = None;
    let (divider_hits, agent_hits) = if props.show_help {
        draw_help(frame, sessions_area, props.theme, props.keybindings);
        (Vec::new(), Vec::new())
    } else if let Some(name) = props.confirm_kill {
        kill_hits = draw_confirm_kill(frame, sessions_area, props.theme, name);
        (Vec::new(), Vec::new())
    } else if let Some(textarea) = props.rename_input {
        draw_rename_input(frame, sessions_area, props.theme, textarea);
        (Vec::new(), Vec::new())
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
            },
        )
    };
    let banner_bounds = draw_footer(
        frame,
        footer_area,
        &ctx,
        FooterProps {
            sidebar_active: props.sidebar_active,
            show_help: props.show_help,
            plugins: props.plugins,
            update_available: if banner_visible {
                props.update_available
            } else {
                None
            },
        },
    );
    (
        banner_bounds,
        divider_hits,
        kill_hits,
        agent_hits,
        Some(agents_checkbox),
    )
}

#[cfg(test)]
#[path = "../../../tests/unit/ui/sidebar.rs"]
mod tests;

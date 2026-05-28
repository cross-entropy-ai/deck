use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::keybindings::{Command, Keybindings};
use crate::layout::{
    plugin_block_rows, BANNER_MIN_WIDTH, TAB_INNER_PAD, TAB_LEADING_PAD, TAB_SEPARATOR,
};
use crate::state::{
    scroll_for_layout, DividerHit, FocusTarget, SidebarItem, SidebarItemKind, SidebarLayout,
    ViewMode,
};
use crate::theme::Theme;
use crate::update::UpdateStatus;
use ratatui_textarea::TextArea;

use super::overlays::{draw_confirm_kill, draw_help, draw_rename_input};
use super::text::{
    format_activity_compact, format_idle_badge, idle_color, pack_hint_lines, pad_line,
    primary_key_string, shorten_dir, status_color, status_icon, status_icon_compact, truncate,
};
use super::{PluginStatus, PluginView, SessionActivity, SessionOrigin, SidebarSession};
use crate::state::SessionStatus;

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
    pub tabs_mode: bool,
    pub spinner_frame: &'a str,
    pub view_mode: ViewMode,
    pub plugins: &'a [PluginView<'a>],
    pub blink_on: bool,
    pub keybindings: &'a Keybindings,
    pub update_available: Option<&'a UpdateStatus>,
}

#[derive(Clone, Copy)]
struct SidebarRenderCtx<'a> {
    theme: &'a Theme,
    spinner_frame: &'a str,
    blink_on: bool,
    keybindings: &'a Keybindings,
}

struct SessionsProps<'a> {
    sessions: &'a [&'a dyn SidebarSession],
    layout: &'a SidebarLayout,
    focus_target: Option<FocusTarget>,
    view_mode: ViewMode,
}

#[derive(Clone, Copy)]
struct RowChrome {
    is_focused: bool,
    bg: Color,
    gutter_bg: Color,
    width: usize,
}

struct SessionCardProps<'a> {
    session: &'a dyn SidebarSession,
    /// Flat layout index; mirrors `FocusTarget.0`. The 1-based
    /// keyboard hint shown on local rows is `session_idx + 1`.
    session_idx: usize,
    chrome: RowChrome,
    /// Target row count for this card. The renderer pads the bottom
    /// with blank lines so every card in the same view mode aligns to
    /// the same height.
    target_height: usize,
}

struct PluginRowsProps<'a> {
    plugins: &'a [PluginView<'a>],
    width: usize,
}

struct FooterProps<'a> {
    sidebar_active: bool,
    show_help: bool,
    plugins: &'a [PluginView<'a>],
    update_available: Option<&'a UpdateStatus>,
}

struct TabsProps<'a> {
    sessions: &'a [&'a dyn SidebarSession],
    focused: usize,
    sidebar_active: bool,
    show_borders: bool,
}

pub fn draw_sidebar(
    frame: &mut Frame,
    area: Rect,
    props: SidebarProps<'_>,
) -> (Option<Rect>, Vec<DividerHit>) {
    let ctx = SidebarRenderCtx {
        theme: props.theme,
        spinner_frame: props.spinner_frame,
        blink_on: props.blink_on,
        keybindings: props.keybindings,
    };

    if props.tabs_mode {
        // Tabs mode currently shows only local sessions; map focus
        // back to a plain local index, defaulting to 0 when focus is
        // on a remote row (those just aren't reachable here). Local
        // rows occupy the first `local_count` flat indices.
        let focused_local = match props.focus_target {
            Some(t) if t.0 < props.local_count => t.0,
            _ => 0,
        };
        let local_sessions = &props.sessions[..props.local_count];
        let banner_bounds = draw_sidebar_tabs(
            frame,
            area,
            &ctx,
            TabsProps {
                sessions: local_sessions,
                focused: focused_local,
                sidebar_active: props.sidebar_active,
                show_borders: props.show_borders,
            },
        );
        return (banner_bounds, Vec::new());
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

    draw_header(frame, header_area, props.local_count, props.theme);
    let divider_hits = if props.show_help {
        draw_help(frame, sessions_area, props.theme, props.keybindings);
        Vec::new()
    } else if let Some(name) = props.confirm_kill {
        draw_confirm_kill(frame, sessions_area, props.theme, name);
        Vec::new()
    } else if let Some(textarea) = props.rename_input {
        draw_rename_input(frame, sessions_area, props.theme, textarea);
        Vec::new()
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
    (banner_bounds, divider_hits)
}

fn draw_sidebar_container(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    sidebar_active: bool,
    show_borders: bool,
) -> Rect {
    if show_borders {
        let border_color = if sidebar_active {
            theme.accent
        } else {
            theme.dim
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(theme.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    } else {
        frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);
        area
    }
}

fn draw_header(frame: &mut Frame, area: Rect, count: usize, theme: &Theme) {
    let title = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled("\u{e795}", Style::default().fg(theme.accent)),
        Span::styled(
            " Projects",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" ({})", count), Style::default().fg(theme.dim)),
    ]);
    frame.render_widget(
        Paragraph::new(vec![title, Line::raw("")]).style(Style::default().bg(theme.bg)),
        area,
    );
}

fn draw_sessions(
    frame: &mut Frame,
    area: Rect,
    ctx: &SidebarRenderCtx<'_>,
    props: SessionsProps<'_>,
) -> Vec<DividerHit> {
    if props.sessions.is_empty() {
        frame.render_widget(
            Paragraph::new("  No projects")
                .style(Style::default().fg(ctx.theme.muted).bg(ctx.theme.bg)),
            area,
        );
        return Vec::new();
    }

    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    // Pending hits: (line_index_in_lines, col_range, host). We resolve
    // absolute screen coordinates after computing the scroll offset.
    let mut pending_hits: Vec<(usize, std::ops::Range<usize>, String)> = Vec::new();

    for item in props.layout.items.iter() {
        let is_focused = is_item_focused(item, props.focus_target);
        let row_bg = if is_focused {
            ctx.theme.surface
        } else {
            ctx.theme.bg
        };
        let chrome = RowChrome {
            is_focused,
            bg: row_bg,
            gutter_bg: ctx.theme.bg,
            width,
        };
        match &item.kind {
            SidebarItemKind::Header { label, host_idx } => {
                let accent = host_accent(ctx.theme, *host_idx);
                let line_idx = lines.len();
                let col_range = render_group_header(&mut lines, label, accent, width, ctx.theme);
                // Extract hostname: label is "  @<host>", strip leading
                // whitespace and '@' to recover the bare hostname.
                let host = label.trim_start().trim_start_matches('@').to_string();
                pending_hits.push((line_idx, col_range, host));
            }
            SidebarItemKind::Session { session_idx } => {
                let Some(&session) = props.sessions.get(*session_idx) else {
                    continue;
                };
                let card = SessionCardProps {
                    session,
                    session_idx: *session_idx,
                    chrome,
                    target_height: item.height,
                };
                match props.view_mode {
                    ViewMode::Expanded => render_session_card_expanded(&mut lines, ctx, card),
                    ViewMode::Compact => render_session_card_compact(&mut lines, ctx, card),
                }
            }
        }
    }

    let visible_height = area.height as usize;
    let scroll = scroll_for_layout(props.layout, props.focus_target, visible_height);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(ctx.theme.bg))
            .scroll((scroll as u16, 0)),
        area,
    );

    // Convert pending hits to absolute screen rects now that we know the
    // scroll offset. Lines whose rendered row falls outside the visible
    // area are discarded (they're scrolled off-screen so can't be clicked).
    let mut hits = Vec::with_capacity(pending_hits.len());
    for (line_idx, col_range, host) in pending_hits {
        // line_idx is 0-based within `lines`; subtract scroll to get the
        // rendered row index within the viewport.
        if line_idx < scroll {
            // Scrolled above the visible area.
            continue;
        }
        let viewport_row = line_idx - scroll;
        if viewport_row >= visible_height {
            // Scrolled below the visible area.
            continue;
        }
        let abs_y = area.y + viewport_row as u16;
        let abs_x = area.x + col_range.start as u16;
        let btn_width = (col_range.end - col_range.start) as u16;
        hits.push(DividerHit {
            host,
            rect: Rect {
                x: abs_x,
                y: abs_y,
                width: btn_width,
                height: 1,
            },
        });
    }
    hits
}

fn is_item_focused(item: &SidebarItem, focus_target: Option<FocusTarget>) -> bool {
    match (&item.kind, focus_target) {
        (SidebarItemKind::Session { session_idx }, Some(target)) => *session_idx == target.0,
        _ => false,
    }
}

/// Per-group accent color used to tint the group divider so adjacent
/// remote hosts stay visually distinct without painting whole rows.
/// Accent color cycled per distinct remote host so adjacent groups
/// stay visually distinct without painting whole rows.
fn host_accent(theme: &Theme, host_idx: usize) -> Color {
    let tints = [theme.teal, theme.pink, theme.yellow, theme.accent];
    tints[host_idx % tints.len()]
}

fn render_group_header(
    lines: &mut Vec<Line<'_>>,
    label: &str,
    accent: Color,
    width: usize,
    theme: &Theme,
) -> std::ops::Range<usize> {
    let label_text = label.trim_start().to_string();
    let leading = " ";
    let leading_w = leading.width();
    let label_w = label_text.as_str().width();
    let spacer_w = 1;
    let button_w = 3; // "[…]"
    let button_gap = 1; // space between dashes and button
    let rule_w = width
        .saturating_sub(leading_w)
        .saturating_sub(label_w)
        .saturating_sub(spacer_w)
        .saturating_sub(button_gap)
        .saturating_sub(button_w);
    let rule = "─".repeat(rule_w);

    lines.push(pad_line(
        vec![
            Span::styled(leading, Style::default().bg(theme.bg)),
            Span::styled(
                label_text,
                Style::default()
                    .fg(accent)
                    .bg(theme.bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().bg(theme.bg)),
            Span::styled(rule, Style::default().fg(accent).bg(theme.bg)),
            Span::styled(" ", Style::default().bg(theme.bg)),
            Span::styled("[…]", Style::default().fg(accent).bg(theme.bg)),
        ],
        theme.bg,
        width,
    ));

    // Cell range of "[…]" within this rendered line.
    let button_x = leading_w + label_w + spacer_w + rule_w + button_gap;
    button_x..(button_x + button_w)
}

/// Visual treatment of the index hint at the start of a row. Local
/// rows show a 1-based number (matches the number-key shortcut);
/// remote rows leave the slot blank because the shortcut doesn't reach
/// them.
fn idx_hint(session: &dyn SidebarSession, session_idx: usize) -> String {
    match session.origin() {
        SessionOrigin::Local => format!("{:>2}", session_idx + 1),
        SessionOrigin::Remote { .. } => "  ".to_string(),
    }
}

/// Styled spans for the leading accent bar + name field. Shared
/// between expanded/compact card renderers so the focus/loading/
/// unreachable styling stays in one place.
struct RowHead {
    accent_span: (&'static str, Style),
    name_style: Style,
    index_style: Style,
    /// Resolved label to show in the name slot — usually the session
    /// name, but a `(connecting…)` placeholder for loading remote rows.
    label: String,
}

fn row_head(theme: &Theme, session: &dyn SidebarSession, is_focused: bool, bg: Color) -> RowHead {
    let loading = session.loading();
    let unreachable = session.unreachable();
    let name_fg = if loading || unreachable {
        theme.muted
    } else if is_focused {
        theme.text
    } else {
        theme.secondary
    };
    let mut name_style = Style::default().fg(name_fg).bg(bg);
    if is_focused && !loading {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    RowHead {
        accent_span: (
            if is_focused { "▌" } else { " " },
            Style::default()
                .fg(if is_focused { theme.green } else { bg })
                .bg(bg),
        ),
        name_style,
        index_style: if is_focused {
            Style::default().fg(theme.secondary).bg(bg)
        } else {
            Style::default().fg(theme.dim).bg(bg)
        },
        label: if loading {
            "(connecting…)".to_string()
        } else {
            session.name().to_string()
        },
    }
}

fn render_session_card_expanded(
    lines: &mut Vec<Line<'_>>,
    ctx: &SidebarRenderCtx<'_>,
    props: SessionCardProps<'_>,
) {
    let session = props.session;
    let RowChrome {
        is_focused,
        bg,
        gutter_bg,
        width,
    } = props.chrome;
    let theme = ctx.theme;
    let activity = session.activity();
    let head = row_head(theme, session, is_focused, bg);

    // Status icon slot: real glyph when activity is known, blank space
    // of the same width when the backend doesn't collect activity yet.
    // Keeping the slot reserved avoids name-column reflow between rows
    // that do/don't have activity data.
    let activity_icon = match activity {
        Some(a) => status_icon(
            a.status,
            a.is_current,
            theme,
            ctx.spinner_frame,
            ctx.blink_on,
            is_focused,
            bg,
        ),
        None => Span::styled(" ", Style::default().bg(bg)),
    };

    let text_width = width.saturating_sub(6);
    let name_display = truncate(&head.label, text_width);
    let before = lines.len();
    lines.push(pad_line(
        vec![
            Span::styled(head.accent_span.0, head.accent_span.1),
            activity_icon,
            Span::styled(idx_hint(session, props.session_idx), head.index_style),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(name_display, head.name_style),
        ],
        bg,
        width,
    ));

    // Dir + idle badge line. Skip the dir text for loading rows
    // (placeholder name) and when the backend didn't report one.
    let dir_text = if session.loading() || session.dir().is_empty() {
        String::new()
    } else {
        truncate(&shorten_dir(session.dir()), text_width.saturating_sub(2)).to_string()
    };
    let dir_color = if is_focused { theme.teal } else { theme.muted };
    let (badge_text, badge_color) = match activity {
        Some(a) => {
            let badge = format_idle_badge(a.idle_seconds)
                .map(|t| format!("{t:^6}"))
                .unwrap_or_else(|| " ".repeat(6));
            (badge, idle_color(theme, a.idle_seconds, is_focused))
        }
        None => (" ".repeat(6), theme.dim),
    };
    lines.push(pad_line(
        vec![
            Span::styled(badge_text, Style::default().fg(badge_color).bg(bg)),
            Span::styled(dir_text, Style::default().fg(dir_color).bg(bg)),
        ],
        bg,
        width,
    ));

    // Pad to target_height - 1 with row-bg lines, then a final gutter
    // line. The gutter uses gutter_bg so the separator between groups
    // reads as neutral negative space (callers set gutter_bg = theme.bg
    // when this is the last row of a group, otherwise the group's bg).
    while lines.len() - before < props.target_height.saturating_sub(1) {
        lines.push(pad_line(
            vec![Span::styled(" ", Style::default().bg(bg))],
            bg,
            width,
        ));
    }
    if lines.len() - before < props.target_height {
        lines.push(pad_line(
            vec![Span::styled(" ", Style::default().bg(gutter_bg))],
            gutter_bg,
            width,
        ));
    }
}

/// Compact-mode activity glyph + color. `None` activity returns
/// `(" ", bg)` so the slot stays the same width across rows.
fn compact_activity(
    activity: Option<SessionActivity>,
    theme: &Theme,
    spinner_frame: &str,
    blink_on: bool,
    is_focused: bool,
    bg: Color,
) -> (String, Color) {
    let Some(a) = activity else {
        return (" ".to_string(), bg);
    };
    let text = if a.is_current {
        status_icon_compact(a.status, true, spinner_frame)
    } else {
        match a.status {
            SessionStatus::Working => spinner_frame.to_string(),
            SessionStatus::Idle => {
                if a.idle_seconds < 3 {
                    spinner_frame.to_string()
                } else {
                    "󰒲".to_string()
                }
            }
        }
    };
    let color = if a.is_current {
        status_color(a.status, true, theme, blink_on, is_focused)
    } else {
        match a.status {
            SessionStatus::Idle => idle_color(theme, a.idle_seconds, is_focused),
            _ => status_color(a.status, false, theme, blink_on, is_focused),
        }
    };
    (text, color)
}

/// Compact-mode label: prefix the session name with its origin so the
/// reader can tell local/remote apart on a single line. Falls back to
/// the loading placeholder for not-yet-refreshed remote rows.
fn compact_label(session: &dyn SidebarSession) -> String {
    let prefix = match session.origin() {
        SessionOrigin::Local => "local",
        SessionOrigin::Remote { host } => host,
    };
    if session.loading() {
        format!("{prefix}:(connecting…)")
    } else {
        format!("{prefix}:{}", session.name())
    }
}

fn render_session_card_compact(
    lines: &mut Vec<Line<'_>>,
    ctx: &SidebarRenderCtx<'_>,
    props: SessionCardProps<'_>,
) {
    let session = props.session;
    let RowChrome {
        is_focused,
        bg,
        width,
        ..
    } = props.chrome;
    let theme = ctx.theme;
    let activity = session.activity();
    let head = row_head(theme, session, is_focused, bg);

    let (activity_text, activity_color) = compact_activity(
        activity,
        theme,
        ctx.spinner_frame,
        ctx.blink_on,
        is_focused,
        bg,
    );

    let label = compact_label(session);
    lines.push(pad_line(
        vec![
            Span::styled(head.accent_span.0, head.accent_span.1),
            Span::styled(activity_text, Style::default().fg(activity_color).bg(bg)),
            Span::styled(idx_hint(session, props.session_idx), head.index_style),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(truncate(&label, width.saturating_sub(6)), head.name_style),
        ],
        bg,
        width,
    ));
}

fn plugin_dot_style(status: PluginStatus, blink_on: bool, theme: &Theme) -> Style {
    match status {
        PluginStatus::Foreground => Style::default().fg(theme.green),
        // Strong visibility pulse: bright yellow + bold when on, dim
        // when off. `dim` is defined to be close-to-bg in every theme
        // (both dark and light), so the off-phase reads as "fading
        // out" rather than "turning a different color".
        PluginStatus::Background => {
            if blink_on {
                Style::default()
                    .fg(theme.yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.dim)
            }
        }
        PluginStatus::Inactive => Style::default().fg(theme.dim),
    }
}

fn plugin_dot_glyph(status: PluginStatus) -> &'static str {
    match status {
        PluginStatus::Inactive => "○",
        _ => "●",
    }
}

fn append_plugin_rows(
    rows: &mut Vec<Line<'static>>,
    ctx: &SidebarRenderCtx<'_>,
    props: PluginRowsProps<'_>,
) {
    let theme = ctx.theme;
    let plugins = props.plugins;
    let width = props.width;
    if plugins.is_empty() {
        return;
    }

    rows.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("\u{eb5c}", Style::default().fg(theme.accent)),
        Span::styled(
            " Plugins",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ]));

    for p in plugins {
        let dot_style = plugin_dot_style(p.status, ctx.blink_on, theme);
        let key_color = match p.status {
            PluginStatus::Inactive => theme.dim,
            _ => theme.muted,
        };
        let name_color = match p.status {
            PluginStatus::Foreground => theme.text,
            PluginStatus::Background => theme.secondary,
            PluginStatus::Inactive => theme.muted,
        };
        let name_style = match p.status {
            PluginStatus::Foreground => {
                Style::default().fg(name_color).add_modifier(Modifier::BOLD)
            }
            _ => Style::default().fg(name_color),
        };
        rows.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(plugin_dot_glyph(p.status), dot_style),
            Span::raw(" "),
            Span::styled(p.key.to_string(), Style::default().fg(key_color)),
            Span::raw("  "),
            Span::styled(p.name.to_string(), name_style),
        ]));
    }

    rows.push(Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.dim),
    )));
}

fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    ctx: &SidebarRenderCtx<'_>,
    props: FooterProps<'_>,
) -> Option<Rect> {
    let theme = ctx.theme;
    let keybindings = ctx.keybindings;
    let w = area.width as usize;
    let sep = Line::from(Span::styled("─".repeat(w), Style::default().fg(theme.dim)));

    let hint_lines: Vec<Line> = if props.sidebar_active {
        let nav_key = {
            let next = primary_key_string(keybindings, Command::FocusNext);
            let prev = primary_key_string(keybindings, Command::FocusPrev);
            match (prev.is_empty(), next.is_empty()) {
                (false, false) => format!("{}/{}", next, prev),
                (false, true) => prev,
                (true, false) => next,
                (true, true) => String::new(),
            }
        };
        let mut entries: Vec<(String, String)> = vec![
            (nav_key, "nav".into()),
            (
                primary_key_string(keybindings, Command::OpenSettings),
                "settings".into(),
            ),
            (
                primary_key_string(keybindings, Command::OpenThemePicker),
                "theme".into(),
            ),
            (
                primary_key_string(keybindings, Command::ReloadConfig),
                "reload".into(),
            ),
            (
                primary_key_string(keybindings, Command::ToggleHelp),
                "help".into(),
            ),
            (
                primary_key_string(keybindings, Command::Quit),
                "quit".into(),
            ),
        ];
        entries.retain(|(k, _)| !k.is_empty());
        pack_hint_lines(&entries, w, theme)
    } else {
        let toggle_key = primary_key_string(keybindings, Command::ToggleFocus);
        let label = if toggle_key.is_empty() {
            " sidebar".to_string()
        } else {
            format!(" {} sidebar", toggle_key)
        };
        vec![Line::from(vec![Span::styled(
            label,
            Style::default().fg(theme.subtle),
        )])]
    };

    let rows_capacity =
        usize::from(3 + plugin_block_rows(props.plugins.len()) + props.update_available.is_some() as u16);
    let mut rows: Vec<Line> = Vec::with_capacity(rows_capacity);
    rows.push(sep);

    append_plugin_rows(
        &mut rows,
        ctx,
        PluginRowsProps {
            plugins: props.plugins,
            width: w,
        },
    );

    let mut upgrade_bounds: Option<Rect> = None;
    if let Some(status) = props.update_available {
        let upgrade_label = "upgrade";
        let leading = 1u16;
        let gap = 3u16;
        let upgrade_width = upgrade_label.width() as u16;
        let full = format!(
            "v{} available (current v{})",
            status.latest_version, status.current_version
        );
        let short = format!("v{} available", status.latest_version);
        let tiny = "update available".to_string();
        let chosen = [full, short, tiny]
            .into_iter()
            .find(|text| leading + text.width() as u16 + gap + upgrade_width <= area.width);

        let banner_row_y = area.y + rows.len() as u16;

        if let Some(banner_text) = chosen {
            let text_width = banner_text.width() as u16;
            let upgrade_x = area.x + leading + text_width + gap;
            upgrade_bounds = Some(Rect {
                x: upgrade_x,
                y: banner_row_y,
                width: upgrade_width,
                height: 1,
            });
            rows.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(banner_text, Style::default().fg(theme.dim)),
                Span::raw("   "),
                Span::styled(
                    upgrade_label.to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ),
            ]));
        } else if leading + upgrade_width <= area.width {
            upgrade_bounds = Some(Rect {
                x: area.x + leading,
                y: banner_row_y,
                width: upgrade_width,
                height: 1,
            });
            rows.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    upgrade_label.to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ),
            ]));
        } else {
            rows.push(Line::default());
        }
    }

    let overflow = hint_lines.len() > 1;
    let mut iter = hint_lines.into_iter();
    if let Some(first) = iter.next() {
        rows.push(first);
    } else {
        rows.push(Line::default());
    }

    if overflow {
        rows.push(iter.next().unwrap_or_default());
    } else if props.show_help {
        rows.push(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                format!(
                    "About  {} v{}",
                    env!("CARGO_PKG_NAME"),
                    env!("CARGO_PKG_VERSION")
                ),
                Style::default().fg(theme.dim),
            ),
        ]));
    } else {
        rows.push(Line::default());
    }

    frame.render_widget(
        Paragraph::new(rows).style(Style::default().bg(theme.bg)),
        area,
    );

    upgrade_bounds
}

fn draw_sidebar_tabs(
    frame: &mut Frame,
    area: Rect,
    ctx: &SidebarRenderCtx<'_>,
    props: TabsProps<'_>,
) -> Option<Rect> {
    let theme = ctx.theme;
    let keybindings = ctx.keybindings;
    let sessions = props.sessions;
    let focused = props.focused;
    let content =
        draw_sidebar_container(frame, area, theme, props.sidebar_active, props.show_borders);

    if content.height == 0 {
        return None;
    }

    let tab_area = Rect {
        height: 1,
        ..content
    };
    let leading_pad: String = " ".repeat(TAB_LEADING_PAD as usize);
    let inner_pad: String = " ".repeat(TAB_INNER_PAD as usize);
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(leading_pad, Style::default().bg(theme.bg)));

    for (i, session) in sessions.iter().enumerate() {
        let is_focused = i == focused;

        let bg = if is_focused { theme.surface } else { theme.bg };
        let name_fg = if is_focused {
            theme.green
        } else {
            theme.secondary
        };
        let idx_fg = if is_focused {
            theme.secondary
        } else {
            theme.dim
        };

        spans.push(Span::styled(
            format!("{}", i + 1),
            Style::default().fg(idx_fg).bg(bg),
        ));
        spans.push(Span::styled(inner_pad.clone(), Style::default().bg(bg)));
        spans.push(Span::styled(
            session.name().to_string(),
            Style::default()
                .fg(name_fg)
                .bg(bg)
                .add_modifier(if is_focused {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));
        spans.push(Span::styled(inner_pad.clone(), Style::default().bg(bg)));

        if i + 1 < sessions.len() {
            spans.push(Span::styled(
                TAB_SEPARATOR,
                Style::default().fg(theme.dim).bg(theme.bg),
            ));
        }
    }

    let tabs_width: usize = spans.iter().map(|s| s.width()).sum();
    let width = content.width as usize;
    let hint_pairs: Vec<(String, String)> = if props.sidebar_active {
        vec![
            (
                primary_key_string(keybindings, Command::ToggleHelp),
                " help  ".into(),
            ),
            (
                primary_key_string(keybindings, Command::Quit),
                " quit".into(),
            ),
        ]
    } else {
        vec![(
            primary_key_string(keybindings, Command::ToggleFocus),
            " sidebar".into(),
        )]
    };
    let hint_pairs: Vec<(String, String)> = hint_pairs
        .into_iter()
        .filter(|(k, _)| !k.is_empty())
        .collect();
    let hint_width: usize = hint_pairs.iter().map(|(k, v)| k.len() + v.len()).sum();
    if tabs_width + hint_width + 2 < width {
        let gap = width - tabs_width - hint_width;
        spans.push(Span::styled(" ".repeat(gap), Style::default().bg(theme.bg)));
        for (k, v) in hint_pairs {
            spans.push(Span::styled(
                k,
                Style::default().fg(theme.muted).bg(theme.bg),
            ));
            spans.push(Span::styled(
                v,
                Style::default().fg(theme.subtle).bg(theme.bg),
            ));
        }
    }
    let tab_line = pad_line(spans, theme.bg, width);
    frame.render_widget(
        Paragraph::new(vec![tab_line]).style(Style::default().bg(theme.bg)),
        tab_area,
    );

    if content.height > 1 {
        let detail_area = Rect {
            y: content.y + 1,
            height: content.height - 1,
            ..content
        };

        if let Some(&session) = sessions.get(focused) {
            let avail = content.width as usize;
            let dir = shorten_dir(session.dir());
            // Tabs mode is local-only today, so the activity is always
            // Some(...) for the rows we're given. Fall back to neutral
            // defaults if a non-local row ever slips in.
            let activity = session.activity().unwrap_or(SessionActivity {
                status: SessionStatus::Idle,
                idle_seconds: 0,
                is_current: false,
            });
            let dir_activity = format_activity_compact(activity.idle_seconds, ctx.spinner_frame);
            let status_text =
                status_icon_compact(activity.status, activity.is_current, ctx.spinner_frame);
            let status_color = status_color(
                activity.status,
                activity.is_current,
                theme,
                ctx.blink_on,
                true,
            );

            let mut tail = format!("  {}", dir);
            tail.push_str(&format!("  {}", dir_activity));
            let tail = truncate(&tail, avail.saturating_sub(status_text.width() + 2));

            let detail_line = pad_line(
                vec![
                    Span::styled(
                        format!(" {} ", status_text),
                        Style::default().fg(status_color).bg(theme.bg),
                    ),
                    Span::styled(tail, Style::default().fg(theme.subtle).bg(theme.bg)),
                ],
                theme.bg,
                avail,
            );
            frame.render_widget(
                Paragraph::new(vec![detail_line]).style(Style::default().bg(theme.bg)),
                detail_area,
            );
        }
    }

    None
}

#[cfg(test)]
#[path = "../../tests/unit/ui/sidebar.rs"]
mod tests;

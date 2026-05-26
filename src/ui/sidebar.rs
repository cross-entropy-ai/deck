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
    scroll_for_layout, FocusTarget, GroupKind, SidebarItem, SidebarItemKind, SidebarLayout,
    ViewMode,
};
use crate::theme::Theme;
use crate::update::UpdateStatus;

use super::overlays::{draw_confirm_kill, draw_help, draw_rename_input};
use super::text::{
    build_status_spans, build_tab_status, format_activity_compact, format_git_status,
    format_idle_badge, idle_color, pack_hint_lines, pad_line, primary_key_string, shorten_dir,
    status_color, status_icon, status_icon_compact, truncate,
};
use super::{PluginStatus, PluginView, RemoteSessionView, SessionView};
use crate::state::SessionStatus;

/// Inputs needed to draw the sidebar. Grouping these into one props
/// object keeps the public API readable as the sidebar gains display
/// modes and optional adornments.
pub struct SidebarProps<'a, 'view> {
    pub sessions: &'a [SessionView<'view>],
    pub remote_sessions: &'a [RemoteSessionView<'view>],
    pub layout: &'a SidebarLayout,
    pub focus_target: Option<FocusTarget>,
    pub sidebar_active: bool,
    pub theme: &'a Theme,
    pub show_help: bool,
    pub confirm_kill: Option<&'a str>,
    pub rename_input: Option<(&'a str, usize)>,
    pub show_borders: bool,
    pub tabs_mode: bool,
    pub spinner_frame: &'a str,
    pub view_mode: ViewMode,
    pub plugins: &'a [PluginView<'view>],
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

struct SessionsProps<'a, 'view> {
    sessions: &'a [SessionView<'view>],
    remote_sessions: &'a [RemoteSessionView<'view>],
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

struct LocalCardProps<'a, 'view> {
    session: &'a SessionView<'view>,
    sidebar_idx: usize,
    chrome: RowChrome,
}

struct RemoteRowProps<'a, 'view> {
    row: &'a RemoteSessionView<'view>,
    chrome: RowChrome,
    target_height: usize,
}

struct PluginRowsProps<'a, 'view> {
    plugins: &'a [PluginView<'view>],
    width: usize,
}

struct FooterProps<'a, 'view> {
    sidebar_active: bool,
    show_help: bool,
    plugins: &'a [PluginView<'view>],
    update_available: Option<&'a UpdateStatus>,
}

struct TabsProps<'a, 'view> {
    sessions: &'a [SessionView<'view>],
    focused: usize,
    sidebar_active: bool,
    show_borders: bool,
}

pub fn draw_sidebar(frame: &mut Frame, area: Rect, props: SidebarProps<'_, '_>) -> Option<Rect> {
    let ctx = SidebarRenderCtx {
        theme: props.theme,
        spinner_frame: props.spinner_frame,
        blink_on: props.blink_on,
        keybindings: props.keybindings,
    };

    if props.tabs_mode {
        // Tabs mode currently shows only local sessions; map focus
        // back to a plain local index, defaulting to 0 when focus is
        // on a remote row (those just aren't reachable here).
        let focused_local = match props.focus_target {
            Some(FocusTarget::Local(pos)) => pos,
            _ => 0,
        };
        return draw_sidebar_tabs(
            frame,
            area,
            &ctx,
            TabsProps {
                sessions: props.sessions,
                focused: focused_local,
                sidebar_active: props.sidebar_active,
                show_borders: props.show_borders,
            },
        );
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

    draw_header(frame, header_area, props.sessions.len(), props.theme);
    if props.show_help {
        draw_help(frame, sessions_area, props.theme, props.keybindings);
    } else if let Some(name) = props.confirm_kill {
        draw_confirm_kill(frame, sessions_area, props.theme, name);
    } else if let Some((input, cursor)) = props.rename_input {
        draw_rename_input(frame, sessions_area, props.theme, input, cursor);
    } else {
        draw_sessions(
            frame,
            sessions_area,
            &ctx,
            SessionsProps {
                sessions: props.sessions,
                remote_sessions: props.remote_sessions,
                layout: props.layout,
                focus_target: props.focus_target,
                view_mode: props.view_mode,
            },
        );
    }
    draw_footer(
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
    )
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
    props: SessionsProps<'_, '_>,
) {
    if props.sessions.is_empty() && props.remote_sessions.is_empty() {
        frame.render_widget(
            Paragraph::new("  No projects")
                .style(Style::default().fg(ctx.theme.muted).bg(ctx.theme.bg)),
            area,
        );
        return;
    }

    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    for (i, item) in props.layout.items.iter().enumerate() {
        let is_focused = is_item_focused(item, props.focus_target);
        let group_bg = group_bg(ctx.theme, item.group);
        // Focused rows get a single uniform surface tint so they pop
        // regardless of which group they're in; unfocused rows keep
        // the group's bg.
        let row_bg = if is_focused {
            ctx.theme.surface
        } else {
            group_bg
        };
        // The card's bottom gutter should bleed the group's bg only
        // when another row from the SAME group follows. At the last
        // item of a group (or the very last item), fall back to
        // theme.bg so the separator between groups stays neutral.
        let is_last_in_group = props
            .layout
            .items
            .get(i + 1)
            .map(|next| next.group != item.group)
            .unwrap_or(true);
        let gutter_bg = if is_last_in_group {
            ctx.theme.bg
        } else {
            group_bg
        };
        let chrome = RowChrome {
            is_focused,
            bg: row_bg,
            gutter_bg,
            width,
        };
        match &item.kind {
            SidebarItemKind::Header { label } => {
                render_group_header(&mut lines, label, group_bg, width, ctx.theme);
            }
            SidebarItemKind::LocalSession { filtered_pos } => {
                if let Some(session) = props.sessions.get(*filtered_pos) {
                    let card = LocalCardProps {
                        session,
                        sidebar_idx: *filtered_pos,
                        chrome,
                    };
                    match props.view_mode {
                        ViewMode::Expanded => render_local_card_expanded(&mut lines, ctx, card),
                        ViewMode::Compact => render_local_card_compact(&mut lines, ctx, card),
                    }
                }
            }
            SidebarItemKind::RemoteSession { remote_idx } => {
                if let Some(row) = props.remote_sessions.get(*remote_idx) {
                    render_remote_row(
                        &mut lines,
                        ctx,
                        RemoteRowProps {
                            row,
                            chrome,
                            target_height: item.height,
                        },
                    );
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
}

fn is_item_focused(item: &SidebarItem, focus_target: Option<FocusTarget>) -> bool {
    matches!(
        (&item.kind, focus_target),
        (SidebarItemKind::LocalSession { filtered_pos }, Some(FocusTarget::Local(pos)))
            if *filtered_pos == pos
    ) || matches!(
        (&item.kind, focus_target),
        (SidebarItemKind::RemoteSession { remote_idx }, Some(FocusTarget::Remote(idx)))
            if *remote_idx == idx
    )
}

/// Pick a background color for a group. Local stays on the default
/// theme bg; each remote host gets a subtle tint from a small palette
/// so they read as visually distinct rows without overwhelming the
/// existing card design.
fn group_bg(theme: &Theme, group: GroupKind) -> Color {
    match group {
        GroupKind::Local => theme.bg,
        GroupKind::Remote(idx) => {
            let tints = [theme.teal, theme.pink, theme.yellow, theme.accent];
            let tint = tints[idx % tints.len()];
            blend(theme.bg, tint, 0.10)
        }
    }
}

fn blend(a: Color, b: Color, t: f32) -> Color {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg_g, bb)) = (a, b) else {
        return a;
    };
    let lerp = |x: u8, y: u8| -> u8 {
        ((x as f32) * (1.0 - t) + (y as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::Rgb(lerp(ar, br), lerp(ag, bg_g), lerp(ab, bb))
}

fn render_group_header(
    lines: &mut Vec<Line<'_>>,
    label: &str,
    bg: Color,
    width: usize,
    theme: &Theme,
) {
    lines.push(pad_line(
        vec![Span::styled(
            label.to_string(),
            Style::default()
                .fg(theme.dim)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )],
        bg,
        width,
    ));
}

struct LocalRowBase<'a> {
    accent: &'static str,
    accent_style: Style,
    name_style: Style,
    index_style: Style,
    idx_str: String,
    theme: &'a Theme,
}

fn local_row_base(
    theme: &Theme,
    is_focused: bool,
    bg: Color,
    sidebar_idx: usize,
) -> LocalRowBase<'_> {
    LocalRowBase {
        accent: if is_focused { "▌" } else { " " },
        accent_style: Style::default()
            .fg(if is_focused { theme.green } else { bg })
            .bg(bg),
        name_style: if is_focused {
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.secondary)
        },
        index_style: if is_focused {
            Style::default().fg(theme.secondary)
        } else {
            Style::default().fg(theme.dim)
        },
        idx_str: format!("{:>2}", sidebar_idx + 1),
        theme,
    }
}

fn render_local_card_expanded(
    lines: &mut Vec<Line<'_>>,
    ctx: &SidebarRenderCtx<'_>,
    props: LocalCardProps<'_, '_>,
) {
    let session = props.session;
    let sidebar_idx = props.sidebar_idx;
    let RowChrome {
        is_focused,
        bg,
        gutter_bg,
        width,
    } = props.chrome;
    let base = local_row_base(ctx.theme, is_focused, bg, sidebar_idx);
    let theme = base.theme;

    let activity_icon = status_icon(
        session.status,
        session.is_current,
        theme,
        ctx.spinner_frame,
        ctx.blink_on,
        is_focused,
        bg,
    );
    let text_width = width.saturating_sub(6);
    let name_display = truncate(session.name, text_width);
    lines.push(pad_line(
        vec![
            Span::styled(base.accent, base.accent_style),
            activity_icon,
            Span::styled(base.idx_str, base.index_style.bg(bg)),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(name_display, base.name_style.bg(bg)),
        ],
        bg,
        width,
    ));

    let dir_display = truncate(&shorten_dir(session.dir), text_width.saturating_sub(2));
    let dir_color = if is_focused { theme.teal } else { theme.muted };
    let badge = format_idle_badge(session.idle_seconds)
        .map(|text| format!("{text:^6}"))
        .unwrap_or_else(|| " ".repeat(6));
    lines.push(pad_line(
        vec![
            Span::styled(
                badge,
                Style::default()
                    .fg(idle_color(theme, session.idle_seconds, is_focused))
                    .bg(bg),
            ),
            Span::styled(dir_display, Style::default().fg(dir_color).bg(bg)),
        ],
        bg,
        width,
    ));

    if session.branch.is_empty() {
        lines.push(pad_line(
            vec![
                Span::styled("      ", Style::default().bg(bg)),
                Span::styled(
                    "\u{e725}  no git",
                    Style::default()
                        .fg(if is_focused { theme.dim } else { theme.muted })
                        .bg(bg),
                ),
            ],
            bg,
            width,
        ));
    } else {
        let branch_color = if is_focused { theme.pink } else { theme.muted };
        let branch_display = truncate(session.branch, text_width.saturating_sub(2));
        lines.push(pad_line(
            vec![
                Span::styled("      ", Style::default().bg(bg)),
                Span::styled("\u{e725} ", Style::default().fg(branch_color).bg(bg)),
                Span::styled(branch_display, Style::default().fg(branch_color).bg(bg)),
            ],
            bg,
            width,
        ));
    }

    let status_spans = build_status_spans(session, is_focused, bg, theme, text_width);
    let mut row4 = vec![Span::styled("      ", Style::default().bg(bg))];
    if status_spans.is_empty() {
        row4.push(Span::styled(
            "—",
            Style::default()
                .fg(if is_focused { theme.dim } else { theme.muted })
                .bg(bg),
        ));
    } else {
        row4.extend(status_spans);
    }
    lines.push(pad_line(row4, bg, width));

    // 5th line is the inter-card gutter. The caller decides what
    // color it should be: inside a group it's the group bg so the
    // tint flows continuously between cards; at the last card of a
    // group it's theme.bg so the separator to the next group reads
    // as neutral negative space.
    lines.push(pad_line(
        vec![Span::styled(" ", Style::default().bg(gutter_bg))],
        gutter_bg,
        width,
    ));
}

fn compact_activity(
    session: &SessionView<'_>,
    theme: &Theme,
    spinner_frame: &str,
    blink_on: bool,
    is_focused: bool,
) -> (String, Color) {
    let text = if session.is_current {
        status_icon_compact(session.status, true, spinner_frame)
    } else {
        match session.status {
            SessionStatus::Working => spinner_frame.to_string(),
            SessionStatus::Idle => {
                if session.idle_seconds < 3 {
                    spinner_frame.to_string()
                } else {
                    "󰒲".to_string()
                }
            }
        }
    };

    let color = if session.is_current {
        status_color(session.status, true, theme, blink_on, is_focused)
    } else {
        match session.status {
            SessionStatus::Idle => idle_color(theme, session.idle_seconds, is_focused),
            _ => status_color(session.status, false, theme, blink_on, is_focused),
        }
    };

    (text, color)
}

fn render_local_card_compact(
    lines: &mut Vec<Line<'_>>,
    ctx: &SidebarRenderCtx<'_>,
    props: LocalCardProps<'_, '_>,
) {
    let session = props.session;
    let sidebar_idx = props.sidebar_idx;
    let RowChrome {
        is_focused,
        bg,
        width,
        ..
    } = props.chrome;
    let base = local_row_base(ctx.theme, is_focused, bg, sidebar_idx);
    let theme = base.theme;

    let (activity_text, activity_color) =
        compact_activity(session, theme, ctx.spinner_frame, ctx.blink_on, is_focused);
    let mut spans = vec![
        Span::styled(base.accent, base.accent_style),
        Span::styled(activity_text, Style::default().fg(activity_color).bg(bg)),
        Span::styled(base.idx_str, base.index_style.bg(bg)),
        Span::styled("  ", Style::default().bg(bg)),
        Span::styled(
            truncate(session.name, width.saturating_sub(6)),
            base.name_style.bg(bg),
        ),
    ];

    if !session.branch.is_empty() {
        let branch_color = if is_focused { theme.pink } else { theme.muted };
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        spans.push(Span::styled(
            truncate(session.branch, width.saturating_sub(20)),
            Style::default().fg(branch_color).bg(bg),
        ));

        let status = format_git_status(session, true);
        if !status.is_empty() {
            let status_color = if status == "✓" {
                if is_focused {
                    theme.green
                } else {
                    theme.muted
                }
            } else if is_focused {
                theme.yellow
            } else {
                theme.dim
            };
            spans.push(Span::styled(" ", Style::default().bg(bg)));
            spans.push(Span::styled(
                status,
                Style::default().fg(status_color).bg(bg),
            ));
        }
    }

    lines.push(pad_line(spans, bg, width));

    let text_width = width.saturating_sub(6);
    let dir_display = truncate(&shorten_dir(session.dir), text_width);
    let dir_color = if is_focused { theme.teal } else { theme.muted };
    lines.push(pad_line(
        vec![
            Span::styled("      ", Style::default().bg(bg)),
            Span::styled(dir_display, Style::default().fg(dir_color).bg(bg)),
        ],
        bg,
        width,
    ));
}

fn render_remote_row(
    lines: &mut Vec<Line<'_>>,
    ctx: &SidebarRenderCtx<'_>,
    props: RemoteRowProps<'_, '_>,
) {
    let theme = ctx.theme;
    let row = props.row;
    let target_height = props.target_height;
    let RowChrome {
        is_focused,
        bg,
        gutter_bg,
        width,
    } = props.chrome;
    let accent_color = if is_focused { theme.green } else { bg };
    let accent = if is_focused { "▌" } else { " " };
    let name_fg = if row.loading || row.unreachable {
        theme.muted
    } else if is_focused {
        theme.text
    } else {
        theme.secondary
    };
    let mut name_style = Style::default().fg(name_fg).bg(bg);
    if is_focused && !row.loading {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    let label_text = if row.loading {
        "  (connecting…)".to_string()
    } else {
        format!("  {}", row.name)
    };
    let before = lines.len();
    lines.push(pad_line(
        vec![
            Span::styled(accent, Style::default().fg(accent_color).bg(bg)),
            Span::styled(truncate(&label_text, width.saturating_sub(1)), name_style),
        ],
        bg,
        width,
    ));
    if !row.dir.is_empty() && !row.loading {
        lines.push(pad_line(
            vec![Span::styled(
                truncate(
                    &format!("    {}", shorten_dir(row.dir)),
                    width.saturating_sub(1),
                ),
                Style::default().fg(theme.muted).bg(bg),
            )],
            bg,
            width,
        ));
    }
    // Pad to the target height so each remote row occupies the same
    // vertical space as a local card. Intermediate rows use the row
    // bg (group tint, or surface when focused) so the focused card's
    // highlight is solid; the LAST row uses the group bg so the
    // group's continuous background extends across the gutter
    // between adjacent remote cards (matching the local-card behavior
    // where the gutter sits on the group's bg).
    while lines.len() - before < target_height.saturating_sub(1) {
        lines.push(pad_line(
            vec![Span::styled(" ", Style::default().bg(bg))],
            bg,
            width,
        ));
    }
    if lines.len() - before < target_height {
        lines.push(pad_line(
            vec![Span::styled(" ", Style::default().bg(gutter_bg))],
            gutter_bg,
            width,
        ));
    }
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
    props: PluginRowsProps<'_, '_>,
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
    props: FooterProps<'_, '_>,
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
    props: TabsProps<'_, '_>,
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
            session.name,
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

        if let Some(session) = sessions.get(focused) {
            let avail = content.width as usize;
            let dir = shorten_dir(session.dir);
            let git = build_tab_status(session);
            let activity = format_activity_compact(session.idle_seconds, ctx.spinner_frame);
            let status_text =
                status_icon_compact(session.status, session.is_current, ctx.spinner_frame);
            let status_color = status_color(
                session.status,
                session.is_current,
                theme,
                ctx.blink_on,
                true,
            );

            let mut tail = format!("  {}", dir);
            if !session.branch.is_empty() {
                tail.push_str(&format!("  {}", session.branch));
            }
            if !git.is_empty() {
                tail.push_str(&format!("  {}", git));
            }
            tail.push_str(&format!("  {}", activity));
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

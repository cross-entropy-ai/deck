use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::keybindings::{Command, Keybindings};
use crate::layout::{
    card_height, plugin_block_rows, BANNER_MIN_WIDTH, TAB_INNER_PAD, TAB_LEADING_PAD, TAB_SEPARATOR,
};
use crate::state::ViewMode;
use crate::theme::Theme;
use crate::update::UpdateStatus;

use super::overlays::{draw_confirm_kill, draw_help, draw_rename_input};
use super::text::{
    build_status_spans, build_tab_status, format_activity_compact, format_git_status,
    format_idle_badge, idle_color, pack_hint_lines, pad_line, primary_key_string, scroll_offset,
    shorten_dir, status_color, status_icon, status_icon_compact, truncate,
};
use super::{PluginStatus, PluginView, SessionView};
use crate::state::SessionStatus;

#[allow(clippy::too_many_arguments)]
pub fn draw_sidebar(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    sidebar_active: bool,
    theme: &Theme,
    show_help: bool,
    confirm_kill: Option<&str>,
    rename_input: Option<(&str, usize)>,
    show_borders: bool,
    tabs_mode: bool,
    spinner_frame: &str,
    view_mode: ViewMode,
    plugins: &[PluginView],
    blink_on: bool,
    keybindings: &Keybindings,
    update_available: Option<&UpdateStatus>,
) -> Option<Rect> {
    if tabs_mode {
        return draw_sidebar_tabs(
            frame,
            area,
            sessions,
            focused,
            sidebar_active,
            theme,
            show_borders,
            spinner_frame,
            blink_on,
            keybindings,
            update_available,
        );
    }
    let content = if show_borders {
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
        let c = block.inner(area);
        frame.render_widget(block, area);
        c
    } else {
        frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);
        area
    };

    let banner_visible = update_available.is_some() && content.width >= BANNER_MIN_WIDTH;
    let plugin_rows = plugin_block_rows(plugins.len());
    let footer_height: u16 = 3 + banner_visible as u16 + plugin_rows;

    let [header_area, sessions_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(footer_height),
    ])
    .areas(content);

    draw_header(frame, header_area, sessions.len(), theme);
    if show_help {
        draw_help(frame, sessions_area, theme, keybindings);
    } else if let Some(name) = confirm_kill {
        draw_confirm_kill(frame, sessions_area, theme, name);
    } else if let Some((input, cursor)) = rename_input {
        draw_rename_input(frame, sessions_area, theme, input, cursor);
    } else {
        draw_sessions(
            frame,
            sessions_area,
            sessions,
            focused,
            spinner_frame,
            blink_on,
            theme,
            view_mode,
        );
    }
    draw_footer(
        frame,
        footer_area,
        sidebar_active,
        theme,
        footer_area.width,
        show_help,
        plugins,
        blink_on,
        keybindings,
        if banner_visible {
            update_available
        } else {
            None
        },
    )
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

#[allow(clippy::too_many_arguments)]
fn draw_sessions(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    spinner_frame: &str,
    blink_on: bool,
    theme: &Theme,
    view_mode: ViewMode,
) {
    if sessions.is_empty() {
        frame.render_widget(
            Paragraph::new("  No projects").style(Style::default().fg(theme.muted).bg(theme.bg)),
            area,
        );
        return;
    }

    match view_mode {
        ViewMode::Expanded => {
            let width = area.width as usize;
            let mut lines: Vec<Line> = Vec::new();

            for (i, session) in sessions.iter().enumerate() {
                let is_focused = i == focused;
                let is_emphasized = is_focused;

                let accent_color = if is_focused { theme.green } else { theme.bg };
                let accent = if is_focused { "▌" } else { " " };
                let name_style = if is_focused {
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.secondary)
                };
                let index_style = if is_focused {
                    Style::default().fg(theme.secondary)
                } else {
                    Style::default().fg(theme.dim)
                };
                let bg = if is_focused { theme.surface } else { theme.bg };

                let activity_icon = status_icon(
                    session.status,
                    session.is_current,
                    theme,
                    spinner_frame,
                    blink_on,
                    is_emphasized,
                    bg,
                );
                let idx_str = format!("{:>2}", i + 1);
                let text_width = width.saturating_sub(6);
                let name_display = truncate(session.name, text_width);
                lines.push(pad_line(
                    vec![
                        Span::styled(accent, Style::default().fg(accent_color).bg(bg)),
                        activity_icon,
                        Span::styled(idx_str, index_style.bg(bg)),
                        Span::styled("  ", Style::default().bg(bg)),
                        Span::styled(name_display, name_style.bg(bg)),
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
                                .fg(idle_color(theme, session.idle_seconds, is_emphasized))
                                .bg(bg),
                        ),
                        Span::styled("", Style::default().fg(dir_color).bg(bg)),
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
                                    .fg(if is_emphasized {
                                        theme.dim
                                    } else {
                                        theme.muted
                                    })
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

                let status_spans =
                    build_status_spans(session, is_emphasized, bg, theme, text_width);
                let mut row4 = vec![Span::styled("      ", Style::default().bg(bg))];
                if status_spans.is_empty() {
                    row4.push(Span::styled(
                        "—",
                        Style::default()
                            .fg(if is_emphasized {
                                theme.dim
                            } else {
                                theme.muted
                            })
                            .bg(bg),
                    ));
                } else {
                    row4.extend(status_spans);
                }
                lines.push(pad_line(row4, bg, width));

                lines.push(Line::from(Span::styled(" ", Style::default().bg(theme.bg))));
            }

            let scroll = scroll_offset(focused, area.height, card_height(view_mode));
            frame.render_widget(
                Paragraph::new(lines)
                    .style(Style::default().bg(theme.bg))
                    .scroll((scroll as u16, 0)),
                area,
            );
        }
        ViewMode::Compact => {
            draw_sessions_compact(frame, area, sessions, focused, spinner_frame, blink_on, theme);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_sessions_compact(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    spinner_frame: &str,
    blink_on: bool,
    theme: &Theme,
) {
    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    for (i, session) in sessions.iter().enumerate() {
        let is_focused = i == focused;
        let is_emphasized = is_focused;

        let accent_color = if is_focused { theme.green } else { theme.bg };
        let accent = if is_focused { "▌" } else { " " };
        let name_style = if is_focused {
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.secondary)
        };
        let index_style = if is_focused {
            Style::default().fg(theme.secondary)
        } else {
            Style::default().fg(theme.dim)
        };
        let bg = if is_focused { theme.surface } else { theme.bg };

        // For Working sessions we keep the spinner; for Idle we show
        // the compact "time since last activity" badge (e.g. "2m");
        // for Waiting we surface the bell glyph + blink color so it
        // stands out even in the cramped compact layout.
        let activity_text = if session.is_current {
            status_icon_compact(session.status, true, spinner_frame)
        } else {
            match session.status {
                SessionStatus::Working => spinner_frame.to_string(),
                SessionStatus::Waiting => {
                    status_icon_compact(session.status, false, spinner_frame)
                }
                SessionStatus::Idle => {
                    format_activity_compact(session.idle_seconds, spinner_frame)
                }
            }
        };
        let activity_color = if session.is_current {
            status_color(session.status, true, theme, blink_on, is_emphasized)
        } else {
            match session.status {
                SessionStatus::Idle => idle_color(theme, session.idle_seconds, is_emphasized),
                _ => status_color(session.status, false, theme, blink_on, is_emphasized),
            }
        };
        let idx_str = format!("{:>2}", i + 1);

        let mut spans = vec![
            Span::styled(accent, Style::default().fg(accent_color).bg(bg)),
            Span::styled(activity_text, Style::default().fg(activity_color).bg(bg)),
            Span::styled(idx_str, index_style.bg(bg)),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(
                truncate(session.name, width.saturating_sub(6)),
                name_style.bg(bg),
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
                    if is_emphasized {
                        theme.green
                    } else {
                        theme.muted
                    }
                } else if is_emphasized {
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

    let scroll = scroll_offset(focused, area.height, card_height(ViewMode::Compact));
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(theme.bg))
            .scroll((scroll as u16, 0)),
        area,
    );
}

fn plugin_dot_color(status: PluginStatus, blink_on: bool, theme: &Theme) -> ratatui::style::Color {
    match status {
        PluginStatus::Foreground => theme.green,
        // Alternates at 1 Hz between yellow and subtle so the pulse is
        // visible against the sidebar bg without changing the glyph.
        PluginStatus::Background => {
            if blink_on {
                theme.yellow
            } else {
                theme.subtle
            }
        }
        PluginStatus::Inactive => theme.dim,
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
    plugins: &[PluginView],
    blink_on: bool,
    width: usize,
    theme: &Theme,
) {
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
        let dot_color = plugin_dot_color(p.status, blink_on, theme);
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
            PluginStatus::Foreground => Style::default().fg(name_color).add_modifier(Modifier::BOLD),
            _ => Style::default().fg(name_color),
        };
        rows.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(plugin_dot_glyph(p.status), Style::default().fg(dot_color)),
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

#[allow(clippy::too_many_arguments)]
fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    sidebar_active: bool,
    theme: &Theme,
    width: u16,
    show_help: bool,
    plugins: &[PluginView],
    blink_on: bool,
    keybindings: &Keybindings,
    update_available: Option<&UpdateStatus>,
) -> Option<Rect> {
    let w = width as usize;
    let sep = Line::from(Span::styled("─".repeat(w), Style::default().fg(theme.dim)));

    let hint_lines: Vec<Line> = if sidebar_active {
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

    let mut rows: Vec<Line> = Vec::with_capacity(5);
    rows.push(sep);

    append_plugin_rows(&mut rows, plugins, blink_on, w, theme);

    let mut upgrade_bounds: Option<Rect> = None;
    if let Some(status) = update_available {
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
    } else if show_help {
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

#[allow(clippy::too_many_arguments)]
fn draw_sidebar_tabs(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    sidebar_active: bool,
    theme: &Theme,
    show_borders: bool,
    spinner_frame: &str,
    blink_on: bool,
    keybindings: &Keybindings,
    _update_available: Option<&UpdateStatus>,
) -> Option<Rect> {
    let content = if show_borders {
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
        let c = block.inner(area);
        frame.render_widget(block, area);
        c
    } else {
        frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);
        area
    };

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
    let hint_pairs: Vec<(String, String)> = if sidebar_active {
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
            let activity = format_activity_compact(session.idle_seconds, spinner_frame);
            let status_text =
                status_icon_compact(session.status, session.is_current, spinner_frame);
            let status_color =
                status_color(session.status, session.is_current, theme, blink_on, true);

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

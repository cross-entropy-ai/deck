use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::keybindings::{format_key, Command, Keybindings};
use crate::layout::{
    card_height, context_menu_width, TAB_INNER_PAD, TAB_LEADING_PAD, TAB_SEPARATOR,
};
use crate::state::{FilterMode, LayoutMode, ViewMode, FILTER_TABS};
use crate::theme::Theme;
use crate::update::UpdateStatus;

/// Minimum content width to allocate a banner row at all. The very last
/// fallback just shows " upgrade" (8 cols).
const BANNER_MIN_WIDTH: u16 = 8;

/// Minimal data needed to render one session row.
pub struct SessionView<'a> {
    pub name: &'a str,
    pub dir: &'a str,
    pub branch: &'a str,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub idle_seconds: u64,
}

pub struct ExcludeEditorView<'a> {
    pub patterns: &'a [String],
    pub selected: usize,
    pub adding: bool,
    pub input: &'a str,
    pub error: Option<&'a str>,
}

pub struct SettingsView<'a> {
    pub selected: usize,
    pub focus_main: bool,
    pub theme_name: &'a str,
    pub theme_picker_open: bool,
    pub theme_picker_selected: usize,
    pub theme_names: Vec<&'a str>,
    pub layout_mode: LayoutMode,
    pub show_borders: bool,
    pub view_mode: ViewMode,
    pub exclude_count: usize,
    pub exclude_editor: Option<ExcludeEditorView<'a>>,
    pub keybindings: &'a Keybindings,
    pub keybindings_view_open: bool,
    pub keybindings_view_scroll: u16,
    pub update_check_enabled: bool,
    pub update_check_help: String,
}

/// Draw the sidebar into the given area. Returns the column range (as a Rect)
/// occupied by the clickable "upgrade" banner span if one was drawn — used by
/// the caller to record the hit-test region on AppState for mouse routing.
#[allow(clippy::too_many_arguments)]
pub fn draw_sidebar(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    sidebar_active: bool,
    theme: &Theme,
    filter_mode: FilterMode,
    show_help: bool,
    confirm_kill: Option<&str>,
    rename_input: Option<(&str, usize)>,
    show_borders: bool,
    tabs_mode: bool,
    spinner_frame: &str,
    view_mode: ViewMode,
    plugins: &[(char, &str)],
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

    // Banner is information, not a hint — surface it regardless of focus so
    // users working in the main pane still see it. Narrow sidebars skip the
    // banner (we don't want to truncate a critical message mid-word).
    let banner_visible = update_available.is_some() && content.width >= BANNER_MIN_WIDTH;
    let footer_height: u16 = if banner_visible { 4 } else { 3 };

    let [header_area, sessions_area, footer_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(footer_height),
    ])
    .areas(content);

    draw_header(frame, header_area, sessions.len(), theme, filter_mode);
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
        keybindings,
        if banner_visible { update_available } else { None },
    )
}

fn draw_header(
    frame: &mut Frame,
    area: Rect,
    count: usize,
    theme: &Theme,
    filter_mode: FilterMode,
) {
    let title = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled("\u{e795}", Style::default().fg(theme.accent)),
        Span::styled(
            " Projects",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" ({})", count), Style::default().fg(theme.dim)),
    ]);
    let mut tabs: Vec<Span> = vec![Span::styled(" ", Style::default())];
    for (idx, mode) in FILTER_TABS.iter().enumerate() {
        let active = *mode == filter_mode;
        let style = if active {
            Style::default()
                .fg(theme.bg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        tabs.push(Span::styled(format!(" {} ", mode.tab_label()), style));
        if idx + 1 < FILTER_TABS.len() {
            tabs.push(Span::styled(" ", Style::default()));
        }
    }
    frame.render_widget(
        Paragraph::new(vec![title, Line::from(tabs), Line::raw("")])
            .style(Style::default().bg(theme.bg)),
        area,
    );
}

fn pad_line(spans: Vec<Span>, bg: ratatui::style::Color, width: usize) -> Line {
    let mut line = Line::from(spans);
    let content_width = line.width();
    if content_width < width {
        line.spans.push(Span::styled(
            " ".repeat(width - content_width),
            Style::default().bg(bg),
        ));
    }
    line
}

fn draw_sessions(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    spinner_frame: &str,
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

                // Row 1: accent + activity icon + index + name
                let activity_icon = if session.idle_seconds < 3 {
                    Span::styled(spinner_frame, Style::default().fg(theme.green).bg(bg))
                } else {
                    Span::styled(
                        "󰒲",
                        Style::default()
                            .fg(if is_emphasized {
                                theme.dim
                            } else {
                                theme.muted
                            })
                            .bg(bg),
                    )
                };
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

                // Row 2: idle badge + directory
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

                // Row 3: branch (always rendered to keep card height consistent)
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

                // Row 4: status indicators (always rendered)
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
            draw_sessions_compact(frame, area, sessions, focused, spinner_frame, theme);
        }
    }
}

fn draw_sessions_compact(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    spinner_frame: &str,
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

        // Row 1: accent + activity + index + name + branch + git status
        let activity_text = format_activity_compact(session.idle_seconds, spinner_frame);
        let activity_color = idle_color(theme, session.idle_seconds, is_emphasized);
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

        // Row 2: directory
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

#[allow(clippy::too_many_arguments)]
fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    sidebar_active: bool,
    theme: &Theme,
    width: u16,
    show_help: bool,
    plugins: &[(char, &str)],
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
            (primary_key_string(keybindings, Command::OpenSettings), "settings".into()),
            (primary_key_string(keybindings, Command::OpenThemePicker), "theme".into()),
            (primary_key_string(keybindings, Command::ToggleHelp), "help".into()),
            (primary_key_string(keybindings, Command::Quit), "quit".into()),
        ];
        for &(key, name) in plugins {
            entries.push((key.to_string(), name.to_string()));
        }
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

    // Row layout inside the footer area:
    //   row 0: separator
    //   row 1: banner (if update available)
    //   rows 1-2 (no banner) or 2-3 (banner): hints line 1, then hints line 2 OR About
    let mut rows: Vec<Line> = Vec::with_capacity(4);
    rows.push(sep);

    // Banner row (optional). Tracks the "upgrade" span bounds for hit-testing.
    let mut upgrade_bounds: Option<Rect> = None;
    if let Some(status) = update_available {
        let upgrade_label = "upgrade";
        let leading = 1u16;
        let gap = 3u16;
        let upgrade_width = upgrade_label.width() as u16;
        // Prefer the most informative banner text that still fits. Fall back
        // through progressively shorter forms so even a narrow sidebar shows
        // something — the "upgrade" button is the critical element.
        let full = format!(
            "v{} available (current v{})",
            status.latest_version, status.current_version
        );
        let short = format!("v{} available", status.latest_version);
        let tiny = "update available".to_string();
        let chosen = [full, short, tiny].into_iter().find(|text| {
            leading + text.width() as u16 + gap + upgrade_width <= area.width
        });

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
            // Last resort: drop the text, keep only the clickable button so
            // the user still has a way to trigger the upgrade in a tiny
            // sidebar.
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
            // Even `upgrade` alone won't fit — give up, render blank.
            rows.push(Line::default());
        }
    }

    // Hints line 1
    let overflow = hint_lines.len() > 1;
    let mut iter = hint_lines.into_iter();
    if let Some(first) = iter.next() {
        rows.push(first);
    } else {
        rows.push(Line::default());
    }

    // Final row: hints line 2, About, or blank — priority in that order.
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

fn pack_hint_lines(entries: &[(String, String)], width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let sep_width = 2;
    let leading = 1;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut cur_width = leading;

    for (key, label) in entries {
        let entry_width = key.width() + 1 + label.width();
        let has_content = spans.len() > 1;
        let needed = if has_content { sep_width + entry_width } else { entry_width };

        // Wrap to a new line if this entry won't fit (and the current line has
        // at least one entry already — a single entry that's wider than width
        // still gets placed rather than dropped).
        if has_content && cur_width + needed > width {
            lines.push(Line::from(std::mem::replace(
                &mut spans,
                vec![Span::raw(" ")],
            )));
            cur_width = leading;
        }

        if spans.len() > 1 {
            spans.push(Span::raw("  "));
            cur_width += sep_width;
        }
        spans.push(Span::styled(key.clone(), Style::default().fg(theme.muted)));
        spans.push(Span::styled(
            format!(" {}", label),
            Style::default().fg(theme.subtle),
        ));
        cur_width += entry_width;
    }

    if spans.len() > 1 {
        lines.push(Line::from(spans));
    }

    lines
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
    keybindings: &Keybindings,
    _update_available: Option<&UpdateStatus>,
) -> Option<Rect> {
    // Vertical/tabs mode intentionally skips the update banner — the layout
    // is too compact to carry it. Users in vertical mode see updates via
    // the Settings "Update check" row or by switching to horizontal.
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

    // Row 1: tab bar
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

        // Separator between tabs
        if i + 1 < sessions.len() {
            spans.push(Span::styled(
                TAB_SEPARATOR,
                Style::default().fg(theme.dim).bg(theme.bg),
            ));
        }
    }

    // Right-align hints in the tab bar
    let tabs_width: usize = spans.iter().map(|s| s.width()).sum();
    let width = content.width as usize;
    let hint_pairs: Vec<(String, String)> = if sidebar_active {
        vec![
            (primary_key_string(keybindings, Command::ToggleHelp), " help  ".into()),
            (primary_key_string(keybindings, Command::Quit), " quit".into()),
        ]
    } else {
        vec![(primary_key_string(keybindings, Command::ToggleFocus), " sidebar".into())]
    };
    let hint_pairs: Vec<(String, String)> =
        hint_pairs.into_iter().filter(|(k, _)| !k.is_empty()).collect();
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

    // Row 2+: details of focused session
    if content.height > 1 {
        let detail_area = Rect {
            y: content.y + 1,
            height: content.height - 1,
            ..content
        };

        if let Some(session) = sessions.get(focused) {
            let avail = content.width as usize;

            // Build detail segments: (text, color)
            let dir = shorten_dir(session.dir);
            let status = build_tab_status(session);
            let activity = format_activity_compact(session.idle_seconds, spinner_frame);

            // Assemble with separators, truncate to fit
            let mut detail = format!("  {}", dir);
            if !session.branch.is_empty() {
                detail.push_str(&format!("  {}", session.branch));
            }
            if !status.is_empty() {
                detail.push_str(&format!("  {}", status));
            }
            detail.push_str(&format!("  {}", activity));
            let detail = truncate(&detail, avail);

            // Render as single styled line (use muted for the truncated overview)
            let detail_line = pad_line(
                vec![Span::styled(
                    detail,
                    Style::default().fg(theme.subtle).bg(theme.bg),
                )],
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

fn build_tab_status(session: &SessionView) -> String {
    format_git_status(session, false)
}

fn format_keys_for(keybindings: &Keybindings, cmd: Command) -> String {
    let keys = keybindings.keys_for(cmd);
    keys.iter()
        .map(format_key)
        .collect::<Vec<_>>()
        .join("/")
}

fn primary_key_string(keybindings: &Keybindings, cmd: Command) -> String {
    keybindings
        .keys_for(cmd)
        .first()
        .map(format_key)
        .unwrap_or_default()
}

fn draw_help(frame: &mut Frame, area: Rect, theme: &Theme, keybindings: &Keybindings) {
    let key_span = |k: String| -> Span<'static> {
        Span::styled(format!("  {k:<10}"), Style::default().fg(theme.accent))
    };
    let desc_span = |d: &'static str| Span::styled(d, Style::default().fg(theme.secondary));

    let mut lines: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "  Keybindings",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];

    for &cmd in Command::ALL {
        let keys = format_keys_for(keybindings, cmd);
        if keys.is_empty() {
            continue;
        }
        lines.push(Line::from(vec![key_span(keys), desc_span(cmd.description())]));
    }

    lines.push(Line::from(vec![
        key_span("1-9".to_string()),
        desc_span("quick jump"),
    ]));
    lines.push(Line::from(vec![
        key_span("Mouse".to_string()),
        desc_span("click All / Idle / Working tabs"),
    ]));

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  press any key to close",
        Style::default().fg(theme.dim),
    )));

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );
}

fn draw_confirm_kill(frame: &mut Frame, area: Rect, theme: &Theme, name: &str) {
    let lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  Kill ", Style::default().fg(theme.text)),
            Span::styled(
                name,
                Style::default()
                    .fg(theme.yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("?", Style::default().fg(theme.text)),
        ]),
        Line::raw(""),
        Line::from(Span::styled("  y/n", Style::default().fg(theme.muted))),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );
}

fn draw_rename_input(frame: &mut Frame, area: Rect, theme: &Theme, input: &str, cursor: usize) {
    use unicode_width::UnicodeWidthStr;

    let max_w = area.width.saturating_sub(4) as usize;

    // Find a display-width-safe prefix of the input to show
    let (display, cursor_pos) = if input.width() > max_w {
        // Scroll so cursor is visible: show the tail
        let mut start = 0;
        for (i, _ch) in input.char_indices() {
            if input[i..].width() <= max_w {
                start = i;
                break;
            }
        }
        let display = &input[start..];
        let cursor_pos = if cursor >= start { cursor - start } else { 0 };
        (display, cursor_pos)
    } else {
        (input, cursor)
    };

    // Split at cursor using char boundary
    let cursor_pos = cursor_pos.min(display.len());
    let before = &display[..cursor_pos];
    let after = &display[cursor_pos..];

    // Get the character under cursor (first char of `after`), or space if at end
    let (cursor_char, rest) = if let Some(ch) = after.chars().next() {
        let len = ch.len_utf8();
        (&after[..len], &after[len..])
    } else {
        (" ", "")
    };

    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "  Rename session",
            Style::default().fg(theme.text),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(before, Style::default().fg(theme.accent)),
            Span::styled(cursor_char, Style::default().fg(theme.bg).bg(theme.accent)),
            Span::styled(rest, Style::default().fg(theme.accent)),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "  Enter confirm / Esc cancel",
            Style::default().fg(theme.muted),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );
}

pub fn draw_context_menu(
    frame: &mut Frame,
    menu_x: u16,
    menu_y: u16,
    selected: usize,
    items: &[&str],
    theme: &Theme,
) {
    let w = context_menu_width(items);
    let h = items.len() as u16 + 2;
    let area = frame.area();
    let x = menu_x.min(area.width.saturating_sub(w));
    let y = menu_y.min(area.height.saturating_sub(h));

    let menu_area = Rect::new(x, y, w, h);

    // Clear underlying content
    frame.render_widget(Clear, menu_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(theme.dim))
        .style(Style::default().bg(theme.surface));
    let inner = block.inner(menu_area);
    frame.render_widget(block, menu_area);

    let inner_w = inner.width as usize;
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let label = format!(" {:<width$}", item, width = inner_w.saturating_sub(1));
            if i == selected {
                Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.bg).bg(theme.accent),
                ))
            } else {
                Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.secondary).bg(theme.surface),
                ))
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn draw_settings_page(frame: &mut Frame, area: Rect, settings: &SettingsView, theme: &Theme) {
    frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);

    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                "  Settings",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  main pane", Style::default().fg(theme.dim)),
        ]),
        Line::from(Span::styled(
            "  Change appearance and layout without leaving the current session.",
            Style::default().fg(theme.subtle),
        )),
        Line::raw(""),
    ];

    let entries: Vec<(&str, String, &str)> = vec![
        (
            "Theme",
            settings.theme_name.to_string(),
            "Enter opens the theme list",
        ),
        (
            "Layout",
            match settings.layout_mode {
                LayoutMode::Horizontal => "Horizontal".to_string(),
                LayoutMode::Vertical => "Vertical".to_string(),
            },
            "Left/right toggles the split direction",
        ),
        (
            "Borders",
            if settings.show_borders { "On" } else { "Off" }.to_string(),
            "Left/right toggles pane borders",
        ),
        (
            "View",
            match settings.view_mode {
                ViewMode::Expanded => "Expanded".to_string(),
                ViewMode::Compact => "Compact".to_string(),
            },
            "Left/right toggles compact mode",
        ),
        (
            "Exclude",
            format!("{} patterns", settings.exclude_count),
            "Enter opens the pattern editor",
        ),
        (
            "Keybindings",
            "View".to_string(),
            "Enter shows current key bindings",
        ),
        (
            "Update check",
            if settings.update_check_enabled {
                "Enabled"
            } else {
                "Disabled"
            }
            .to_string(),
            settings.update_check_help.as_str(),
        ),
    ];

    for (idx, (label, value, help)) in entries.iter().enumerate() {
        let selected = idx == settings.selected;
        let row_bg = if selected { theme.surface } else { theme.bg };
        let label_style = if selected {
            Style::default()
                .fg(theme.text)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.secondary).bg(row_bg)
        };
        let value_style = if selected {
            Style::default()
                .fg(theme.bg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.teal).bg(row_bg)
        };
        let help_style = Style::default()
            .fg(if selected { theme.subtle } else { theme.dim })
            .bg(row_bg);
        let marker_style = Style::default()
            .fg(if selected { theme.accent } else { theme.bg })
            .bg(row_bg);

        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().bg(row_bg)),
            Span::styled(if selected { "▌" } else { " " }, marker_style),
            Span::styled(format!(" {:<10}", label), label_style),
            Span::styled(" ", Style::default().bg(row_bg)),
            Span::styled(format!(" {} ", value), value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("      ", Style::default().bg(row_bg)),
            Span::styled(help.to_string(), help_style),
        ]));
        lines.push(Line::raw(""));
    }

    lines.push(Line::from(Span::styled(
        "  j/k move  h/l change  Enter select  Esc close",
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(Span::styled(
        if settings.focus_main {
            "  Settings focus is active."
        } else {
            "  Press Ctrl+s to move focus back here."
        },
        Style::default().fg(theme.dim),
    )));

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );

    if settings.theme_picker_open {
        draw_theme_picker(frame, area, settings, theme);
    }

    if let Some(ref editor) = settings.exclude_editor {
        draw_exclude_editor(frame, area, editor, theme);
    }

    if settings.keybindings_view_open {
        draw_keybindings_view(
            frame,
            area,
            settings.keybindings,
            settings.keybindings_view_scroll,
            theme,
        );
    }
}

fn draw_keybindings_view(
    frame: &mut Frame,
    area: Rect,
    keybindings: &Keybindings,
    scroll: u16,
    theme: &Theme,
) {
    let rows: Vec<(&'static str, String, bool)> = Command::ALL
        .iter()
        .map(|&cmd| {
            let keys = format_keys_for(keybindings, cmd);
            (cmd.name(), keys, cmd.is_global())
        })
        .collect();

    let name_width = rows
        .iter()
        .map(|(n, _, _)| n.len())
        .max()
        .unwrap_or(16)
        .max(16);
    let keys_width = rows
        .iter()
        .map(|(_, k, _)| k.len())
        .max()
        .unwrap_or(8)
        .max(8);

    let popup_width = (name_width as u16 + keys_width as u16 + 16)
        .min(area.width.saturating_sub(4))
        .max(30);
    let content_height = rows.len() as u16 + 6;
    let popup_height = content_height
        .min(area.height.saturating_sub(2))
        .max(7);
    let x = area.x + area.width.saturating_sub(popup_width) / 2;
    let y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Keybindings ")
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.bg));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Reserve last 3 rows for footer (blank + 2 hint lines).
    let list_rows = inner.height.saturating_sub(3) as usize;
    let total = rows.len();
    let max_scroll = total.saturating_sub(list_rows) as u16;
    let scroll = scroll.min(max_scroll) as usize;
    let end = (scroll + list_rows).min(total);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (name, keys, is_global) in &rows[scroll..end] {
        let display_keys = if keys.is_empty() {
            "<unbound>".to_string()
        } else {
            keys.clone()
        };
        let key_style = if keys.is_empty() {
            Style::default().fg(theme.dim)
        } else {
            Style::default().fg(theme.accent)
        };
        let name_cell = format!("  {:<width$}  ", name, width = name_width);
        let keys_cell = format!("{:<width$}", display_keys, width = keys_width);
        let mut spans = vec![
            Span::styled(name_cell, Style::default().fg(theme.secondary)),
            Span::styled(keys_cell, key_style),
        ];
        if *is_global {
            spans.push(Span::styled(
                "  (global)".to_string(),
                Style::default().fg(theme.dim),
            ));
        }
        lines.push(Line::from(spans));
    }

    while lines.len() < list_rows {
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  Esc close  j/k scroll",
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(Span::styled(
        "  edit ~/.config/deck/config.json to change",
        Style::default().fg(theme.dim),
    )));

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        inner,
    );
}

fn draw_theme_picker(frame: &mut Frame, area: Rect, settings: &SettingsView, theme: &Theme) {
    let width = settings
        .theme_names
        .iter()
        .map(|name| name.len())
        .max()
        .unwrap_or(12)
        .min(area.width.saturating_sub(4) as usize)
        + 6;
    let height = (settings.theme_names.len() as u16 + 2).min(area.height.saturating_sub(2));
    let popup_width = (width as u16).min(area.width.saturating_sub(2)).max(12);
    let popup_height = height.max(3);
    let x = area.x + area.width.saturating_sub(popup_width) / 2;
    let y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .title(" Theme ")
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.surface));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let inner_w = inner.width as usize;
    let lines: Vec<Line> = settings
        .theme_names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            let selected = idx == settings.theme_picker_selected;
            let label = format!(" {:<width$}", name, width = inner_w.saturating_sub(1));
            if selected {
                Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.bg).bg(theme.accent),
                ))
            } else {
                Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.text).bg(theme.surface),
                ))
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_exclude_editor(frame: &mut Frame, area: Rect, editor: &ExcludeEditorView, theme: &Theme) {
    let pattern_count = editor.patterns.len();
    let max_pattern_width = editor
        .patterns
        .iter()
        .map(|p| p.len())
        .max()
        .unwrap_or(0)
        .max(20);

    let content_lines = pattern_count
        + if editor.adding { 1 } else { 0 }
        + if editor.error.is_some() { 1 } else { 0 };
    let height = (content_lines as u16 + 4)
        .min(area.height.saturating_sub(2))
        .max(5);
    let width = (max_pattern_width as u16 + 8)
        .max(30)
        .min(area.width.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Exclude Patterns ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.bg));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines: Vec<Line> = Vec::new();

    if pattern_count == 0 && !editor.adding {
        lines.push(Line::from(Span::styled(
            "  No patterns defined",
            Style::default().fg(theme.dim),
        )));
    }

    for (i, pattern) in editor.patterns.iter().enumerate() {
        let selected = !editor.adding && i == editor.selected;
        let row_bg = if selected { theme.surface } else { theme.bg };
        let marker = if selected { "▌" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(
                marker,
                Style::default()
                    .fg(if selected { theme.accent } else { theme.bg })
                    .bg(row_bg),
            ),
            Span::styled(
                format!(" {} ", pattern),
                Style::default().fg(theme.text).bg(row_bg),
            ),
        ]));
    }

    if editor.adding {
        let display_input = if editor.input.is_empty() {
            "│"
        } else {
            editor.input
        };
        lines.push(Line::from(vec![
            Span::styled("▌", Style::default().fg(theme.green).bg(theme.surface)),
            Span::styled(
                format!(" {} ", display_input),
                Style::default().fg(theme.text).bg(theme.surface),
            ),
        ]));
    }

    if let Some(err) = editor.error {
        lines.push(Line::from(Span::styled(
            format!("  {}", err),
            Style::default().fg(theme.pink),
        )));
    }

    // Help line
    lines.push(Line::raw(""));
    let help = if editor.adding {
        "  Enter: confirm  Esc: cancel"
    } else {
        "  a: add  d: delete  Esc: close"
    };
    lines.push(Line::from(Span::styled(
        help,
        Style::default().fg(theme.muted),
    )));

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        inner,
    );
}

fn truncate(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width <= 1 {
        return ".".to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;

    for ch in s.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width + 1 > max_width {
            break;
        }
        out.push(ch);
        width += ch_width;
    }

    format!("{out}…")
}

#[cfg(test)]
mod tests {
    use super::{format_git_status, format_idle_badge, truncate, SessionView};

    fn session_view<'a>(
        branch: &'a str,
        ahead: u32,
        behind: u32,
        staged: u32,
        modified: u32,
        untracked: u32,
    ) -> SessionView<'a> {
        SessionView {
            name: "demo",
            dir: "/tmp/demo",
            branch,
            ahead,
            behind,
            staged,
            modified,
            untracked,
            idle_seconds: 0,
        }
    }

    #[test]
    fn truncate_handles_unicode_without_panic() {
        assert_eq!(truncate("🪆 Nested deck detected", 10), "🪆 Nested…");
    }

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn idle_time_uses_human_units() {
        assert_eq!(format_idle_badge(5), None);
        assert_eq!(format_idle_badge(59), None);
        assert_eq!(format_idle_badge(60), Some("1m".to_string()));
        assert_eq!(format_idle_badge(3600), Some("1h".to_string()));
        assert_eq!(format_idle_badge(172800), Some("2d".to_string()));
    }

    #[test]
    fn git_status_prefers_symbol_format() {
        let dirty = session_view("main", 2, 1, 3, 4, 5);
        assert_eq!(format_git_status(&dirty, false), "↑2 ↓1 +3 ~4 ?5");
        assert_eq!(format_git_status(&dirty, true), "↑2↓1+3~4?5");

        let clean = session_view("main", 0, 0, 0, 0, 0);
        assert_eq!(format_git_status(&clean, false), "✓");

        let no_git = session_view("", 0, 0, 0, 0, 0);
        assert!(format_git_status(&no_git, false).is_empty());
    }
}

fn shorten_dir(dir: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && dir.starts_with(&home) {
        format!("~{}", &dir[home.len()..])
    } else {
        dir.to_string()
    }
}

fn scroll_offset(focused: usize, visible_height: u16, ch: usize) -> usize {
    let focused_bottom = (focused + 1) * ch;
    let visible = visible_height as usize;
    if focused_bottom > visible {
        focused_bottom - visible
    } else {
        0
    }
}

fn format_idle_badge(seconds: u64) -> Option<String> {
    if seconds < 60 {
        return None;
    }
    if seconds < 3600 {
        return Some(format!("{}m", seconds / 60));
    }
    if seconds < 86_400 {
        return Some(format!("{}h", seconds / 3600));
    }
    Some(format!("{}d", seconds / 86_400))
}

fn format_activity_compact(seconds: u64, spinner_frame: &str) -> String {
    if seconds < 3 {
        return spinner_frame.to_string();
    }
    format_idle_badge(seconds).unwrap_or_else(|| "󰒲".to_string())
}

fn idle_color(theme: &Theme, idle_seconds: u64, emphasized: bool) -> ratatui::style::Color {
    if !emphasized {
        return theme.muted;
    }
    if idle_seconds < 3 {
        theme.green
    } else if idle_seconds < 60 {
        theme.subtle
    } else if idle_seconds < 3600 {
        theme.muted
    } else {
        theme.dim
    }
}

fn format_git_status(session: &SessionView, compact: bool) -> String {
    let mut parts: Vec<String> = Vec::new();

    if session.ahead > 0 {
        parts.push(format!("↑{}", session.ahead));
    }
    if session.behind > 0 {
        parts.push(format!("↓{}", session.behind));
    }
    if session.staged > 0 {
        parts.push(format!("+{}", session.staged));
    }
    if session.modified > 0 {
        parts.push(format!("~{}", session.modified));
    }
    if session.untracked > 0 {
        parts.push(format!("?{}", session.untracked));
    }

    if parts.is_empty() && !session.branch.is_empty() {
        return "✓".to_string();
    }
    if compact {
        parts.join("")
    } else {
        parts.join(" ")
    }
}

fn build_status_spans_symbols<'a>(
    session: &SessionView,
    bg: ratatui::style::Color,
    theme: &Theme,
    emphasized: bool,
    compact: bool,
) -> Vec<Span<'a>> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut push = |text: String, color| {
        if compact {
            spans.push(Span::styled(text, Style::default().fg(color).bg(bg)));
            return;
        }
        if !spans.is_empty() {
            spans.push(Span::styled(" ", Style::default().bg(bg)));
        }
        spans.push(Span::styled(text, Style::default().fg(color).bg(bg)));
    };
    let ahead_color = if emphasized { theme.green } else { theme.muted };
    let behind_color = if emphasized {
        theme.yellow
    } else {
        theme.muted
    };
    let staged_color = if emphasized { theme.teal } else { theme.muted };
    let modified_color = if emphasized {
        theme.yellow
    } else {
        theme.muted
    };
    let untracked_color = theme.muted;
    let clean_color = if emphasized { theme.green } else { theme.muted };

    if session.ahead > 0 {
        push(format!("↑{}", session.ahead), ahead_color);
    }
    if session.behind > 0 {
        push(format!("↓{}", session.behind), behind_color);
    }
    if session.staged > 0 {
        push(format!("+{}", session.staged), staged_color);
    }
    if session.modified > 0 {
        push(format!("~{}", session.modified), modified_color);
    }
    if session.untracked > 0 {
        push(format!("?{}", session.untracked), untracked_color);
    }

    if spans.is_empty() && !session.branch.is_empty() {
        spans.push(Span::styled("✓", Style::default().fg(clean_color).bg(bg)));
    }

    spans
}

fn build_status_spans<'a>(
    session: &SessionView,
    emphasized: bool,
    bg: ratatui::style::Color,
    theme: &Theme,
    max_width: usize,
) -> Vec<Span<'a>> {
    let spaced = build_status_spans_symbols(session, bg, theme, emphasized, false);
    if spaced.iter().map(|span| span.width()).sum::<usize>() <= max_width {
        return spaced;
    }

    let compact = build_status_spans_symbols(session, bg, theme, emphasized, true);
    if compact.iter().map(|span| span.width()).sum::<usize>() <= max_width {
        return compact;
    }

    let text = truncate(&format_git_status(session, true), max_width);
    if text.is_empty() {
        return vec![];
    }

    let color = if text == "✓" {
        if emphasized {
            theme.green
        } else {
            theme.muted
        }
    } else {
        theme.muted
    };
    vec![Span::styled(text, Style::default().fg(color).bg(bg))]
}

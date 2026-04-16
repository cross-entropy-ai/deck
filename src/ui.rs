use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::state::{FilterMode, LayoutMode, ViewMode, FILTER_TABS};
use crate::theme::Theme;

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
}

/// Draw the sidebar into the given area.
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
) {
    if tabs_mode {
        draw_sidebar_tabs(
            frame,
            area,
            sessions,
            focused,
            sidebar_active,
            theme,
            show_borders,
            spinner_frame,
        );
        return;
    }
    let content = if show_borders {
        let border_color = if sidebar_active {
            theme.accent
        } else {
            theme.bg
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

    let [header_area, sessions_area, footer_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(content);

    draw_header(frame, header_area, sessions.len(), theme, filter_mode);
    if show_help {
        draw_help(frame, sessions_area, theme);
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
    );
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

                let accent_color = if is_focused {
                    theme.green
                } else {
                    theme.bg
                };

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
                let dir_color = if is_focused {
                    theme.teal
                } else {
                    theme.muted
                };
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
                    let branch_color = if is_focused {
                        theme.pink
                    } else {
                        theme.muted
                    };
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
                let status_spans = build_status_spans(session, is_emphasized, bg, theme, text_width);
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

        let accent_color = if is_focused {
            theme.green
        } else {
            theme.bg
        };
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
            Span::styled(truncate(session.name, width.saturating_sub(6)), name_style.bg(bg)),
        ];

        if !session.branch.is_empty() {
            let branch_color = if is_focused {
                theme.pink
            } else {
                theme.muted
            };
            spans.push(Span::styled("  ", Style::default().bg(bg)));
            spans.push(Span::styled(
                truncate(session.branch, width.saturating_sub(20)),
                Style::default().fg(branch_color).bg(bg),
            ));

            let status = format_git_status(session, true);
            if !status.is_empty() {
                let status_color = if status == "✓" {
                    if is_emphasized { theme.green } else { theme.muted }
                } else if is_emphasized {
                    theme.yellow
                } else {
                    theme.dim
                };
                spans.push(Span::styled(" ", Style::default().bg(bg)));
                spans.push(Span::styled(status, Style::default().fg(status_color).bg(bg)));
            }
        }

        lines.push(pad_line(spans, bg, width));

        // Row 2: directory
        let text_width = width.saturating_sub(6);
        let dir_display = truncate(&shorten_dir(session.dir), text_width);
        let dir_color = if is_focused {
            theme.teal
        } else {
            theme.muted
        };
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

fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    sidebar_active: bool,
    theme: &Theme,
    width: u16,
    show_help: bool,
    plugins: &[(char, &str)],
) {
    let w = width as usize;
    let sep = Line::from(Span::styled("─".repeat(w), Style::default().fg(theme.dim)));

    let hints = if sidebar_active {
        let mut spans = vec![
            Span::styled(" j/k", Style::default().fg(theme.muted)),
            Span::styled(" nav  ", Style::default().fg(theme.subtle)),
            Span::styled("t", Style::default().fg(theme.muted)),
            Span::styled(" settings  ", Style::default().fg(theme.subtle)),
            Span::styled("h/?", Style::default().fg(theme.muted)),
            Span::styled(" help  ", Style::default().fg(theme.subtle)),
            Span::styled("q", Style::default().fg(theme.muted)),
            Span::styled(" quit", Style::default().fg(theme.subtle)),
        ];
        for &(key, name) in plugins {
            spans.push(Span::styled("  ", Style::default()));
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme.muted),
            ));
            spans.push(Span::styled(
                format!(" {}", name),
                Style::default().fg(theme.subtle),
            ));
        }
        Line::from(spans)
    } else {
        Line::from(vec![
            Span::styled(" Ctrl+s", Style::default().fg(theme.muted)),
            Span::styled(" sidebar", Style::default().fg(theme.subtle)),
        ])
    };

    let info = if show_help {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                format!(
                    "About  {} v{}",
                    env!("CARGO_PKG_NAME"),
                    env!("CARGO_PKG_VERSION")
                ),
                Style::default().fg(theme.dim),
            ),
        ])
    } else if sidebar_active {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(theme.name, Style::default().fg(theme.dim)),
        ])
    } else {
        Line::default()
    };

    frame.render_widget(
        Paragraph::new(vec![sep, hints, info]).style(Style::default().bg(theme.bg)),
        area,
    );
}

fn draw_sidebar_tabs(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    sidebar_active: bool,
    theme: &Theme,
    show_borders: bool,
    spinner_frame: &str,
) {
    let content = if show_borders {
        let border_color = if sidebar_active {
            theme.accent
        } else {
            theme.bg
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
        return;
    }

    // Row 1: tab bar
    let tab_area = Rect {
        height: 1,
        ..content
    };
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(" ", Style::default().bg(theme.bg)));

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
        spans.push(Span::styled(" ", Style::default().bg(bg)));
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
        spans.push(Span::styled(" ", Style::default().bg(bg)));

        // Separator between tabs
        if i + 1 < sessions.len() {
            spans.push(Span::styled(
                "│",
                Style::default().fg(theme.dim).bg(theme.bg),
            ));
        }
    }

    // Right-align hints in the tab bar
    let tabs_width: usize = spans.iter().map(|s| s.width()).sum();
    let width = content.width as usize;
    let hint_spans: Vec<(&str, &str)> = if sidebar_active {
        vec![("h", " help  "), ("q", " quit")]
    } else {
        vec![("Ctrl+s", " sidebar")]
    };
    let hint_width: usize = hint_spans.iter().map(|(k, v)| k.len() + v.len()).sum();
    if tabs_width + hint_width + 2 < width {
        let gap = width - tabs_width - hint_width;
        spans.push(Span::styled(" ".repeat(gap), Style::default().bg(theme.bg)));
        for (k, v) in hint_spans {
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
}

fn build_tab_status(session: &SessionView) -> String {
    format_git_status(session, false)
}

/// Compute the column ranges for each tab. Returns vec of (start_col, end_col) relative to content.
pub fn tab_col_ranges(sessions: &[SessionView]) -> Vec<(u16, u16)> {
    let mut ranges = Vec::new();
    let mut x: u16 = 1; // initial padding
    for (i, session) in sessions.iter().enumerate() {
        let idx_width = format!("{}", i + 1).len() as u16;
        let name_width = UnicodeWidthStr::width(session.name) as u16;
        let tab_width = idx_width + 1 + name_width + 1; // "idx name "
        ranges.push((x, x + tab_width));
        x += tab_width;
        if i + 1 < sessions.len() {
            x += 1; // separator
        }
    }
    ranges
}

fn draw_help(frame: &mut Frame, area: Rect, theme: &Theme) {
    let key =
        |k: &'static str| Span::styled(format!("  {k:<10}"), Style::default().fg(theme.accent));
    let desc = |d: &'static str| Span::styled(d, Style::default().fg(theme.secondary));

    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "  Keybindings",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(vec![key("j/k"), desc("navigate")]),
        Line::from(vec![key("Enter"), desc("switch session")]),
        Line::from(vec![key("1-9"), desc("quick jump")]),
        Line::from(vec![key("x"), desc("kill session")]),
        Line::from(vec![key("t"), desc("open settings")]),
        Line::from(vec![key("b"), desc("toggle borders")]),
        Line::from(vec![key("l"), desc("toggle layout")]),
        Line::from(vec![key("Alt+↑↓"), desc("reorder")]),
        Line::from(vec![key("Mouse"), desc("click All / Idle / Working tabs")]),
        Line::from(vec![key("Ctrl+s"), desc("toggle sidebar")]),
        Line::from(vec![key("Esc"), desc("back to main")]),
        Line::from(vec![key("q"), desc("quit")]),
        Line::raw(""),
        Line::from(Span::styled(
            "  press any key to close",
            Style::default().fg(theme.dim),
        )),
    ];

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
            Span::styled(
                cursor_char,
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent),
            ),
            Span::styled(
                rest,
                Style::default().fg(theme.accent),
            ),
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

pub fn context_menu_width(items: &[&str]) -> u16 {
    let max_len = items.iter().map(|s| s.len()).max().unwrap_or(0);
    (max_len as u16) + 4 // 1 border + 1 padding each side + 1 border
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

fn draw_exclude_editor(
    frame: &mut Frame,
    area: Rect,
    editor: &ExcludeEditorView,
    theme: &Theme,
) {
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
    let height = (content_lines as u16 + 4).min(area.height.saturating_sub(2)).max(5);
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
            Span::styled(format!(" {} ", pattern), Style::default().fg(theme.text).bg(row_bg)),
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
        assert_eq!(
            truncate("🪆 Nested deck detected", 10),
            "🪆 Nested…"
        );
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

pub fn card_height(view_mode: ViewMode) -> usize {
    match view_mode {
        ViewMode::Expanded => 5,
        ViewMode::Compact => 2,
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

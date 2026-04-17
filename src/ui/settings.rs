use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::keybindings::{Command, Keybindings};
use crate::state::{LayoutMode, ViewMode};
use crate::theme::Theme;

use super::text::format_keys_for;
use super::{ExcludeEditorView, SettingsView};

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
    let popup_height = content_height.min(area.height.saturating_sub(2)).max(7);
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

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::keybindings::{Command, Keybindings};
use crate::theme::Theme;
use crate::ui::widgets::{
    centered_rect, clamp_popup_height, field_row, full_width_row, list_item_line, popup_frame,
    scroll_window, style_textarea, PopupStyle, TextAreaColors,
};

use super::text::format_keys_for;
use super::{ExcludeEditorView, SettingsView};

/// Widest display width in `it`, never below `floor`.
fn max_width<'a>(it: impl Iterator<Item = &'a str>, floor: usize) -> usize {
    it.map(UnicodeWidthStr::width).max().unwrap_or(0).max(floor)
}

pub fn draw_settings_page(frame: &mut Frame, area: Rect, settings: &SettingsView, theme: &Theme) {
    frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);

    let header: Vec<Line> = vec![
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
    let footer = Line::from(Span::styled(
        "  j/k move  h/l change  Enter select  Esc close",
        Style::default().fg(theme.muted),
    ));

    // Window the entries so the selected row stays visible on a short terminal
    // (#15). Each entry is variable-height; size the window by how many *whole*
    // entries fit after header/footer, then window by entry index around
    // `selected`. No persisted scroll state: offset derives from `selected`.
    let entries = &settings.rows;
    let entry_height = |row: &super::SettingRowView| 1 + row.help.lines().count() + 1;
    let body_rows = (area.height as usize)
        .saturating_sub(header.len())
        .saturating_sub(1); // footer
                            // Worst-case entry height bounds the window so a selected tall entry
                            // can't be pushed past the bottom.
    let max_entry_height = entries.iter().map(entry_height).max().unwrap_or(1);
    let visible = (body_rows / max_entry_height).max(1);
    let start = scroll_window(settings.selected, entries.len(), visible);
    let end = (start + visible).min(entries.len());

    let mut lines = header;
    for (offset, row) in entries[start..end].iter().enumerate() {
        let idx = start + offset;
        let label = row.label;
        let value = &row.value;
        let help = &row.help;
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
        for help_line in help.lines() {
            lines.push(Line::from(vec![
                Span::styled("      ", Style::default().bg(row_bg)),
                Span::styled(help_line.to_string(), help_style),
            ]));
        }
        lines.push(Line::raw(""));
    }

    lines.push(footer);

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );

    // The theme picker is drawn by the render loop, not here: it's a standalone
    // overlay openable from the sidebar (`t`) without entering this page, so it
    // renders over whatever main view is active. See `App::render`.

    if let Some(ref editor) = settings.exclude_editor {
        draw_exclude_editor(frame, area, editor, theme);
    }

    if let Some(input) = settings.summary_lang_input {
        draw_summary_language_editor(frame, area, input, theme);
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

    let name_width = max_width(rows.iter().map(|(n, _, _)| *n), 16);
    let keys_width = max_width(rows.iter().map(|(_, k, _)| k.as_str()), 8);

    let popup_width = (name_width as u16 + keys_width as u16 + 16)
        .min(area.width.saturating_sub(4))
        .max(30);
    let popup_height = clamp_popup_height(area, rows.len() as u16 + 6, 7);
    let popup_area = centered_rect(area, popup_width, popup_height);

    let inner = popup_frame(
        frame.buffer_mut(),
        popup_area,
        PopupStyle {
            title: Some(" Keybindings "),
            border_fg: theme.accent,
            bg: theme.bg,
        },
    );

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
        "  edit ~/.config/deck/config.yaml to change",
        Style::default().fg(theme.dim),
    )));

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        inner,
    );
}

/// Draw the theme picker overlay centered in `area`. Decoupled from the
/// settings page so it can be opened standalone from the sidebar (`t`); the
/// render loop calls this whenever the picker is open, over any main view.
pub fn draw_theme_picker(
    frame: &mut Frame,
    area: Rect,
    theme_names: &[&str],
    selected_idx: usize,
    theme: &Theme,
) {
    // Not `max_width`: 12 is the empty-list default here, not a floor.
    let width = theme_names
        .iter()
        .map(|name| UnicodeWidthStr::width(*name))
        .max()
        .unwrap_or(12)
        .min(area.width.saturating_sub(4) as usize)
        + 6;
    let popup_width = (width as u16).min(area.width.saturating_sub(2)).max(12);
    let popup_height = clamp_popup_height(area, theme_names.len() as u16 + 2, 3);
    let popup_area = centered_rect(area, popup_width, popup_height);

    // Pad the popup background one cell left and right (not top/bottom) so the
    // overlay floats with horizontal breathing room instead of sitting flush.
    // Clamped to `area` so padding never spills past the pane it's drawn in.
    let left = popup_area.x.saturating_sub(1).max(area.x);
    let right = (popup_area.right() + 1).min(area.right());
    let halo = Rect {
        x: left,
        y: popup_area.y,
        width: right - left,
        height: popup_area.height,
    };
    Clear.render(halo, frame.buffer_mut());
    Block::default()
        .style(Style::default().bg(theme.surface))
        .render(halo, frame.buffer_mut());

    let inner = popup_frame(
        frame.buffer_mut(),
        popup_area,
        PopupStyle {
            title: Some(" Theme "),
            border_fg: theme.accent,
            bg: theme.surface,
        },
    );

    // Window the list around the selection like the keybindings view, so
    // the highlight can't walk off-screen when the theme count exceeds the
    // visible height (#15).
    let inner_w = inner.width as usize;
    let visible = (inner.height as usize).max(1);
    let start = scroll_window(selected_idx, theme_names.len(), visible);
    let end = (start + visible).min(theme_names.len());
    let lines: Vec<Line> = theme_names[start..end]
        .iter()
        .enumerate()
        .map(|(offset, name)| {
            let style = if start + offset == selected_idx {
                Style::default().fg(theme.bg).bg(theme.accent)
            } else {
                Style::default().fg(theme.text).bg(theme.surface)
            };
            full_width_row(name, inner_w, style)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// A small centered popup with a single free-text field for the generated
/// summary's language. Empty = the model's default.
fn draw_summary_language_editor(
    frame: &mut Frame,
    area: Rect,
    input: &ratatui_textarea::TextArea<'static>,
    theme: &Theme,
) {
    let width = 44u16.min(area.width.saturating_sub(4));
    let popup_area = centered_rect(area, width, 6);
    let inner = popup_frame(
        frame.buffer_mut(),
        popup_area,
        PopupStyle {
            title: Some(" Summary Language "),
            border_fg: theme.accent,
            bg: theme.bg,
        },
    );

    let rows = Layout::vertical([
        Constraint::Length(1), // field
        Constraint::Length(1), // pad
        Constraint::Length(1), // hint
        Constraint::Min(0),
    ])
    .split(inner);

    // A 1-wide empty label over the popup's `theme.bg` fill leaves that
    // leading cell unchanged while indenting the field by one column.
    field_row(
        frame.buffer_mut(),
        rows[0],
        " ",
        Style::default().bg(theme.bg),
        input,
        true,
        TextAreaColors::field(theme, theme.accent, theme.bg),
    );

    Paragraph::new(Line::from(Span::styled(
        " e.g. English, 中文 — blank for default · Enter save / Esc cancel",
        Style::default().fg(theme.muted),
    )))
    .render(rows[2], frame.buffer_mut());
}

fn draw_exclude_editor(frame: &mut Frame, area: Rect, editor: &ExcludeEditorView, theme: &Theme) {
    let pattern_count = editor.patterns.len();
    let max_pattern_width = max_width(editor.patterns.iter().map(String::as_str), 20);

    let content_lines =
        pattern_count + usize::from(editor.adding) + usize::from(editor.error.is_some());
    // content rows + blank + help row + top/bottom borders = +4
    let height = clamp_popup_height(area, content_lines as u16 + 4, 5);
    let width = (max_pattern_width as u16 + 8)
        .max(30)
        .min(area.width.saturating_sub(4));
    let popup_area = centered_rect(area, width, height);

    let inner = popup_frame(
        frame.buffer_mut(),
        popup_area,
        PopupStyle {
            title: Some(" Exclude Patterns "),
            border_fg: theme.accent,
            bg: theme.bg,
        },
    );

    // Build row constraints: one per content line + blank + help.
    let mut constraints: Vec<Constraint> = Vec::new();
    if pattern_count == 0 && !editor.adding {
        constraints.push(Constraint::Length(1)); // "No patterns"
    }
    for _ in 0..pattern_count {
        constraints.push(Constraint::Length(1));
    }
    if editor.adding {
        constraints.push(Constraint::Length(1)); // textarea row
    }
    if editor.error.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // blank
    constraints.push(Constraint::Length(1)); // help
    constraints.push(Constraint::Min(0)); // tail

    let rows = Layout::vertical(constraints).split(inner);
    let mut row_idx: usize = 0;

    if pattern_count == 0 && !editor.adding {
        Paragraph::new(Span::styled(
            "  No patterns defined",
            Style::default().fg(theme.dim),
        ))
        .render(rows[row_idx], frame.buffer_mut());
        row_idx += 1;
    }

    for (i, pattern) in editor.patterns.iter().enumerate() {
        let selected = !editor.adding && i == editor.selected;
        let marker = if selected { "▌" } else { " " };
        Paragraph::new(list_item_line(
            theme,
            selected,
            marker,
            format!(" {} ", pattern),
        ))
        .render(rows[row_idx], frame.buffer_mut());
        row_idx += 1;
    }

    if editor.adding {
        // Split the row: marker (1 char) + textarea.
        let ta_row = rows[row_idx];
        let cols = Layout::horizontal([Constraint::Length(1), Constraint::Min(0)]).split(ta_row);
        Paragraph::new(Span::styled(
            "▌",
            Style::default().fg(theme.green).bg(theme.surface),
        ))
        .render(cols[0], frame.buffer_mut());

        let mut ta = editor.input.clone();
        style_textarea(
            &mut ta,
            true,
            TextAreaColors::field(theme, theme.text, theme.surface),
        );
        ta.render(cols[1], frame.buffer_mut());
        row_idx += 1;
    }

    if let Some(err) = editor.error {
        Paragraph::new(Span::styled(
            format!("  {}", err),
            Style::default().fg(theme.error),
        ))
        .render(rows[row_idx], frame.buffer_mut());
        row_idx += 1;
    }

    // blank row
    row_idx += 1;

    let help = if editor.adding {
        "  Enter: confirm  Esc: cancel"
    } else {
        "  a: add  d: delete  Esc: close"
    };
    Paragraph::new(Span::styled(help, Style::default().fg(theme.muted)))
        .render(rows[row_idx], frame.buffer_mut());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybindings::Keybindings;
    use ratatui::{backend::TestBackend, Terminal};

    fn sample_rows() -> Vec<super::super::SettingRowView> {
        // Representative rows with long help text to exercise the windowing
        // math. Self-contained on purpose: the renderer only consumes
        // `SettingRowView`, so the test doesn't reach into the `app`-layer table.
        [
            "Theme",
            "Transparent",
            "Layout",
            "Borders",
            "View",
            "Frame rate",
            "Exclude",
            "Keybindings",
            "Update check",
            "Summary lang",
            "Agents probe",
        ]
        .into_iter()
        .map(|label| super::super::SettingRowView {
            label,
            value: "value".to_string(),
            help: "a fairly long help line that takes a row".to_string(),
        })
        .collect()
    }

    /// #15 regression guard: on a short terminal the selected (last) row
    /// must still be painted, i.e. the page windows around the selection
    /// instead of letting it scroll off the bottom.
    #[test]
    fn short_terminal_keeps_selected_setting_in_view() {
        let theme = &crate::theme::THEMES[0];
        let keybindings = Keybindings::default();
        let rows = sample_rows();
        let last = rows.len() - 1;
        let last_label = rows[last].label;

        let backend = TestBackend::new(60, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                let view = SettingsView {
                    selected: last,
                    rows,
                    exclude_editor: None,
                    keybindings: &keybindings,
                    keybindings_view_open: false,
                    keybindings_view_scroll: 0,
                    summary_lang_input: None,
                };
                super::draw_settings_page(frame, area, &view, theme);
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[(x, y)].symbol());
            }
        }
        assert!(
            text.contains(last_label),
            "selected (last) setting {last_label:?} must be visible on a short \
             terminal; screen was: {text:?}"
        );
    }
}

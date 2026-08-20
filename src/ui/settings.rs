use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::keybindings::{Command, Keybindings};
use crate::state::SettingsPage;
use crate::theme::Theme;
use crate::ui::widgets::{
    clamp_popup_height, field_row, form_field_row, full_width_row, list_item_line, modal_footer,
    modal_list_lines, modal_selection_foreground, scroll_window, style_textarea, FormFieldState,
    ListViewport, ModalFrame, TextAreaColors,
};

use super::style::{text_style, TextRole};
use super::text::{format_keys_for, pad_line};
use super::{ExcludeEditorView, SettingsView, SshSettingEditorView};

/// Widest display width in `it`, never below `floor`.
fn max_width<'a>(it: impl Iterator<Item = &'a str>, floor: usize) -> usize {
    it.map(UnicodeWidthStr::width).max().unwrap_or(0).max(floor)
}

/// Clip and pad a label to one display-width column. `format!` width counts
/// characters rather than terminal cells, so do the display-width accounting
/// explicitly even though today's setting labels are ASCII.
fn label_cell(label: &str, width: usize) -> String {
    let clipped = crate::geometry::truncate(label, width);
    let padding = width.saturating_sub(clipped.width());
    format!("{clipped}{}", " ".repeat(padding))
}

pub fn draw_settings_page(frame: &mut Frame, area: Rect, settings: &SettingsView, theme: &Theme) {
    frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);

    let (title, context, description) = match settings.page {
        SettingsPage::Appearance => (
            "Appearance",
            "settings",
            "Configure theme, layout, borders, view, and frame rate.",
        ),
        SettingsPage::Theme => (
            "Theme",
            "settings",
            "Choose a fixed theme or follow the terminal's appearance.",
        ),
        SettingsPage::Agents => (
            "Agents",
            "settings",
            "Configure agent probes and generated summaries.",
        ),
        SettingsPage::Remote => (
            "Remote",
            "settings",
            "Manage remote hosts and port forwards.",
        ),
        SettingsPage::Root => (
            "Settings",
            "main pane",
            "Change appearance and layout without leaving the current session.",
        ),
    };
    let header: Vec<Line> = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                format!("  {title}"),
                text_style(theme, TextRole::ScreenTitle),
            ),
            Span::styled(format!("  {context}"), text_style(theme, TextRole::Context)),
        ]),
        Line::from(Span::styled(
            format!("  {description}"),
            text_style(theme, TextRole::Description),
        )),
        Line::raw(""),
    ];
    let footer_text = if settings.page != SettingsPage::Root {
        "  j/k move  Enter select  Esc back"
    } else {
        "  j/k move  Enter select  Esc close"
    };
    let footer = Line::from(Span::styled(footer_text, text_style(theme, TextRole::Hint)));

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

    // All values on one page share a column. Keep enough room for a useful
    // value on narrow panes and truncate the label column before it can push
    // values off-screen.
    const LABEL_FLOOR: usize = 10;
    const VALUE_MIN: usize = 8;
    const PRIMARY_ROW_CHROME: usize = 6; // "  " + marker + gap + value gap
    let preferred_label_width = max_width(entries.iter().map(|row| row.label), LABEL_FLOOR);
    let max_label_width = usize::from(area.width)
        .saturating_sub(PRIMARY_ROW_CHROME + VALUE_MIN)
        .max(1);
    let label_width = preferred_label_width.min(max_label_width);

    let mut lines = header;
    for (offset, row) in entries[start..end].iter().enumerate() {
        let idx = start + offset;
        let label = row.label;
        let value = &row.value;
        let help = &row.help;
        let selected = idx == settings.selected;
        let primary_bg = if selected {
            theme.selection_bg
        } else {
            theme.bg
        };
        let label_style = if selected {
            text_style(theme, TextRole::Selection).bg(primary_bg)
        } else {
            text_style(theme, TextRole::Item).bg(primary_bg)
        };
        let value_style = if selected {
            text_style(theme, TextRole::Selection).bg(primary_bg)
        } else {
            text_style(theme, TextRole::Value).bg(primary_bg)
        };
        // Help remains secondary even for the focused setting. The blue bar
        // identifies the actionable primary line; extending it through the
        // explanation would flatten both lines into the same visual weight.
        let help_style = text_style(theme, TextRole::Help).bg(theme.bg);
        let marker_style = if selected {
            text_style(theme, TextRole::Selection).bg(primary_bg)
        } else {
            Style::default().fg(theme.bg).bg(primary_bg)
        };

        lines.push(pad_line(
            vec![
                Span::styled("  ", Style::default().bg(primary_bg)),
                Span::styled(if selected { "▌" } else { " " }, marker_style),
                Span::styled(" ", Style::default().bg(primary_bg)),
                Span::styled(label_cell(label, label_width), label_style),
                Span::styled("  ", Style::default().bg(primary_bg)),
                Span::styled(value.clone(), value_style),
            ],
            primary_bg,
            usize::from(area.width),
        ));
        for help_line in help.lines() {
            lines.push(Line::from(vec![
                Span::styled("      ", Style::default().bg(theme.bg)),
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
}

pub fn draw_keybindings_view(
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
    let inner = ModalFrame::centered(popup_width, popup_height, Some("Keybindings"), theme)
        .render(frame.buffer_mut(), area);

    let list_rows = inner.height.saturating_sub(3) as usize;
    let total = rows.len();
    let max_scroll = total.saturating_sub(list_rows) as u16;
    let scroll = scroll.min(max_scroll) as usize;

    let mut lines: Vec<Line<'static>> = modal_list_lines(
        &rows,
        list_rows,
        ListViewport::Offset(scroll),
        |_, (name, keys, is_global)| {
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
            Line::from(spans)
        },
    );

    while lines.len() < list_rows {
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw(""));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.elevated)),
        inner,
    );
    let footer_rows = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    modal_footer(
        frame.buffer_mut(),
        footer_rows[1],
        "  Esc close  j/k scroll",
        theme,
    );
    modal_footer(
        frame.buffer_mut(),
        footer_rows[2],
        "  edit ~/.config/deck/config.yaml to change",
        theme,
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
    let inner = ModalFrame::centered(popup_width, popup_height, Some("Theme"), theme)
        .render(frame.buffer_mut(), area);

    // Window the list around the selection like the keybindings view, so
    // the highlight can't walk off-screen when the theme count exceeds the
    // visible height (#15).
    let inner_w = inner.width as usize;
    let visible = (inner.height as usize).max(1);
    let lines: Vec<Line> = modal_list_lines(
        theme_names,
        visible,
        ListViewport::FollowSelection(selected_idx),
        |idx, name| {
            let style = if idx == selected_idx {
                Style::default()
                    .fg(modal_selection_foreground(theme))
                    .bg(theme.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text).bg(theme.elevated)
            };
            full_width_row(name, inner_w, style)
        },
    );

    frame.render_widget(Paragraph::new(lines), inner);
}

/// A small centered popup with a single free-text field for the generated
/// summary's language. Empty = the model's default.
pub fn draw_summary_language_editor(
    frame: &mut Frame,
    area: Rect,
    input: &ratatui_textarea::TextArea<'static>,
    theme: &Theme,
) {
    let width = 44u16.min(area.width.saturating_sub(4));
    let inner = ModalFrame::centered(width, 6, Some("Summary Language"), theme)
        .render(frame.buffer_mut(), area);

    let rows = Layout::vertical([
        Constraint::Length(1), // field
        Constraint::Length(1), // pad
        Constraint::Length(1), // hint
        Constraint::Min(0),
    ])
    .split(inner);

    // A 1-wide empty label over the elevated modal surface leaves that
    // leading cell unchanged while indenting the field by one column.
    field_row(
        frame.buffer_mut(),
        rows[0],
        " ",
        Style::default().bg(theme.elevated),
        input,
        true,
        TextAreaColors::field(theme, theme.accent, theme.input_bg),
    );

    modal_footer(
        frame.buffer_mut(),
        rows[2],
        " e.g. English, 中文 — blank for default · Enter save / Esc cancel",
        theme,
    );
}

pub fn draw_ssh_setting_editor(
    frame: &mut Frame,
    area: Rect,
    editor: &SshSettingEditorView<'_>,
    theme: &Theme,
) {
    const LABEL_WIDTH: usize = 14;
    const CONTENT_OFFSET: usize = LABEL_WIDTH + 5;
    let (title, label, description, desired_width) = match editor.field {
        crate::overlay::SshSettingField::ControlPath => (
            "SSH Control Path",
            "Control path",
            "Socket path template; supports %r, %h, and %p.",
            76,
        ),
        crate::overlay::SshSettingField::ControlPersist => (
            "SSH Reuse Duration",
            "Reuse duration",
            "Idle lifetime: 10m, 1h30m, yes, or no.",
            70,
        ),
    };
    let height = if editor.error.is_some() { 11 } else { 9 };
    let width = desired_width.min(area.width.saturating_sub(4));
    let inner =
        ModalFrame::centered(width, height, Some(title), theme).render(frame.buffer_mut(), area);

    let mut constraints = vec![
        Constraint::Length(1), // description
        Constraint::Length(1), // pad
        Constraint::Length(1), // field
    ];
    if editor.error.is_some() {
        constraints.push(Constraint::Length(2));
    }
    constraints.extend([
        Constraint::Length(1), // pad
        Constraint::Length(1), // commands
        Constraint::Min(0),
    ]);
    let rows = Layout::vertical(constraints).split(inner);

    Paragraph::new(Span::styled(
        format!("    {description}"),
        text_style(theme, TextRole::Description).bg(theme.elevated),
    ))
    .render(rows[0], frame.buffer_mut());

    form_field_row(
        frame.buffer_mut(),
        rows[2],
        label,
        LABEL_WIDTH,
        editor.input,
        FormFieldState::Focused,
        theme,
    );

    let mut next = 3;
    if let Some(error) = editor.error {
        Paragraph::new(Line::from(Span::styled(
            format!("{}Error  {error}", " ".repeat(CONTENT_OFFSET)),
            Style::default().fg(theme.error).bg(theme.elevated),
        )))
        .wrap(Wrap { trim: true })
        .render(rows[next], frame.buffer_mut());
        next += 1;
    }
    next += 1; // pad
    modal_footer(
        frame.buffer_mut(),
        rows[next],
        "    [Enter] Save   [Esc] Cancel",
        theme,
    );
}

pub fn draw_exclude_editor(
    frame: &mut Frame,
    area: Rect,
    editor: &ExcludeEditorView,
    theme: &Theme,
) {
    let pattern_count = editor.patterns.len();
    let max_pattern_width = max_width(editor.patterns.iter().map(String::as_str), 20);

    let content_lines =
        pattern_count + usize::from(editor.adding) + usize::from(editor.error.is_some());
    // content rows + blank + help row + top/bottom borders = +4
    let height = clamp_popup_height(area, content_lines as u16 + 4, 5);
    let width = (max_pattern_width as u16 + 8)
        .max(30)
        .min(area.width.saturating_sub(4));
    let inner = ModalFrame::centered(width, height, Some("Exclude Patterns"), theme)
        .render(frame.buffer_mut(), area);

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

    let pattern_lines = modal_list_lines(
        editor.patterns,
        editor.patterns.len(),
        ListViewport::FollowSelection(editor.selected),
        |i, pattern| {
            let selected = !editor.adding && i == editor.selected;
            let marker = if selected { "▸" } else { " " };
            list_item_line(
                theme,
                selected,
                format!("  {marker} "),
                pattern.as_str(),
                inner.width as usize,
            )
        },
    );
    for line in pattern_lines {
        Paragraph::new(line).render(rows[row_idx], frame.buffer_mut());
        row_idx += 1;
    }

    if editor.adding {
        // Split the row: standard list indent/marker + textarea.
        let ta_row = rows[row_idx];
        let cols = Layout::horizontal([Constraint::Length(4), Constraint::Min(0)]).split(ta_row);
        Paragraph::new(Span::styled(
            "  ▸ ",
            Style::default().fg(theme.green).bg(theme.elevated),
        ))
        .render(cols[0], frame.buffer_mut());

        let mut ta = editor.input.clone();
        style_textarea(
            &mut ta,
            true,
            TextAreaColors::field(theme, theme.text, theme.input_bg),
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
    modal_footer(frame.buffer_mut(), rows[row_idx], help, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::{backend::TestBackend, Terminal};

    fn cell_x(buf: &Buffer, y: u16, symbol: &str) -> u16 {
        (0..buf.area.width)
            .find(|&x| buf[(x, y)].symbol() == symbol)
            .unwrap()
    }

    fn buffer_lines(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

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
                    page: SettingsPage::Root,
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

    #[test]
    fn setting_values_align_to_the_pages_longest_label() {
        let theme = &crate::theme::THEMES[0];
        let rows = vec![
            super::super::SettingRowView {
                label: "Theme",
                value: "111".to_string(),
                help: "short explanation".to_string(),
            },
            super::super::SettingRowView {
                label: "Transparent background",
                value: "222".to_string(),
                help: "another explanation".to_string(),
            },
        ];

        let backend = TestBackend::new(72, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let view = SettingsView {
                    selected: 0,
                    rows,
                    page: SettingsPage::Theme,
                };
                super::draw_settings_page(frame, frame.area(), &view, theme);
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let first_x = cell_x(buf, 4, "1");
        let second_x = cell_x(buf, 7, "2");
        assert_eq!(first_x, second_x);
    }

    #[test]
    fn selected_setting_highlights_only_its_primary_line() {
        let mut theme = crate::theme::THEMES[0];
        theme.selection_bg = ratatui::style::Color::Rgb(1, 2, 3);
        theme.selection_fg = ratatui::style::Color::Rgb(250, 251, 252);
        let rows = vec![super::super::SettingRowView {
            label: "Theme",
            value: "value".to_string(),
            help: "explanation".to_string(),
        }];

        let backend = TestBackend::new(48, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let view = SettingsView {
                    selected: 0,
                    rows,
                    page: SettingsPage::Theme,
                };
                super::draw_settings_page(frame, frame.area(), &view, &theme);
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        assert_eq!(buf[(4, 4)].bg, theme.selection_bg);
        assert_eq!(buf[(4, 4)].fg, theme.selection_fg);
        assert_eq!(buf[(47, 4)].bg, theme.selection_bg);
        assert_eq!(buf[(6, 5)].symbol(), "e");
        assert_eq!(buf[(6, 5)].bg, theme.bg);
        assert_eq!(buf[(6, 5)].fg, theme.dim);
    }

    #[test]
    fn ssh_editor_keeps_context_field_error_and_actions_in_reading_order() {
        let theme = &crate::theme::THEMES[0];
        let input = ratatui_textarea::TextArea::default();
        let backend = TestBackend::new(90, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let view = SshSettingEditorView {
                    field: crate::overlay::SshSettingField::ControlPath,
                    input: &input,
                    error: Some("Path is not usable"),
                };
                super::draw_ssh_setting_editor(frame, frame.area(), &view, theme);
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let context_y = lines
            .iter()
            .position(|line| line.contains("Socket path template"))
            .unwrap();
        let field_y = lines
            .iter()
            .position(|line| line.contains("Control path"))
            .unwrap();
        let error_y = lines
            .iter()
            .position(|line| line.contains("Error  Path is not usable"))
            .unwrap();
        let actions_y = lines
            .iter()
            .position(|line| line.contains("[Enter] Save   [Esc] Cancel"))
            .unwrap();

        assert!(context_y < field_y);
        assert_eq!(error_y, field_y + 1);
        assert!(error_y < actions_y);
        assert!(lines[field_y].contains('▌'));
    }
}

//! List/row rendering: selectable row spans, full-width highlight rows,
//! selection windowing, and the shared filter-picker list.

use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

use super::scroll::scrollbar_cells;

/// How a modal list chooses its first visible item.
#[derive(Debug, Clone, Copy)]
pub enum ListViewport {
    /// Keep this selected item in view, scrolling only when necessary.
    FollowSelection(usize),
    /// Use an explicit scroll offset, clamped so the final window is full.
    Offset(usize),
}

/// Stateful viewport for a single-selection picker list.
#[derive(Debug, Clone, Copy)]
pub(super) struct PickerViewport {
    pub selected: usize,
    pub scroll: usize,
    pub focused: bool,
}

/// Foreground that maintains readable contrast over the theme's accent fill.
pub fn modal_selection_foreground(theme: &Theme) -> ratatui::style::Color {
    contrasting_foreground(theme, theme.accent)
}

/// Foreground that maintains readable contrast over an arbitrary semantic
/// fill (accent selection, warning action, and future modal controls).
pub fn contrasting_foreground(theme: &Theme, fill: ratatui::style::Color) -> ratatui::style::Color {
    use ratatui::style::Color;

    let Some(fill_luminance) = relative_luminance(fill) else {
        return theme.text;
    };
    let bg_contrast = contrast_with(fill_luminance, theme.bg);
    let text_contrast = contrast_with(fill_luminance, theme.text);
    let (themed_color, themed_contrast) = if bg_contrast > text_contrast {
        (theme.bg, bg_contrast)
    } else {
        (theme.text, text_contrast)
    };
    if themed_contrast >= 4.5 {
        return themed_color;
    }

    let black = Color::Rgb(0, 0, 0);
    let white = Color::Rgb(255, 255, 255);
    if contrast_with(fill_luminance, black) > contrast_with(fill_luminance, white) {
        black
    } else {
        white
    }
}

fn relative_luminance(color: ratatui::style::Color) -> Option<f64> {
    let ratatui::style::Color::Rgb(r, g, b) = color else {
        return None;
    };
    let channel = |value: u8| {
        let value = f64::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b))
}

fn contrast_with(base: f64, color: ratatui::style::Color) -> f64 {
    let Some(other) = relative_luminance(color) else {
        return 0.0;
    };
    let (lighter, darker) = if base > other {
        (base, other)
    } else {
        (other, base)
    };
    (lighter + 0.05) / (darker + 0.05)
}

/// One full-width selectable modal row. Unselected rows use the modal surface;
/// selected rows use an accent fill with a contrast-safe foreground.
pub fn list_item_line<'a>(
    theme: &Theme,
    selected: bool,
    marker: impl Into<Cow<'a, str>>,
    content: impl Into<Cow<'a, str>>,
    width: usize,
) -> Line<'a> {
    let marker = marker.into();
    let content = content.into();
    let used = marker.as_ref().width() + content.as_ref().width();
    let padding = " ".repeat(width.saturating_sub(used));
    let row_bg = if selected {
        theme.accent
    } else {
        theme.surface
    };
    let content_fg = if selected {
        modal_selection_foreground(theme)
    } else {
        theme.text
    };
    let marker_fg = if selected { content_fg } else { theme.surface };
    let row_style = |fg| {
        let style = Style::default().fg(fg).bg(row_bg);
        if selected {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    };
    Line::from(vec![
        Span::styled(marker, row_style(marker_fg)),
        Span::styled(content, row_style(content_fg)),
        Span::styled(padding, Style::default().bg(row_bg)),
    ])
}

/// A full-width selectable row as a single styled span: `label` left-aligned in
/// `width` columns with a 1-col leading pad. Shared by the context menu and
/// theme picker (caller picks `style` per selected/disabled state).
pub fn full_width_row(label: &str, width: usize, style: Style) -> Line<'static> {
    let padding = " ".repeat(width.saturating_sub(1 + label.width()));
    Line::from(Span::styled(format!(" {label}{padding}"), style))
}

/// Compute the first visible index so that `selected` stays in view.
pub fn scroll_window(selected: usize, total: usize, window: usize) -> usize {
    if total <= window || selected < window {
        return 0;
    }
    let max_start = total - window;
    (selected + 1).saturating_sub(window).min(max_start)
}

/// Build the visible lines for a modal list. This is the common windowing and
/// iteration path for pickers, menus, settings lists, and port forwards; each
/// caller supplies only the row's content and styles.
pub fn modal_list_lines<'items, 'line, T>(
    items: &'items [T],
    visible: usize,
    viewport: ListViewport,
    mut line: impl FnMut(usize, &'items T) -> Line<'line>,
) -> Vec<Line<'line>> {
    if visible == 0 || items.is_empty() {
        return Vec::new();
    }
    let start = match viewport {
        ListViewport::FollowSelection(selected) => scroll_window(selected, items.len(), visible),
        ListViewport::Offset(offset) => offset.min(items.len().saturating_sub(visible)),
    };
    let end = (start + visible).min(items.len());
    items[start..end]
        .iter()
        .enumerate()
        .map(|(offset, item)| line(start + offset, item))
        .collect()
}

/// Render a windowed, single-selection list into `rows`, one item per row.
///
/// Used by the shared filter picker with its stored viewport; the selected
/// item gets a `▸` marker and `content(filtered[i])` supplies row text. `rows`
/// must hold at least `window` slots; any beyond the rendered items are left
/// untouched (caller reserves them as blanks).
pub fn draw_picker_list(
    buf: &mut Buffer,
    rows: &[Rect],
    theme: &Theme,
    filtered: &[usize],
    viewport: PickerViewport,
    window: usize,
    mut content: impl FnMut(usize) -> String,
) {
    let row_width = rows.first().map_or(0, |row| row.width as usize);
    let start = viewport.scroll.min(filtered.len().saturating_sub(window));
    let scrollbar = scrollbar_cells(window, filtered.len(), start);
    let lines = modal_list_lines(
        filtered,
        window,
        ListViewport::Offset(start),
        |display, &idx| {
            let sel = viewport.focused && display == viewport.selected;
            let marker = if sel { "\u{25b8}" } else { " " };
            let bar = scrollbar.get(display - start).copied().flatten();
            let mut line = list_item_line(
                theme,
                sel,
                format!("  {marker} "),
                content(idx),
                row_width.saturating_sub(usize::from(bar.is_some())),
            );
            if let Some(glyph) = bar {
                line.spans.push(Span::styled(
                    glyph,
                    Style::default().fg(theme.dim).bg(theme.surface),
                ));
            }
            line
        },
    );
    for (pos, line) in lines.into_iter().enumerate() {
        Paragraph::new(line).render(rows[pos], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_selection_foreground_is_readable_for_every_theme() {
        for theme in crate::theme::THEMES {
            let accent = relative_luminance(theme.accent).unwrap();
            let contrast = contrast_with(accent, modal_selection_foreground(theme));
            assert!(
                contrast >= 4.5,
                "{} modal selection contrast is only {contrast:.2}:1",
                theme.name
            );

            let warning = relative_luminance(theme.yellow).unwrap();
            let contrast = contrast_with(warning, contrasting_foreground(theme, theme.yellow));
            assert!(
                contrast >= 4.5,
                "{} warning action contrast is only {contrast:.2}:1",
                theme.name
            );
        }
    }

    #[test]
    fn modal_list_rows_use_surface_and_full_width_accent_selection() {
        let theme = &crate::theme::THEMES[0];
        let unselected = list_item_line(theme, false, "    ", "entry", 12);
        let selected = list_item_line(theme, true, "  ▸ ", "entry", 12);

        assert_eq!(unselected.width(), 12);
        assert!(unselected
            .spans
            .iter()
            .all(|span| span.style.bg == Some(theme.surface)));
        assert_eq!(selected.width(), 12);
        assert!(selected
            .spans
            .iter()
            .all(|span| span.style.bg == Some(theme.accent)));
    }

    #[test]
    fn full_width_row_pads_by_terminal_columns() {
        let line = full_width_row("中文", 8, Style::default());
        assert_eq!(line.width(), 8);
    }

    #[test]
    fn picker_list_highlights_selection_and_draws_scrollbar_at_right_edge() {
        let theme = &crate::theme::THEMES[0];
        let area = Rect::new(0, 0, 12, 3);
        let rows = [
            Rect::new(0, 0, 12, 1),
            Rect::new(0, 1, 12, 1),
            Rect::new(0, 2, 12, 1),
        ];
        let filtered = [0, 1, 2, 3, 4];

        let mut top = Buffer::empty(area);
        draw_picker_list(
            &mut top,
            &rows,
            theme,
            &filtered,
            PickerViewport {
                selected: 0,
                scroll: 0,
                focused: true,
            },
            3,
            |idx| format!("dir-{idx}/"),
        );
        assert_eq!(top[(11, 0)].symbol(), "█");
        assert_eq!(top[(11, 1)].symbol(), "░");
        assert_eq!(top[(1, 0)].bg, theme.accent);

        let mut bottom = Buffer::empty(area);
        draw_picker_list(
            &mut bottom,
            &rows,
            theme,
            &filtered,
            PickerViewport {
                selected: 4,
                scroll: 2,
                focused: true,
            },
            3,
            |idx| format!("dir-{idx}/"),
        );
        assert_eq!(bottom[(11, 0)].symbol(), "░");
        assert_eq!(bottom[(11, 2)].symbol(), "█");
        assert_eq!(bottom[(1, 2)].bg, theme.accent);
    }

    #[test]
    fn picker_list_hides_selection_when_another_field_has_focus() {
        let theme = &crate::theme::THEMES[0];
        let area = Rect::new(0, 0, 12, 2);
        let rows = [Rect::new(0, 0, 12, 1), Rect::new(0, 1, 12, 1)];
        let filtered = [0, 1];
        let mut buf = Buffer::empty(area);

        draw_picker_list(
            &mut buf,
            &rows,
            theme,
            &filtered,
            PickerViewport {
                selected: 1,
                scroll: 0,
                focused: false,
            },
            2,
            |idx| format!("dir-{idx}/"),
        );

        assert!(buf.content().iter().all(|cell| cell.bg != theme.accent));
        assert!(buf.content().iter().all(|cell| cell.symbol() != "▸"));
    }
}

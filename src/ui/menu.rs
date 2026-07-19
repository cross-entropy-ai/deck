use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::geometry::context_menu_rect;
use crate::state::MenuItem;
use crate::theme::Theme;
use crate::ui::widgets::{full_width_row, popup_frame, PopupStyle};

/// Pick the theme foreground with the strongest contrast against the accent
/// fill. A fixed `theme.bg` works for dark themes, but becomes light-on-light
/// for warm/pastel accents in several light themes.
fn highlight_foreground(theme: &Theme) -> Color {
    let Some(accent) = relative_luminance(theme.accent) else {
        return theme.text;
    };
    let bg_contrast = relative_luminance(theme.bg)
        .map(|fg| contrast_ratio(fg, accent))
        .unwrap_or(0.0);
    let text_contrast = relative_luminance(theme.text)
        .map(|fg| contrast_ratio(fg, accent))
        .unwrap_or(0.0);
    let (themed_color, themed_contrast) = if bg_contrast > text_contrast {
        (theme.bg, bg_contrast)
    } else {
        (theme.text, text_contrast)
    };
    if themed_contrast >= 4.5 {
        return themed_color;
    }

    // Keep the theme's own foreground whenever it reaches the readability
    // target. Only edge-case accents fall back to an absolute endpoint.
    let black = Color::Rgb(0, 0, 0);
    let white = Color::Rgb(255, 255, 255);
    if contrast_ratio(0.0, accent) > contrast_ratio(1.0, accent) {
        black
    } else {
        white
    }
}

fn relative_luminance(color: Color) -> Option<f64> {
    let Color::Rgb(r, g, b) = color else {
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

fn contrast_ratio(a: f64, b: f64) -> f64 {
    let (lighter, darker) = if a > b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

pub fn draw_context_menu(
    frame: &mut Frame,
    menu_x: u16,
    menu_y: u16,
    selected: usize,
    items: &[MenuItem],
    disabled: &[MenuItem],
    theme: &Theme,
) {
    // Same rect `AppState::menu_item_at` hit-tests against.
    let area = frame.area();
    let menu_area = context_menu_rect(items, menu_x, menu_y, area.width, area.height);

    let inner = popup_frame(
        frame.buffer_mut(),
        menu_area,
        PopupStyle {
            title: None,
            border_fg: theme.dim,
            bg: theme.surface,
        },
    );

    let inner_w = inner.width as usize;
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            // Greyed-out items are shown for context but not selectable, so
            // they never take the accent highlight even at `selected`.
            let style = if disabled.contains(item) {
                Style::default().fg(theme.dim).bg(theme.surface)
            } else if i == selected {
                Style::default()
                    .fg(highlight_foreground(theme))
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.secondary).bg(theme.surface)
            };
            full_width_row(item.label(), inner_w, style)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MenuItem;
    use crate::theme::THEMES;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn highlight_foreground_meets_contrast_target_for_every_theme() {
        for theme in THEMES {
            let fg = relative_luminance(highlight_foreground(theme)).unwrap();
            let accent = relative_luminance(theme.accent).unwrap();
            assert!(
                contrast_ratio(fg, accent) >= 4.5,
                "{} menu highlight contrast is only {:.2}:1",
                theme.name,
                contrast_ratio(fg, accent)
            );
        }
    }

    #[test]
    fn light_theme_renders_selected_label_with_contrasting_text() {
        let theme = THEMES
            .iter()
            .find(|theme| theme.name == "Raycast (Light)")
            .unwrap();
        let backend = TestBackend::new(30, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_context_menu(
                    frame,
                    1,
                    1,
                    0,
                    &[MenuItem::Rename, MenuItem::Close],
                    &[],
                    theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let selected_cell = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "R")
            .expect("selected Rename label must be rendered");
        assert_eq!(selected_cell.bg, theme.accent);
        assert_eq!(selected_cell.fg, highlight_foreground(theme));
        assert_ne!(selected_cell.fg, theme.bg);
    }
}

//! Popup sizing and framing: centering/clamping a modal rect and drawing
//! the rounded bordered frame every overlay shares.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};

use crate::theme::Theme;
use crate::ui::style::{text_style, TextRole};

/// Center a `width` x `height` rect inside `area`, clamping each
/// dimension to `area` so the popup never overflows its bounds.
pub fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}

/// Clamp a popup's height: at least `min_height`, at most the area minus a
/// 1-row top/bottom margin. Shared by `popup_rect` and the settings popups
/// (which need a different width margin) so the vertical-fit rule lives once.
pub fn clamp_popup_height(area: Rect, content_height: u16, min_height: u16) -> u16 {
    content_height
        .max(min_height)
        .min(area.height.saturating_sub(2))
}

/// Size and center a filter-picker popup: clamp `content_height` to
/// `min_height`..area (1-row top/bottom, 2-col left/right margin), clamp
/// `width` likewise, then center in `area`.
pub fn popup_rect(area: Rect, width: u16, content_height: u16, min_height: u16) -> Rect {
    let height = clamp_popup_height(area, content_height, min_height);
    let width = width.min(area.width.saturating_sub(4));
    centered_rect(area, width, height)
}

/// Resolved visual options for a rounded modal frame. Kept private so modal
/// callers choose a semantic frame variant instead of independently selecting
/// background and border colors.
#[derive(Clone, Copy)]
struct ModalStyle<'a> {
    title: Option<&'a str>,
    border_fg: Color,
    bg: Color,
}

impl<'a> ModalStyle<'a> {
    fn standard(theme: &Theme, title: Option<&'a str>) -> Self {
        Self {
            title,
            border_fg: theme.focus_border,
            bg: theme.elevated,
        }
    }

    fn subtle(theme: &Theme, title: Option<&'a str>) -> Self {
        Self {
            title,
            border_fg: theme.border,
            bg: theme.elevated,
        }
    }

    fn warning(theme: &Theme, title: Option<&'a str>) -> Self {
        Self {
            title,
            border_fg: theme.yellow,
            bg: theme.elevated,
        }
    }
}

/// Placement for a modal surface. Most modals are centered; anchored popovers
/// such as the context menu supply their already-clamped rectangle directly.
#[derive(Clone, Copy)]
enum ModalPlacement {
    Centered { width: u16, height: u16 },
    Exact(Rect),
}

/// Shared visual shell for every framed modal: placement, clearing, rounded
/// border, title, and background. Modal bodies only decide their content and
/// desired dimensions.
#[derive(Clone, Copy)]
pub struct ModalFrame<'a> {
    placement: ModalPlacement,
    style: ModalStyle<'a>,
}

impl<'a> ModalFrame<'a> {
    /// Standard modal surface: accent border over the theme's opaque surface.
    pub fn centered(width: u16, height: u16, title: Option<&'a str>, theme: &Theme) -> Self {
        Self {
            placement: ModalPlacement::Centered { width, height },
            style: ModalStyle::standard(theme, title),
        }
    }

    /// Standard modal with caller-supplied placement (pickers and popovers).
    pub fn exact(area: Rect, title: Option<&'a str>, theme: &Theme) -> Self {
        Self {
            placement: ModalPlacement::Exact(area),
            style: ModalStyle::standard(theme, title),
        }
    }

    /// Visually quieter anchored surface used by the context menu.
    pub fn subtle_exact(area: Rect, title: Option<&'a str>, theme: &Theme) -> Self {
        Self {
            placement: ModalPlacement::Exact(area),
            style: ModalStyle::subtle(theme, title),
        }
    }

    /// Warning surface: standard modal background with a semantic warning border.
    pub fn warning_centered(
        width: u16,
        height: u16,
        title: Option<&'a str>,
        theme: &Theme,
    ) -> Self {
        Self {
            placement: ModalPlacement::Centered { width, height },
            style: ModalStyle::warning(theme, title),
        }
    }

    /// Resolve the modal's outer rectangle inside `bounds`.
    pub fn area(self, bounds: Rect) -> Rect {
        match self.placement {
            ModalPlacement::Centered { width, height } => {
                let safe_bounds = modal_bounds(bounds);
                centered_rect(safe_bounds, width, height)
            }
            ModalPlacement::Exact(area) => area.intersection(bounds),
        }
    }

    /// Draw the modal shell inside `bounds` and return its content rectangle.
    pub fn render(self, buf: &mut Buffer, bounds: Rect) -> Rect {
        let area = self.area(bounds);
        clear_horizontal_margin(buf, area, bounds, self.style.bg);
        popup_frame(buf, area, self.style)
    }
}

/// Reserve one real terminal cell around centered modals whenever the pane is
/// large enough. The horizontal clearing halo additionally protects the border
/// from adjacent wide glyphs.
fn modal_bounds(bounds: Rect) -> Rect {
    let horizontal = u16::from(bounds.width > 2);
    let vertical = u16::from(bounds.height > 2);
    Rect::new(
        bounds.x.saturating_add(horizontal),
        bounds.y.saturating_add(vertical),
        bounds.width.saturating_sub(horizontal * 2),
        bounds.height.saturating_sub(vertical * 2),
    )
}

/// Draw a modal's one-line command hint with the standard elevated style.
pub fn modal_footer(buf: &mut Buffer, area: Rect, text: &str, theme: &Theme) {
    Paragraph::new(Span::styled(
        text,
        text_style(theme, TextRole::Hint).bg(theme.elevated),
    ))
    .render(area, buf);
}

/// The rect covering `hint` inside a footer row that was drawn with `text`.
///
/// Modal footers double as button bars: a hint like `⏎ mount` is both the label
/// and the mouse's way to trigger it. Locating the hint in the very string that
/// was painted keeps the target and the text from drifting when either is
/// reworded. `None` when the hint is absent; the result is clipped to `row`, so
/// a terminal too narrow to hold the whole footer yields a target no wider than
/// the text it can actually show.
pub fn hint_rect(row: Rect, text: &str, hint: &str) -> Option<Rect> {
    use unicode_width::UnicodeWidthStr;

    let offset = text.find(hint)?;
    let x = row.x.saturating_add(text[..offset].width() as u16);
    Some(row.intersection(Rect {
        x,
        y: row.y,
        width: hint.width() as u16,
        height: 1,
    }))
    .filter(|rect| !rect.is_empty())
}

/// Clear one cell immediately outside each vertical edge of the modal.
///
/// A double-width glyph that starts beside a modal can otherwise occupy the
/// border's cell in the terminal, making the border look interrupted. Keeping
/// this margin in the shared frame makes the protection apply to every modal,
/// including anchored ones, without changing their requested dimensions.
fn clear_horizontal_margin(buf: &mut Buffer, area: Rect, bounds: Rect, bg: Color) {
    let left = area.x.saturating_sub(1).max(bounds.x);
    let right = area.right().saturating_add(1).min(bounds.right());
    let margin = Rect::new(left, area.y, right.saturating_sub(left), area.height);

    Clear.render(margin, buf);
    Block::default()
        .style(Style::default().bg(bg))
        .render(margin, buf);
}

/// Clear `area`, draw a rounded bordered block over it, and return the inner
/// content rect. Kept private so every framed overlay goes through `ModalFrame`.
fn popup_frame(buf: &mut Buffer, area: Rect, style: ModalStyle<'_>) -> Rect {
    Clear.render(area, buf);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(style.border_fg))
        .style(Style::default().bg(style.bg));
    let block = match style.title {
        Some(title) => block.title(format!(" {title} ")),
        None => block,
    };
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_frame_clears_one_cell_beside_both_vertical_edges() {
        let bounds = Rect::new(0, 0, 12, 5);
        let mut buf = Buffer::empty(bounds);

        // The wide glyph starts in the future left margin and occupies the
        // future border cell. Rendering the modal must clear both cells before
        // painting the border.
        Paragraph::new("  界").render(Rect::new(0, 2, bounds.width, 1), &mut buf);

        let theme = &crate::theme::THEMES[0];
        ModalFrame::centered(6, 3, None, theme).render(&mut buf, bounds);

        // Centered width 6 in width 12 occupies x=3..9, so x=2 and x=9
        // are the shared one-cell horizontal margins.
        assert_eq!(buf[(2, 2)].symbol(), " ");
        assert_eq!(buf[(2, 2)].bg, theme.elevated);
        assert_eq!(buf[(3, 2)].symbol(), "│");
        assert_eq!(buf[(9, 2)].symbol(), " ");
        assert_eq!(buf[(9, 2)].bg, theme.elevated);
    }

    #[test]
    fn modal_frame_clamps_horizontal_margin_to_bounds() {
        let bounds = Rect::new(4, 2, 6, 5);
        let mut buf = Buffer::empty(bounds);

        let inner =
            ModalFrame::centered(20, 3, None, &crate::theme::THEMES[0]).render(&mut buf, bounds);

        assert_eq!(inner, Rect::new(6, 4, 2, 1));
        assert_eq!(buf[(4, 3)].symbol(), " ");
        assert_eq!(buf[(9, 3)].symbol(), " ");
    }

    #[test]
    fn modal_frame_normalizes_title_padding_and_elevated_color() {
        let bounds = Rect::new(0, 0, 16, 5);
        let mut buf = Buffer::empty(bounds);
        let theme = &crate::theme::THEMES[0];

        ModalFrame::centered(12, 3, Some("Title"), theme).render(&mut buf, bounds);

        let top: String = (2..14).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(top.contains(" Title "));
        assert_eq!(buf[(3, 2)].bg, theme.elevated);
    }

    #[test]
    fn modal_frame_uses_semantic_surface_and_border_roles() {
        let bounds = Rect::new(0, 0, 12, 5);
        let mut theme = crate::theme::THEMES[0];
        theme.elevated = Color::Rgb(1, 2, 3);
        theme.focus_border = Color::Rgb(4, 5, 6);
        theme.border = Color::Rgb(7, 8, 9);

        let mut standard = Buffer::empty(bounds);
        ModalFrame::centered(8, 3, None, &theme).render(&mut standard, bounds);
        assert_eq!(standard[(3, 2)].bg, theme.elevated);
        assert_eq!(standard[(2, 2)].fg, theme.focus_border);

        let mut subtle = Buffer::empty(bounds);
        ModalFrame::subtle_exact(Rect::new(2, 1, 8, 3), None, &theme).render(&mut subtle, bounds);
        assert_eq!(subtle[(3, 2)].bg, theme.elevated);
        assert_eq!(subtle[(2, 2)].fg, theme.border);
    }
}

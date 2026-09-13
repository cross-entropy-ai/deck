//! The Buddy connection prompt.
//!
//! Unlike the kill confirmation — the other yes/no surface — this one is drawn
//! the same way in both layouts. It is raised by something arriving from the
//! network rather than by a gesture the user just made, so it has to be equally
//! visible whichever way the panes are split.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;

use crate::geometry::BuddyHits;
use crate::theme::Theme;
use crate::ui::style::{text_style, TextRole};
use crate::ui::widgets::{hint_rect, modal_footer, ModalFrame};

const TITLE: &str = "Buddy";
const FOOTER: &str = "  y allow   n deny";

pub fn draw_buddy_approve(
    frame: &mut Frame,
    area: Rect,
    peer: std::net::IpAddr,
    theme: &Theme,
) -> BuddyHits {
    let peer = peer.to_string();
    // Wide enough for the address plus the sentence around it, but never wider
    // than the pane.
    let width = (peer.chars().count() as u16 + 34).clamp(34, area.width.max(1));
    let inner = ModalFrame::centered(width, 7, Some(TITLE), theme).render(frame.buffer_mut(), area);
    if inner.height == 0 {
        return BuddyHits::default();
    }

    let body = vec![
        Line::from(Span::styled(
            format!("  {peer}"),
            text_style(theme, TextRole::ScreenTitle),
        )),
        Line::from(Span::styled(
            "  wants to control this Mac",
            text_style(theme, TextRole::Description),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "  It can type and click as you.",
            text_style(theme, TextRole::Hint),
        )),
    ];
    let text_area = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    Paragraph::new(body).render(text_area, frame.buffer_mut());

    let footer = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };
    modal_footer(frame.buffer_mut(), footer, FOOTER, theme);
    BuddyHits {
        allow: hint_rect(footer, FOOTER, "y allow").unwrap_or_default(),
        deny: hint_rect(footer, FOOTER, "n deny").unwrap_or_default(),
    }
}

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::geometry::{TAB_INNER_PAD, TAB_LEADING_PAD, TAB_SEPARATOR};

use super::super::text::pad_line;
use super::super::SidebarSession;
use super::container::draw_sidebar_container;
use super::{menu_span, SidebarRenderCtx, MENU_LABEL};

pub(super) struct TabsProps<'a> {
    pub sessions: &'a [&'a dyn SidebarSession],
    pub focused: usize,
    pub sidebar_active: bool,
    pub show_borders: bool,
}

pub(super) fn draw_sidebar_tabs(
    frame: &mut Frame,
    area: Rect,
    ctx: &SidebarRenderCtx<'_>,
    props: TabsProps<'_>,
) -> Option<Rect> {
    let theme = ctx.theme;
    let sessions = props.sessions;
    let focused = props.focused;
    let content =
        draw_sidebar_container(frame, area, theme, props.sidebar_active, props.show_borders);

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

        let label = crate::geometry::tab_label(session.host(), session.name());

        let bg = if is_focused { theme.surface } else { theme.bg };
        let name_fg = if session.unreachable() {
            theme.dim
        } else if is_focused {
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
            label,
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
    let menu_width = MENU_LABEL.width();
    let menu_bounds = if tabs_width + menu_width + 2 < width {
        let gap = width - tabs_width - menu_width - 1;
        spans.push(Span::styled(" ".repeat(gap), Style::default().bg(theme.bg)));
        spans.push(menu_span(theme));
        Some(Rect {
            x: content.x + (width - menu_width - 1) as u16,
            y: content.y,
            width: menu_width as u16,
            height: 1,
        })
    } else {
        None
    };
    let tab_line = pad_line(spans, theme.bg, width);
    frame.render_widget(
        Paragraph::new(vec![tab_line]).style(Style::default().bg(theme.bg)),
        tab_area,
    );
    menu_bounds
}

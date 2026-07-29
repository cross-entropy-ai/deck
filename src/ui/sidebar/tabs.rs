use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::geometry::{tab_bar_layout, truncate, TAB_OVERFLOW_MARKER, TAB_SEPARATOR};

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
    let mut spans: Vec<Span> = Vec::new();
    let labels: Vec<String> = sessions
        .iter()
        .map(|session| crate::geometry::tab_label(session.host(), session.name()))
        .collect();
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let layout = tab_bar_layout(&label_refs, focused, content.width);
    let mut cursor = 0u16;
    let pad_to = |target: u16, spans: &mut Vec<Span<'static>>, cursor: &mut u16| {
        if target > *cursor {
            spans.push(Span::styled(
                " ".repeat((target - *cursor) as usize),
                Style::default().bg(theme.bg),
            ));
            *cursor = target;
        }
    };

    pad_to(1, &mut spans, &mut cursor);
    if layout.left_clipped {
        spans.push(Span::styled(
            format!("{TAB_OVERFLOW_MARKER} "),
            Style::default().fg(theme.dim).bg(theme.bg),
        ));
        cursor += 2;
    }

    for (visible_pos, tab) in layout.tabs.iter().enumerate() {
        pad_to(tab.start, &mut spans, &mut cursor);
        let i = tab.index;
        let session = sessions[i];
        let is_focused = i == focused;
        let tab_width = tab.end - tab.start;
        let idx = format!("{}", i + 1);
        let idx_width = idx.width() as u16;
        let after_idx = tab_width.saturating_sub(idx_width);
        let label_room = after_idx.saturating_sub(2) as usize;
        let label = truncate(&labels[i], label_room);

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

        spans.push(Span::styled(idx, Style::default().fg(idx_fg).bg(bg)));
        if after_idx > 0 {
            let label_style = Style::default()
                .fg(name_fg)
                .bg(bg)
                .add_modifier(if is_focused {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });
            let used = 1 + label.width() as u16;
            let trailing = after_idx.saturating_sub(used);
            spans.push(Span::styled(
                format!(" {label}{}", " ".repeat(trailing as usize)),
                label_style,
            ));
        }
        cursor = tab.end;

        if visible_pos + 1 < layout.tabs.len() {
            spans.push(Span::styled(
                TAB_SEPARATOR,
                Style::default().fg(theme.dim).bg(theme.bg),
            ));
            cursor += TAB_SEPARATOR.width() as u16;
        }
    }

    if layout.right_clipped {
        spans.push(Span::styled(
            format!(" {TAB_OVERFLOW_MARKER}"),
            Style::default().fg(theme.dim).bg(theme.bg),
        ));
        cursor += 2;
    }

    let menu_bounds = if let Some(menu_x) = layout.menu_x {
        pad_to(menu_x, &mut spans, &mut cursor);
        spans.push(menu_span(theme));
        Some(Rect {
            x: content.x + menu_x,
            y: content.y,
            width: MENU_LABEL.width() as u16,
            height: 1,
        })
    } else {
        None
    };
    let tab_line = pad_line(spans, theme.bg, content.width as usize);
    frame.render_widget(
        Paragraph::new(vec![tab_line]).style(Style::default().bg(theme.bg)),
        tab_area,
    );
    menu_bounds
}

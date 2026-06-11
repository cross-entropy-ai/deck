use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keybindings::Command;
use crate::layout::{TAB_INNER_PAD, TAB_LEADING_PAD, TAB_SEPARATOR};

use super::super::text::{pad_line, primary_key_string};
use super::super::{SessionOrigin, SidebarSession};
use super::container::draw_sidebar_container;
use super::SidebarRenderCtx;

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
) {
    let theme = ctx.theme;
    let keybindings = ctx.keybindings;
    let sessions = props.sessions;
    let focused = props.focused;
    let content =
        draw_sidebar_container(frame, area, theme, props.sidebar_active, props.show_borders);

    if content.height == 0 {
        return;
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

        let label = match session.origin() {
            SessionOrigin::Local => crate::layout::tab_label(None, session.name()),
            SessionOrigin::Remote { host } => crate::layout::tab_label(Some(host), session.name()),
        };

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
    let hint_pairs: Vec<(String, String)> = if props.sidebar_active {
        vec![
            (
                primary_key_string(keybindings, Command::ToggleHelp),
                " help  ".into(),
            ),
            (
                primary_key_string(keybindings, Command::Quit),
                " quit".into(),
            ),
        ]
    } else {
        vec![(
            primary_key_string(keybindings, Command::ToggleFocus),
            " sidebar".into(),
        )]
    };
    let hint_pairs: Vec<(String, String)> = hint_pairs
        .into_iter()
        .filter(|(k, _)| !k.is_empty())
        .collect();
    let hint_width: usize = hint_pairs.iter().map(|(k, v)| k.len() + v.len()).sum();
    if tabs_width + hint_width + 2 < width {
        let gap = width - tabs_width - hint_width;
        spans.push(Span::styled(" ".repeat(gap), Style::default().bg(theme.bg)));
        for (k, v) in hint_pairs {
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
}

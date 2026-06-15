use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;
use crate::update::UpdateStatus;

use super::{menu_span, SidebarRenderCtx, MENU_LABEL};

pub(super) struct FooterProps<'a> {
    pub update_available: Option<&'a UpdateStatus>,
}

/// Click regions the footer publishes each frame: the update banner's
/// "upgrade" link and the "menu" button that opens the global context menu.
#[derive(Default)]
pub(super) struct FooterHits {
    pub upgrade: Option<Rect>,
    pub menu: Option<Rect>,
}

/// A full-width horizontal rule in the footer's dim color.
fn divider_line(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.dim),
    ))
}

pub(super) fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    ctx: &SidebarRenderCtx<'_>,
    props: FooterProps<'_>,
) -> FooterHits {
    let theme = ctx.theme;
    let w = area.width as usize;
    let sep = divider_line(w, theme);

    let rows_capacity = usize::from(2 + props.update_available.is_some() as u16);
    let mut rows: Vec<Line> = Vec::with_capacity(rows_capacity);
    rows.push(sep);

    let mut upgrade_bounds: Option<Rect> = None;
    if let Some(status) = props.update_available {
        let upgrade_label = "upgrade";
        let leading = 1u16;
        let gap = 3u16;
        let upgrade_width = upgrade_label.width() as u16;
        let full = format!(
            "v{} available (current v{})",
            status.latest_version, status.current_version
        );
        let short = format!("v{} available", status.latest_version);
        let tiny = "update available".to_string();
        let chosen = [full, short, tiny]
            .into_iter()
            .find(|text| leading + text.width() as u16 + gap + upgrade_width <= area.width);

        let banner_row_y = area.y + rows.len() as u16;

        if let Some(banner_text) = chosen {
            let text_width = banner_text.width() as u16;
            let upgrade_x = area.x + leading + text_width + gap;
            upgrade_bounds = Some(Rect {
                x: upgrade_x,
                y: banner_row_y,
                width: upgrade_width,
                height: 1,
            });
            rows.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(banner_text, Style::default().fg(theme.dim)),
                Span::raw("   "),
                Span::styled(
                    upgrade_label.to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ),
            ]));
        } else if leading + upgrade_width <= area.width {
            upgrade_bounds = Some(Rect {
                x: area.x + leading,
                y: banner_row_y,
                width: upgrade_width,
                height: 1,
            });
            rows.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    upgrade_label.to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ),
            ]));
        } else {
            rows.push(Line::default());
        }
    }

    // The "menu" button opens the global context menu — the same one a
    // right-click on empty space shows.
    let menu_y = area.y + rows.len() as u16;
    let menu = Some(Rect {
        x: area.x + 1,
        y: menu_y,
        width: MENU_LABEL.width() as u16,
        height: 1,
    });
    rows.push(Line::from(vec![
        Span::raw(" "),
        menu_span(theme),
        Span::styled(
            format!("   [$ deck v{}]", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme.dim),
        ),
    ]));

    frame.render_widget(
        Paragraph::new(rows).style(Style::default().bg(theme.bg)),
        area,
    );

    FooterHits {
        upgrade: upgrade_bounds,
        menu,
    }
}

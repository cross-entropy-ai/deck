use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::layout::plugin_block_rows;
use crate::theme::Theme;
use crate::update::UpdateStatus;

use super::super::{PluginStatus, PluginView};
use super::SidebarRenderCtx;

pub(super) struct FooterProps<'a> {
    pub plugins: &'a [PluginView<'a>],
    pub update_available: Option<&'a UpdateStatus>,
}

/// Click regions the footer publishes each frame: the update banner's
/// "upgrade" link and the "menu" button that opens the global context menu.
#[derive(Default)]
pub(super) struct FooterHits {
    pub upgrade: Option<Rect>,
    pub menu: Option<Rect>,
}

struct PluginRowsProps<'a> {
    plugins: &'a [PluginView<'a>],
    width: usize,
}

fn plugin_dot_style(status: PluginStatus, blink_on: bool, theme: &Theme) -> Style {
    match status {
        PluginStatus::Foreground => Style::default().fg(theme.green),
        PluginStatus::Background => {
            if blink_on {
                Style::default()
                    .fg(theme.yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.dim)
            }
        }
        PluginStatus::Inactive => Style::default().fg(theme.dim),
    }
}

fn plugin_dot_glyph(status: PluginStatus) -> &'static str {
    match status {
        PluginStatus::Inactive => "○",
        _ => "●",
    }
}

fn append_plugin_rows(
    rows: &mut Vec<Line<'static>>,
    ctx: &SidebarRenderCtx<'_>,
    props: PluginRowsProps<'_>,
) {
    let theme = ctx.theme;
    let plugins = props.plugins;
    let width = props.width;
    if plugins.is_empty() {
        return;
    }

    rows.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("\u{eb5c}", Style::default().fg(theme.accent)),
        Span::styled(
            " Plugins",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ]));

    for p in plugins {
        let dot_style = plugin_dot_style(p.status, ctx.blink_on, theme);
        let key_color = match p.status {
            PluginStatus::Inactive => theme.dim,
            _ => theme.muted,
        };
        let name_color = match p.status {
            PluginStatus::Foreground => theme.text,
            PluginStatus::Background => theme.secondary,
            PluginStatus::Inactive => theme.muted,
        };
        let name_style = match p.status {
            PluginStatus::Foreground => {
                Style::default().fg(name_color).add_modifier(Modifier::BOLD)
            }
            _ => Style::default().fg(name_color),
        };
        rows.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(plugin_dot_glyph(p.status), dot_style),
            Span::raw(" "),
            Span::styled(p.key.to_string(), Style::default().fg(key_color)),
            Span::raw("  "),
            Span::styled(p.name.to_string(), name_style),
        ]));
    }

    rows.push(Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.dim),
    )));
}

pub(super) fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    ctx: &SidebarRenderCtx<'_>,
    props: FooterProps<'_>,
) -> FooterHits {
    let theme = ctx.theme;
    let w = area.width as usize;
    let sep = Line::from(Span::styled("─".repeat(w), Style::default().fg(theme.dim)));

    let rows_capacity = usize::from(
        2 + plugin_block_rows(props.plugins.len()) + props.update_available.is_some() as u16,
    );
    let mut rows: Vec<Line> = Vec::with_capacity(rows_capacity);
    rows.push(sep);

    append_plugin_rows(
        &mut rows,
        ctx,
        PluginRowsProps {
            plugins: props.plugins,
            width: w,
        },
    );

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

    // The "menu" button replaces the old key hints; clicking it opens the
    // global context menu — the same one a right-click on empty space shows.
    let menu_label = "\u{2261} menu";
    let menu_y = area.y + rows.len() as u16;
    let menu = Some(Rect {
        x: area.x + 1,
        y: menu_y,
        width: menu_label.width() as u16,
        height: 1,
    });
    rows.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            menu_label,
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        ),
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

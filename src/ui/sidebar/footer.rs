use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::keybindings::{format_key, Command, Keybindings};
use crate::state::SidebarTab;
use crate::theme::Theme;
use crate::ui::style::{text_style, TextRole};
use crate::update::UpdateStatus;

use super::{menu_span, MENU_LABEL};

pub(super) struct FooterProps<'a> {
    pub update_available: Option<&'a UpdateStatus>,
    pub sidebar_active: bool,
    pub show_borders: bool,
    pub sidebar_tab: SidebarTab,
    pub keybindings: &'a Keybindings,
}

/// Click regions the footer publishes each frame: the update banner's
/// "upgrade" link and the pinned menu button.
#[derive(Default)]
pub(super) struct FooterHits {
    pub upgrade: Option<Rect>,
    pub menu: Option<Rect>,
}

struct FooterAction {
    key: String,
    label: &'static str,
}

/// A full-width horizontal rule in the footer's dim color.
fn divider_line(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.dim),
    ))
}

fn first_key(keybindings: &Keybindings, command: Command) -> Option<String> {
    keybindings.keys_for(command).first().map(format_key)
}

fn joined_nav_keys(keybindings: &Keybindings) -> Option<String> {
    let next = first_key(keybindings, Command::FocusNext)?;
    let prev = first_key(keybindings, Command::FocusPrev)?;
    Some(format!("{next}/{prev}"))
}

fn actions(props: &FooterProps<'_>) -> Vec<FooterAction> {
    if !props.sidebar_active {
        return first_key(props.keybindings, Command::ToggleFocus)
            .map(|key| {
                vec![FooterAction {
                    key,
                    label: "sidebar",
                }]
            })
            .unwrap_or_default();
    }

    let mut out = Vec::new();
    if let Some(key) = first_key(props.keybindings, Command::NewLocalSession) {
        out.push(FooterAction { key, label: "new" });
    }
    let help_keys = crate::ui::text::format_keys_for(props.keybindings, Command::ToggleHelp);
    if !help_keys.is_empty() {
        out.push(FooterAction {
            key: help_keys,
            label: "help",
        });
    }
    if let Some(key) = joined_nav_keys(props.keybindings) {
        out.push(FooterAction { key, label: "move" });
    }
    if let Some(key) = first_key(props.keybindings, Command::SwitchProject) {
        out.push(FooterAction {
            key,
            label: if props.sidebar_tab == SidebarTab::Agents {
                "focus"
            } else {
                "open"
            },
        });
    }
    out
}

pub(super) fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    props: FooterProps<'_>,
) -> FooterHits {
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

        // The `chosen` predicate above already guarantees the banner-plus-gap
        // layout fits, so `Some` implies the wide branch and the width check
        // only gates the bare-button fallback.
        let prefix_w = chosen.as_ref().map_or(0, |t| t.width() as u16 + gap);
        if chosen.is_some() || leading + upgrade_width <= area.width {
            upgrade_bounds = Some(Rect {
                x: area.x + leading + prefix_w,
                y: banner_row_y,
                width: upgrade_width,
                height: 1,
            });
            let mut spans = vec![Span::raw(" ")];
            if let Some(banner_text) = chosen {
                spans.push(Span::styled(banner_text, Style::default().fg(theme.dim)));
                spans.push(Span::raw("   "));
            }
            spans.push(Span::styled(
                upgrade_label.to_string(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ));
            rows.push(Line::from(spans));
        } else {
            rows.push(Line::default());
        }
    }

    let menu_width = MENU_LABEL.width() as u16;
    let menu_x = if area.width > menu_width {
        area.width - menu_width - 1
    } else {
        area.width
    };
    let hints_limit = menu_x.saturating_sub(1); // quiet cell before menu
    let mut hint_spans = vec![Span::styled(" ", Style::default().bg(theme.bg))];
    let mut cursor = 1u16;

    if !props.show_borders {
        let focus = if props.sidebar_active {
            "Sidebar"
        } else {
            "Terminal"
        };
        let width = focus.width() as u16;
        if cursor + width <= hints_limit {
            hint_spans.push(Span::styled(
                focus,
                text_style(theme, TextRole::NavigationActive).bg(theme.bg),
            ));
            cursor += width;
        }
    }

    for action in actions(&props) {
        let separator = if cursor > 1 { "  " } else { "" };
        let needed =
            separator.width() as u16 + action.key.width() as u16 + 1 + action.label.width() as u16;
        if cursor + needed > hints_limit {
            continue;
        }
        if !separator.is_empty() {
            hint_spans.push(Span::styled(
                separator,
                Style::default().fg(theme.dim).bg(theme.bg),
            ));
        }
        hint_spans.push(Span::styled(
            action.key,
            text_style(theme, TextRole::Shortcut).bg(theme.bg),
        ));
        hint_spans.push(Span::styled(
            format!(" {}", action.label),
            text_style(theme, TextRole::Hint).bg(theme.bg),
        ));
        cursor += needed;
    }

    let menu_y = area.y + rows.len() as u16;
    let menu = (menu_x < area.width).then(|| Rect {
        x: area.x + menu_x,
        y: menu_y,
        width: menu_width,
        height: 1,
    });
    if let Some(menu) = menu {
        let relative_x = menu.x - area.x;
        if relative_x > cursor {
            hint_spans.push(Span::styled(
                " ".repeat((relative_x - cursor) as usize),
                Style::default().bg(theme.bg),
            ));
        }
        hint_spans.push(menu_span(theme));
    }
    rows.push(Line::from(hint_spans));

    frame.render_widget(
        Paragraph::new(rows).style(Style::default().bg(theme.bg)),
        area,
    );

    FooterHits {
        upgrade: upgrade_bounds,
        menu,
    }
}

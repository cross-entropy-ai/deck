use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::geometry::TabRects;
use crate::state::SidebarTab;
use crate::theme::Theme;
use crate::ui::icons::{icon, Icon};
use crate::ui::style::{text_style, TextRole};

/// Click regions published by the responsive sidebar Header.
pub(super) struct HeaderHits {
    pub tabs: TabRects,
    pub sidebar_toggle: Rect,
}

struct HeaderLayout {
    projects: String,
    agents: String,
    gap: &'static str,
}

impl HeaderLayout {
    fn width(&self) -> usize {
        1 + self.projects.width() + self.gap.width() + self.agents.width() + 2 // quiet gap + collapse glyph
    }
}

/// Choose the richest Header that fits, progressively dropping decoration
/// without ever clipping the two tab hit targets.
fn responsive_layout(width: u16, projects: usize, agents: usize) -> HeaderLayout {
    let sessions_icon = icon(Icon::Sessions);
    let agents_icon = icon(Icon::Agents);
    let candidates = [
        HeaderLayout {
            projects: format!("{sessions_icon} Sessions {projects}"),
            agents: format!("{agents_icon} Agents {agents}"),
            gap: "   ",
        },
        HeaderLayout {
            projects: format!("Sessions {projects}"),
            agents: format!("Agents {agents}"),
            gap: "  ",
        },
        HeaderLayout {
            projects: format!("{sessions_icon} {projects}"),
            agents: format!("{agents_icon} {agents}"),
            gap: "  ",
        },
        HeaderLayout {
            projects: sessions_icon.to_string(),
            agents: agents_icon.to_string(),
            gap: " ",
        },
    ];

    candidates
        .into_iter()
        .find(|layout| layout.width() <= width as usize)
        .unwrap_or(HeaderLayout {
            projects: sessions_icon.to_string(),
            agents: String::new(),
            gap: "",
        })
}

/// Draw the Sessions / Agents selector with live counts. Local-session
/// creation lives on the local section's `+` button, avoiding a duplicate
/// action in the top-right header.
pub(super) fn draw_header(
    frame: &mut Frame,
    area: Rect,
    active: SidebarTab,
    project_count: usize,
    agent_count: usize,
    theme: &Theme,
) -> HeaderHits {
    frame.render_widget(
        Paragraph::new(vec![Line::raw(""), Line::raw("")]).style(Style::default().bg(theme.bg)),
        area,
    );

    let tab_style = |is_active: bool| {
        if is_active {
            text_style(theme, TextRole::NavigationActive).bg(theme.bg)
        } else {
            text_style(theme, TextRole::NavigationInactive).bg(theme.bg)
        }
    };

    let layout = responsive_layout(area.width, project_count, agent_count);
    let lead = 1u16;
    let projects_x = area.x + lead;
    let projects_w = layout.projects.width() as u16;
    let agents_x = projects_x + projects_w + layout.gap.width() as u16;
    let agents_w = layout.agents.width() as u16;

    let mut spans = vec![
        Span::styled(" ", Style::default().bg(theme.bg)),
        Span::styled(layout.projects, tab_style(active == SidebarTab::Projects)),
        Span::styled(layout.gap, Style::default().bg(theme.bg)),
        Span::styled(layout.agents, tab_style(active == SidebarTab::Agents)),
    ];

    let sidebar_toggle_x = area.right().saturating_sub(2);
    let used = agents_x + agents_w;
    spans.push(Span::styled(
        " ".repeat(sidebar_toggle_x.saturating_sub(used) as usize),
        Style::default().bg(theme.bg),
    ));
    spans.push(Span::styled(
        "‹",
        text_style(theme, TextRole::NavigationActive).bg(theme.bg),
    ));

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.bg)),
        area,
    );

    HeaderHits {
        tabs: TabRects {
            projects: Rect {
                x: projects_x,
                y: area.y,
                width: projects_w,
                height: 1,
            },
            agents: Rect {
                x: agents_x,
                y: area.y,
                width: agents_w,
                height: 1,
            },
        },
        sidebar_toggle: Rect {
            x: sidebar_toggle_x,
            y: area.y,
            width: 1,
            height: 1,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_width_keeps_counts() {
        let layout = responsive_layout(26, 8, 3);
        assert_eq!(layout.projects, "Sessions 8");
        assert_eq!(layout.agents, "Agents 3");
    }

    #[test]
    fn wide_width_uses_icons_and_labels() {
        let layout = responsive_layout(48, 8, 3);
        assert!(layout.projects.contains("Sessions 8"));
    }

    #[test]
    fn narrow_width_uses_icons_and_counts() {
        let layout = responsive_layout(14, 8, 3);
        assert_eq!(layout.projects, format!("{} 8", icon(Icon::Sessions)));
        assert_eq!(layout.agents, format!("{} 3", icon(Icon::Agents)));
        assert!(!layout.projects.contains("Sessions"));
        assert!(!layout.agents.contains("Agents"));
    }

    #[test]
    fn very_narrow_width_keeps_only_tab_icons() {
        let layout = responsive_layout(6, 128, 64);
        assert_eq!(layout.projects, icon(Icon::Sessions));
        assert_eq!(layout.agents, icon(Icon::Agents));
    }
}

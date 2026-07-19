use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::state::{SidebarTab, TabRects};
use crate::theme::Theme;

const PROJECTS_ICON: &str = "\u{e795}";
const AGENTS_ICON: &str = "\u{f085}";

/// Click regions published by the responsive sidebar Header.
pub(super) struct HeaderHits {
    pub tabs: TabRects,
    pub new_session: Option<Rect>,
}

struct HeaderLayout {
    projects: String,
    agents: String,
    gap: &'static str,
    new_label: Option<&'static str>,
}

impl HeaderLayout {
    fn width(&self) -> usize {
        1 + self.projects.width()
            + self.gap.width()
            + self.agents.width()
            + self.new_label.map_or(0, |label| 2 + label.width()) // quiet gap + trailing pad
    }
}

/// Choose the richest Header that fits: icons and a labelled action at wide
/// widths, counts plus a compact `+` at normal widths, then progressively drop
/// decoration without ever clipping the two tab hit targets.
fn responsive_layout(width: u16, projects: usize, agents: usize) -> HeaderLayout {
    let candidates = [
        HeaderLayout {
            projects: format!("{PROJECTS_ICON} Projects {projects}"),
            agents: format!("{AGENTS_ICON} Agents {agents}"),
            gap: "   ",
            new_label: Some("+ New"),
        },
        HeaderLayout {
            projects: format!("Projects {projects}"),
            agents: format!("Agents {agents}"),
            gap: "  ",
            new_label: Some("+"),
        },
        HeaderLayout {
            projects: format!("{PROJECTS_ICON} {projects}"),
            agents: format!("{AGENTS_ICON} {agents}"),
            gap: "  ",
            new_label: Some("+"),
        },
        HeaderLayout {
            projects: PROJECTS_ICON.to_string(),
            agents: AGENTS_ICON.to_string(),
            gap: " ",
            new_label: None,
        },
    ];

    candidates
        .into_iter()
        .find(|layout| layout.width() <= width as usize)
        .unwrap_or(HeaderLayout {
            projects: PROJECTS_ICON.to_string(),
            agents: String::new(),
            gap: "",
            new_label: None,
        })
}

/// Draw the Projects / Agents selector with live counts and a responsive local
/// new-session action. The action is right-aligned when it fits, so it stays a
/// stable primary target while the tab labels consume the left side.
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
            Style::default()
                .fg(theme.accent)
                .bg(theme.bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dim).bg(theme.bg)
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

    let new_session = layout.new_label.map(|label| {
        let label_w = label.width() as u16;
        let x = area.right().saturating_sub(label_w + 1);
        let used = agents_x + agents_w;
        spans.push(Span::styled(
            " ".repeat(x.saturating_sub(used) as usize),
            Style::default().bg(theme.bg),
        ));
        spans.push(Span::styled(
            label,
            Style::default()
                .fg(theme.accent)
                .bg(theme.bg)
                .add_modifier(Modifier::BOLD),
        ));
        Rect {
            x,
            y: area.y,
            width: label_w,
            height: 1,
        }
    });

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
        new_session,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_width_keeps_counts_and_compact_new_action() {
        let layout = responsive_layout(26, 8, 3);
        assert_eq!(layout.projects, "Projects 8");
        assert_eq!(layout.agents, "Agents 3");
        assert_eq!(layout.new_label, Some("+"));
    }

    #[test]
    fn wide_width_uses_icons_and_labelled_new_action() {
        let layout = responsive_layout(48, 8, 3);
        assert!(layout.projects.contains("Projects 8"));
        assert_eq!(layout.new_label, Some("+ New"));
    }

    #[test]
    fn narrow_width_uses_icons_counts_and_compact_new_action() {
        let layout = responsive_layout(14, 8, 3);
        assert_eq!(layout.projects, format!("{PROJECTS_ICON} 8"));
        assert_eq!(layout.agents, format!("{AGENTS_ICON} 3"));
        assert_eq!(layout.new_label, Some("+"));
        assert!(!layout.projects.contains("Projects"));
        assert!(!layout.agents.contains("Agents"));
    }

    #[test]
    fn very_narrow_width_keeps_only_tab_icons() {
        let layout = responsive_layout(6, 128, 64);
        assert_eq!(layout.projects, PROJECTS_ICON);
        assert_eq!(layout.agents, AGENTS_ICON);
        assert_eq!(layout.new_label, None);
    }
}

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use ratatui_sectioned_list::{layout_divider, DividerLayoutSpec};
use unicode_width::UnicodeWidthStr;

use crate::state::{
    AgentHit, AgentRow, AgentTarget, DividerButton, DividerHit, FocusTarget, HostStatus, PfBadge,
    PfBadgeColor, SidebarItemData, SidebarLayout, SummaryState, ViewMode,
};
use crate::theme::Theme;

/// Braille spinner frames for the Summary card's "Generating…" state.
pub(super) const SUMMARY_SPINNER: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

use super::SummaryHits;

use super::super::text::{
    format_idle_badge, idle_color, md_line_spans, md_line_width, pad_line, shorten_dir, truncate,
    wrap_markdown,
};
use super::super::widgets::scrollbar_cells;
use super::super::{SessionOrigin, SidebarSession};
use super::SidebarRenderCtx;

pub(super) struct SessionsProps<'a> {
    pub sessions: &'a [&'a dyn SidebarSession],
    pub layout: &'a SidebarLayout,
    pub focus_target: Option<FocusTarget>,
    pub view_mode: ViewMode,
    pub active_agent: Option<&'a AgentTarget>,
    /// Flattened agent list for the Agents tab; `Agent { row_idx }` items
    /// index into this. Empty on the Projects tab.
    pub agent_rows: &'a [AgentRow],
    /// State of the Agents-tab Summary card.
    pub summary: &'a SummaryState,
    /// Precomputed "Xm ago" age of the Ready summary, `None` otherwise.
    pub summary_age: Option<&'a str>,
    /// Current braille spinner frame index for the "Generating…" state.
    pub spinner_idx: usize,
    /// Scroll offset (wrapped rows) into the Ready summary text.
    pub summary_scroll: usize,
}

#[derive(Clone, Copy)]
struct RowChrome {
    is_focused: bool,
    bg: Color,
    gutter_bg: Color,
    width: usize,
}

struct SessionCardProps<'a> {
    session: &'a dyn SidebarSession,
    session_idx: usize,
    chrome: RowChrome,
    target_height: usize,
}

pub(super) fn draw_sessions(
    frame: &mut Frame,
    area: Rect,
    ctx: &SidebarRenderCtx<'_>,
    props: SessionsProps<'_>,
) -> (Vec<DividerHit>, Vec<AgentHit>, SummaryHits) {
    if props.sessions.is_empty() {
        frame.render_widget(
            Paragraph::new("  No projects")
                .style(Style::default().fg(ctx.theme.muted).bg(ctx.theme.bg)),
            area,
        );
        return (Vec::new(), Vec::new(), SummaryHits::default());
    }

    let width = area.width as usize;
    frame.render_widget(
        Block::default().style(Style::default().bg(ctx.theme.bg)),
        area,
    );

    let collapsible = props.layout.is_collapsible();
    let focus_idx = props.focus_target.map(|f| f.0);
    let scroll = props.layout.scroll_offset(focus_idx, area.height);
    let mut summary = SummaryHits::default();
    let mut hits = Vec::new();
    let mut agent_hits = Vec::new();

    for visible in props.layout.visible_items(scroll, area.height) {
        let item = visible.item;
        let is_focused = visible.row_idx == focus_idx;
        let row_bg = if is_focused {
            ctx.theme.surface
        } else {
            ctx.theme.bg
        };
        let chrome = RowChrome {
            is_focused,
            bg: row_bg,
            gutter_bg: ctx.theme.bg,
            width,
        };
        let mut lines: Vec<Line> = Vec::new();
        match &item.data {
            SidebarItemData::LocalHeader => {
                let collapsed = collapsible && item.collapsed;
                let (_, actions) = render_divider_line(
                    &mut lines,
                    width,
                    ctx.theme,
                    "@local",
                    ctx.theme.accent,
                    collapsed,
                    None,
                    &[(
                        "[\u{2026}]",
                        Style::default().fg(ctx.theme.accent).bg(ctx.theme.bg),
                    )],
                );
                if let Some(y) = visible.viewport_y_for_item_line(0) {
                    hits.push(divider_hit_at(
                        area,
                        y,
                        actions[0].clone(),
                        String::new(),
                        DividerButton::LocalMore,
                    ));
                }
            }
            SidebarItemData::Agent { row_idx } => {
                let Some(row) = props.agent_rows.get(*row_idx) else {
                    continue;
                };
                let a = &row.agent;
                // "you are here" = the pane deck last switched to.
                let here = props
                    .active_agent
                    .is_some_and(|t| t.host == row.host && t.pane_id == a.pane_id);
                let name_fg = if is_focused {
                    ctx.theme.text
                } else if here {
                    ctx.theme.green
                } else {
                    ctx.theme.secondary
                };
                let mut name_style = Style::default().fg(name_fg).bg(row_bg);
                if is_focused || here {
                    name_style = name_style.add_modifier(Modifier::BOLD);
                }
                let accent = if is_focused || here {
                    ctx.theme.green
                } else {
                    row_bg
                };
                // Status dot before the name: red = working, green = idle,
                // yellow = waiting for input, gray = unknown.
                let status_color = match a.status {
                    crate::agent::AgentStatus::Working => ctx.theme.pink,
                    crate::agent::AgentStatus::Idle => ctx.theme.green,
                    crate::agent::AgentStatus::Waiting => ctx.theme.yellow,
                    crate::agent::AgentStatus::Unknown => ctx.theme.dim,
                };
                let label = a.location();
                lines.push(pad_line(
                    vec![
                        Span::styled(
                            if is_focused || here { "▌" } else { " " },
                            Style::default().fg(accent).bg(row_bg),
                        ),
                        Span::styled(" ", Style::default().bg(row_bg)),
                        Span::styled("\u{2022}", Style::default().fg(status_color).bg(row_bg)),
                        Span::styled(" ", Style::default().bg(row_bg)),
                        Span::styled(truncate(&label, width.saturating_sub(4)), name_style),
                    ],
                    row_bg,
                    width,
                ));
                if let Some(y) = visible.viewport_y_for_item_line(0) {
                    agent_hits.push(AgentHit {
                        target: AgentTarget {
                            host: row.host.clone(),
                            session: a.session.clone(),
                            pane_id: a.pane_id.clone(),
                        },
                        rect: Rect {
                            x: area.x,
                            y: area.y + y,
                            width: area.width,
                            height: 1,
                        },
                    });
                }
            }
            SidebarItemData::Spacer => {
                lines.push(pad_line(Vec::new(), ctx.theme.bg, width));
            }
            SidebarItemData::SummaryCard => {
                // Title row. Whenever the card isn't mid-generation it carries
                // a right-aligned Generate button; in the Ready state the button
                // is preceded by the text's "Xm ago" age. The button reuses
                // `summary.button` for hit-testing + the GenerateSummary action.
                let title_line = lines.len() as u16;
                let left = " \u{f0eb} Summary";
                let mut title_spans = vec![
                    Span::styled(" ", Style::default().bg(ctx.theme.bg)),
                    Span::styled(
                        "\u{f0eb} Summary",
                        Style::default()
                            .fg(ctx.theme.accent)
                            .bg(ctx.theme.bg)
                            .add_modifier(Modifier::BOLD),
                    ),
                ];
                if !matches!(props.summary, SummaryState::Generating) {
                    let is_ready = matches!(props.summary, SummaryState::Ready { .. });
                    let gen_label = " \u{21bb} Generate ";
                    let gen_w = gen_label.width();
                    // The popup (big view) button only makes sense once
                    // there's a summary to open.
                    let popup_label = " \u{f065} ";
                    let popup_w = if is_ready { popup_label.width() } else { 0 };
                    // The age only exists once a summary has been generated.
                    let age = if is_ready {
                        props.summary_age.unwrap_or("")
                    } else {
                        ""
                    };
                    let left_w = left.width();
                    let buttons_w = gen_w + popup_w;
                    // Right group is "<age> <Generate><popup>"; drop the age
                    // first when the row is too tight to fit it all.
                    let show_age =
                        !age.is_empty() && left_w + 1 + age.width() + 1 + buttons_w <= width;
                    let right_w = if show_age {
                        age.width() + 1 + buttons_w
                    } else {
                        buttons_w
                    };
                    let filler = width.saturating_sub(left_w + right_w).max(1);
                    title_spans.push(Span::styled(
                        " ".repeat(filler),
                        Style::default().bg(ctx.theme.bg),
                    ));
                    if show_age {
                        title_spans.push(Span::styled(
                            format!("{age} "),
                            Style::default().fg(ctx.theme.muted).bg(ctx.theme.bg),
                        ));
                    }
                    title_spans.push(Span::styled(
                        gen_label,
                        Style::default()
                            .fg(ctx.theme.bg)
                            .bg(ctx.theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ));
                    if is_ready {
                        title_spans.push(Span::styled(
                            popup_label,
                            Style::default()
                                .fg(ctx.theme.bg)
                                .bg(ctx.theme.teal)
                                .add_modifier(Modifier::BOLD),
                        ));
                    }
                    // Buttons hug the right edge: Generate then popup.
                    let gen_x = width.saturating_sub(buttons_w) as u16;
                    let popup_x = width.saturating_sub(popup_w) as u16;
                    if let Some(y) = visible.viewport_y_for_item_line(title_line) {
                        summary.button = Some(Rect {
                            x: area.x + gen_x,
                            y: area.y + y,
                            width: (gen_w as u16).min(area.width.saturating_sub(gen_x)),
                            height: 1,
                        });
                        if is_ready {
                            summary.popup = Some(Rect {
                                x: area.x + popup_x,
                                y: area.y + y,
                                width: (popup_w as u16).min(area.width.saturating_sub(popup_x)),
                                height: 1,
                            });
                        }
                    }
                }
                lines.push(pad_line(title_spans, ctx.theme.bg, width));
                lines.push(pad_line(Vec::new(), ctx.theme.bg, width));

                match props.summary {
                    SummaryState::Idle => {
                        // No summary yet — a dim hint; the Generate button to
                        // the right of the title kicks the first run.
                        lines.push(pad_line(
                            vec![Span::styled(
                                "  No aggregated agent summary yet",
                                Style::default().fg(ctx.theme.muted).bg(ctx.theme.bg),
                            )],
                            ctx.theme.bg,
                            width,
                        ));
                    }
                    SummaryState::Generating => {
                        let spinner = SUMMARY_SPINNER[props.spinner_idx % SUMMARY_SPINNER.len()];
                        lines.push(pad_line(
                            vec![Span::styled(
                                format!("  {spinner} Generating …"),
                                Style::default().fg(ctx.theme.yellow).bg(ctx.theme.bg),
                            )],
                            ctx.theme.bg,
                            width,
                        ));
                    }
                    SummaryState::Ready { text, .. } => {
                        // Scrollable text window with a scrollbar gutter on
                        // the right; markdown `**bold**` renders bold. Height
                        // is the drag-set body rows (card height minus chrome).
                        let rows = (item.height as usize).saturating_sub(3);
                        let content_w = width.saturating_sub(3); // 2 indent + 1 bar
                        let wrapped = wrap_markdown(text, content_w.max(1));
                        let total = wrapped.len();
                        summary.max_scroll = total.saturating_sub(rows);
                        let scroll = props.summary_scroll.min(summary.max_scroll);
                        let bar = scrollbar_cells(rows, total, scroll);
                        let base = Style::default().fg(ctx.theme.text).bg(ctx.theme.bg);
                        for i in 0..rows {
                            let mut spans =
                                vec![Span::styled("  ", Style::default().bg(ctx.theme.bg))];
                            let line_w = match wrapped.get(scroll + i) {
                                Some(runs) => {
                                    spans.extend(md_line_spans(runs, ctx.theme, base));
                                    md_line_width(runs)
                                }
                                None => 0,
                            };
                            if line_w < content_w {
                                spans.push(Span::styled(
                                    " ".repeat(content_w - line_w),
                                    Style::default().bg(ctx.theme.bg),
                                ));
                            }
                            if let Some(glyph) = bar.get(i).copied().flatten() {
                                spans.push(Span::styled(
                                    glyph,
                                    Style::default().fg(ctx.theme.dim).bg(ctx.theme.bg),
                                ));
                            }
                            lines.push(pad_line(spans, ctx.theme.bg, width));
                        }
                    }
                    SummaryState::Error(msg) => {
                        // Wrap the failure reason into the body rows, no
                        // scrollbar; the Generate button stays up to retry.
                        // Errors are plain text — render the whole line pink.
                        let rows = (item.height as usize).saturating_sub(3);
                        let content_w = width.saturating_sub(2);
                        let wrapped = wrap_markdown(msg, content_w.max(1));
                        for i in 0..rows {
                            let mut spans =
                                vec![Span::styled("  ", Style::default().bg(ctx.theme.bg))];
                            if let Some(runs) = wrapped.get(i) {
                                let text: String = runs.iter().map(|(s, _)| s.as_str()).collect();
                                spans.push(Span::styled(
                                    text,
                                    Style::default().fg(ctx.theme.pink).bg(ctx.theme.bg),
                                ));
                            }
                            lines.push(pad_line(spans, ctx.theme.bg, width));
                        }
                    }
                }
                // Fill to one short of the card height, then a dim drag-handle
                // grip as the bottom row (the resize hit-region).
                while lines.len() + 1 < item.height as usize {
                    lines.push(pad_line(Vec::new(), ctx.theme.bg, width));
                }
                let grip = "\u{254c}\u{254c}\u{254c}\u{254c}\u{254c}\u{254c}";
                let grip_pad = width.saturating_sub(grip.width()) / 2;
                lines.push(pad_line(
                    vec![
                        Span::styled(" ".repeat(grip_pad), Style::default().bg(ctx.theme.bg)),
                        Span::styled(grip, Style::default().fg(ctx.theme.dim).bg(ctx.theme.bg)),
                    ],
                    ctx.theme.bg,
                    width,
                ));
                summary.card = Some(Rect {
                    x: area.x,
                    y: area.y + visible.viewport_y,
                    width: area.width,
                    height: visible.visible_height,
                });
            }
            SidebarItemData::AgentsPlaceholder { detecting } => {
                let text = if *detecting {
                    "    detecting…"
                } else {
                    "    no agents"
                };
                lines.push(pad_line(
                    vec![Span::styled(
                        text,
                        Style::default().fg(ctx.theme.muted).bg(ctx.theme.bg),
                    )],
                    ctx.theme.bg,
                    width,
                ));
            }
            SidebarItemData::LocalEmpty => {
                lines.push(pad_line(
                    vec![Span::styled(
                        format!("  {}", crate::state::REMOTE_NO_SESSIONS_LABEL),
                        Style::default().fg(ctx.theme.muted).bg(ctx.theme.bg),
                    )],
                    ctx.theme.bg,
                    width,
                ));
                while lines.len() < item.height as usize {
                    lines.push(pad_line(Vec::new(), ctx.theme.bg, width));
                }
            }
            SidebarItemData::Header {
                host,
                host_idx,
                status,
                pf,
            } => {
                let accent = host_accent(ctx.theme, *host_idx);
                let label = format!("@{host}");
                let collapsed = collapsible && item.collapsed;
                let GroupHeaderHits {
                    reconnect: reconnect_range,
                    more: more_range,
                    badge: badge_range,
                } = render_group_header(
                    &mut lines, &label, accent, *status, width, ctx.theme, *pf, collapsed,
                );
                if let Some(y) = visible.viewport_y_for_item_line(0) {
                    if let Some(badge_range) = badge_range {
                        hits.push(divider_hit_at(
                            area,
                            y,
                            badge_range,
                            host.clone(),
                            DividerButton::PfBadge,
                        ));
                    }
                    hits.push(divider_hit_at(
                        area,
                        y,
                        reconnect_range,
                        host.clone(),
                        DividerButton::Reconnect,
                    ));
                    hits.push(divider_hit_at(
                        area,
                        y,
                        more_range,
                        host.clone(),
                        DividerButton::More,
                    ));
                }
            }
            SidebarItemData::Session { session_idx } => {
                let Some(&session) = props.sessions.get(*session_idx) else {
                    continue;
                };
                let card = SessionCardProps {
                    session,
                    session_idx: *session_idx,
                    chrome,
                    target_height: item.height as usize,
                };
                match props.view_mode {
                    ViewMode::Expanded => render_session_card_expanded(&mut lines, ctx, card),
                    ViewMode::Compact => render_session_card_compact(&mut lines, ctx, card),
                }
            }
        }

        let start = visible.item_y_offset as usize;
        let end = start + visible.visible_height as usize;
        let rendered_lines = if start == 0 && end >= lines.len() {
            lines
        } else {
            lines
                .get(start..end.min(lines.len()))
                .map_or_else(Vec::new, <[_]>::to_vec)
        };
        if rendered_lines.is_empty() {
            continue;
        }
        frame.render_widget(
            Paragraph::new(rendered_lines).style(Style::default().bg(ctx.theme.bg)),
            Rect {
                x: area.x,
                y: area.y + visible.viewport_y,
                width: area.width,
                height: visible.visible_height,
            },
        );
    }

    (hits, agent_hits, summary)
}

fn divider_hit_at(
    area: Rect,
    viewport_y: u16,
    col_range: std::ops::Range<usize>,
    host: String,
    kind: DividerButton,
) -> DividerHit {
    DividerHit {
        host,
        kind,
        rect: Rect {
            x: area.x + col_range.start as u16,
            y: area.y + viewport_y,
            width: (col_range.end - col_range.start) as u16,
            height: 1,
        },
    }
}

fn host_accent(theme: &Theme, host_idx: usize) -> Color {
    let tints = [theme.teal, theme.pink, theme.yellow, theme.accent];
    tints[host_idx % tints.len()]
}

pub(super) struct GroupHeaderHits {
    pub(super) reconnect: std::ops::Range<usize>,
    pub(super) more: std::ops::Range<usize>,
    pub(super) badge: Option<std::ops::Range<usize>>,
}

#[allow(clippy::too_many_arguments)]
fn render_divider_line(
    lines: &mut Vec<Line<'_>>,
    width: usize,
    theme: &Theme,
    label: &str,
    accent: Color,
    collapsed: bool,
    badge: Option<(String, Style)>,
    actions: &[(&'static str, Style)],
) -> (Option<std::ops::Range<usize>>, Vec<std::ops::Range<usize>>) {
    let leading = " ";
    let chevron = collapse_chevron(collapsed);
    let layout = layout_divider(DividerLayoutSpec {
        width,
        leading_width: leading.width(),
        chevron_width: chevron.width() + 1,
        spacer_width: 1,
        gap_width: 1,
        label_width: label.width(),
        badge_width: badge.as_ref().map(|(text, _)| text.as_str().width()),
        action_widths: actions.iter().map(|(glyph, _)| glyph.width()).collect(),
    });
    let label_text = truncate(label, layout.label_width);
    let rule = "\u{2500}".repeat(layout.rule_width);
    let mut spans = vec![
        Span::styled(leading, Style::default().bg(theme.bg)),
        Span::styled(
            format!("{chevron} "),
            Style::default().fg(accent).bg(theme.bg),
        ),
        Span::styled(
            label_text,
            Style::default()
                .fg(accent)
                .bg(theme.bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().bg(theme.bg)),
        Span::styled(rule, Style::default().fg(accent).bg(theme.bg)),
    ];
    if layout.badge.is_some() {
        if let Some((text, style)) = badge {
            spans.push(Span::styled(" ", Style::default().bg(theme.bg)));
            spans.push(Span::styled(text, style));
        }
    }
    for (glyph, style) in actions {
        spans.push(Span::styled(" ", Style::default().bg(theme.bg)));
        spans.push(Span::styled(*glyph, *style));
    }
    lines.push(pad_line(spans, theme.bg, width));

    (layout.badge, layout.actions)
}

fn collapse_chevron(collapsed: bool) -> &'static str {
    if collapsed {
        "\u{25b8}"
    } else {
        "\u{25be}"
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_group_header(
    lines: &mut Vec<Line<'_>>,
    label: &str,
    accent: Color,
    status: HostStatus,
    width: usize,
    theme: &Theme,
    pf: Option<PfBadge>,
    collapsed: bool,
) -> GroupHeaderHits {
    let badge_text = pf.map(|b| format!("\u{21c4}{}", b.count));
    let badge_fg = pf.map(|b| match b.color {
        PfBadgeColor::Healthy => theme.green,
        PfBadgeColor::Degraded => theme.pink,
        PfBadgeColor::Probing => theme.yellow,
    });

    let reconnect_fg = match status {
        HostStatus::Connected => accent,
        HostStatus::Connecting => theme.yellow,
        HostStatus::Unreachable => theme.pink,
    };
    let (badge, actions) = render_divider_line(
        lines,
        width,
        theme,
        label.trim_start(),
        accent,
        collapsed,
        badge_text
            .zip(badge_fg)
            .map(|(text, fg)| (text, Style::default().fg(fg).bg(theme.bg))),
        &[
            ("[\u{27f3}]", Style::default().fg(reconnect_fg).bg(theme.bg)),
            ("[\u{2026}]", Style::default().fg(accent).bg(theme.bg)),
        ],
    );

    GroupHeaderHits {
        reconnect: actions[0].clone(),
        more: actions[1].clone(),
        badge,
    }
}

fn idx_hint(session: &dyn SidebarSession, session_idx: usize) -> String {
    match session.origin() {
        SessionOrigin::Local => format!("{:>2}", session_idx + 1),
        SessionOrigin::Remote { .. } => "  ".to_string(),
    }
}

struct RowHead {
    accent_span: (&'static str, Style),
    name_style: Style,
    index_style: Style,
    label: String,
}

fn row_head(theme: &Theme, session: &dyn SidebarSession, is_focused: bool, bg: Color) -> RowHead {
    let loading = session.loading();
    let unreachable = session.unreachable();
    let name_fg = if loading || unreachable {
        theme.muted
    } else if is_focused {
        theme.text
    } else {
        theme.secondary
    };
    let mut name_style = Style::default().fg(name_fg).bg(bg);
    if is_focused && !loading {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    RowHead {
        accent_span: (
            if is_focused { "▌" } else { " " },
            Style::default()
                .fg(if is_focused { theme.green } else { bg })
                .bg(bg),
        ),
        name_style,
        index_style: if is_focused {
            Style::default().fg(theme.secondary).bg(bg)
        } else {
            Style::default().fg(theme.dim).bg(bg)
        },
        label: if loading {
            "(connecting…)".to_string()
        } else {
            session.name().to_string()
        },
    }
}

fn render_session_card_expanded(
    lines: &mut Vec<Line<'_>>,
    ctx: &SidebarRenderCtx<'_>,
    props: SessionCardProps<'_>,
) {
    let session = props.session;
    let RowChrome {
        is_focused,
        bg,
        gutter_bg,
        width,
    } = props.chrome;
    let theme = ctx.theme;
    let activity = session.activity();
    let head = row_head(theme, session, is_focused, bg);

    let text_width = width.saturating_sub(6);
    let name_display = truncate(&head.label, text_width);
    let before = lines.len();
    lines.push(pad_line(
        vec![
            Span::styled(head.accent_span.0, head.accent_span.1),
            Span::styled(idx_hint(session, props.session_idx), head.index_style),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(name_display, head.name_style),
        ],
        bg,
        width,
    ));

    let dir_text = if session.loading() || session.dir().is_empty() {
        String::new()
    } else {
        truncate(&shorten_dir(session.dir()), text_width.saturating_sub(2)).to_string()
    };
    let dir_color = if is_focused { theme.teal } else { theme.muted };
    let (badge_text, badge_color) = match activity {
        Some(a) => {
            let badge = format_idle_badge(a.idle_seconds)
                .map(|t| format!("{t:^6}"))
                .unwrap_or_else(|| " ".repeat(6));
            (badge, idle_color(theme, a.idle_seconds, is_focused))
        }
        None => (" ".repeat(6), theme.dim),
    };
    lines.push(pad_line(
        vec![
            Span::styled(badge_text, Style::default().fg(badge_color).bg(bg)),
            Span::styled(dir_text, Style::default().fg(dir_color).bg(bg)),
        ],
        bg,
        width,
    ));

    while lines.len() - before < props.target_height.saturating_sub(1) {
        lines.push(pad_line(
            vec![Span::styled(" ", Style::default().bg(bg))],
            bg,
            width,
        ));
    }
    if lines.len() - before < props.target_height {
        lines.push(pad_line(
            vec![Span::styled(" ", Style::default().bg(gutter_bg))],
            gutter_bg,
            width,
        ));
    }
}

fn compact_label(session: &dyn SidebarSession) -> String {
    let prefix = match session.origin() {
        SessionOrigin::Local => "local",
        SessionOrigin::Remote { host } => host,
    };
    if session.loading() {
        format!("{prefix}:(connecting…)")
    } else {
        format!("{prefix}:{}", session.name())
    }
}

fn render_session_card_compact(
    lines: &mut Vec<Line<'_>>,
    ctx: &SidebarRenderCtx<'_>,
    props: SessionCardProps<'_>,
) {
    let session = props.session;
    let RowChrome {
        is_focused,
        bg,
        width,
        ..
    } = props.chrome;
    let theme = ctx.theme;
    let head = row_head(theme, session, is_focused, bg);

    let label = compact_label(session);
    lines.push(pad_line(
        vec![
            Span::styled(head.accent_span.0, head.accent_span.1),
            Span::styled(idx_hint(session, props.session_idx), head.index_style),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(truncate(&label, width.saturating_sub(6)), head.name_style),
        ],
        bg,
        width,
    ));
}

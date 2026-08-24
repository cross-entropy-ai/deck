use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::geometry::SummaryHits;
use crate::summary_card::SummaryState;
use crate::theme::Theme;
use crate::ui::icons::{icon, Icon};

use super::super::text::pad_line;
use super::super::widgets::markdown_window;

/// Braille spinner frames for the Summary card's "Generating…" state.
pub(super) const SUMMARY_SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(super) struct SummaryCardProps<'a> {
    pub summary: &'a SummaryState,
    /// Precomputed "Xm ago" age of the Ready summary, `None` otherwise.
    pub summary_age: Option<&'a str>,
    /// Current braille spinner frame index for the generating state.
    pub spinner_idx: usize,
    /// Scroll offset (wrapped rows) into the Ready summary text.
    pub summary_scroll: usize,
    /// Whether at least one real agent pane is available to capture.
    pub can_generate: bool,
}

/// Draw the Agents-tab Summary card into its own rect, pinned above the
/// list. Returns the card's click/scroll regions.
pub(super) fn draw_summary_card(
    frame: &mut Frame,
    rect: Rect,
    theme: &Theme,
    props: SummaryCardProps<'_>,
) -> SummaryHits {
    let width = rect.width as usize;
    let mut summary = SummaryHits {
        card: Some(rect),
        ..SummaryHits::default()
    };
    frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), rect);

    let mut lines: Vec<Line> = Vec::new();

    // Top row: a centered dim drag grip — the card's top edge is its resize
    // boundary now that it's pinned to the bottom (the list sits above it).
    let grip = "╌╌╌╌╌╌";
    let grip_pad = width.saturating_sub(grip.width()) / 2;
    lines.push(pad_line(
        vec![
            Span::styled(" ".repeat(grip_pad), Style::default().bg(theme.bg)),
            Span::styled(grip, Style::default().fg(theme.dim).bg(theme.bg)),
        ],
        theme.bg,
        width,
    ));

    // Title row: "Summary", plus a right-aligned Generate button (and the
    // text's "Xm ago" age + a popup button once Ready).
    let summary_icon = icon(Icon::Summary);
    let left = format!(" {summary_icon} Summary");
    let mut title_spans = vec![
        Span::styled(" ", Style::default().bg(theme.bg)),
        Span::styled(
            format!("{summary_icon} Summary"),
            Style::default()
                .fg(theme.accent)
                .bg(theme.bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if !matches!(props.summary, SummaryState::Generating) {
        let is_ready = matches!(props.summary, SummaryState::Ready { .. });
        let gen_label = " ↻ Generate ";
        let gen_w = gen_label.width();
        let popup_label = format!(" {} ", icon(Icon::Open));
        let popup_w = if is_ready { popup_label.width() } else { 0 };
        let age = if is_ready {
            props.summary_age.unwrap_or("")
        } else {
            ""
        };
        let left_w = left.width();
        let buttons_w = gen_w + popup_w;
        let show_age = !age.is_empty() && left_w + 1 + age.width() + 1 + buttons_w <= width;
        let right_w = if show_age {
            age.width() + 1 + buttons_w
        } else {
            buttons_w
        };
        let filler = width.saturating_sub(left_w + right_w).max(1);
        title_spans.push(Span::styled(
            " ".repeat(filler),
            Style::default().bg(theme.bg),
        ));
        if show_age {
            title_spans.push(Span::styled(
                format!("{age} "),
                Style::default().fg(theme.muted).bg(theme.bg),
            ));
        }
        let generate_style = if props.can_generate {
            Style::default()
                .fg(theme.bg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted).bg(theme.surface)
        };
        title_spans.push(Span::styled(gen_label, generate_style));
        if is_ready {
            title_spans.push(Span::styled(
                popup_label,
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.teal)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        // Derive each button's column from the running offset (matching the
        // drawn spans `left + filler + [age] + Generate + [popup]`), clamped to
        // card width so a narrow card never reports a rect past its right edge.
        let age_run = if show_age { age.width() + 1 } else { 0 };
        let gen_x = (left_w + filler + age_run) as u16;
        let popup_x = gen_x + gen_w as u16;
        let btn = |x: u16, w: usize| Rect {
            x: rect.x + x,
            y: rect.y + 1,
            width: (w as u16).min(rect.width.saturating_sub(x)),
            height: 1,
        };
        summary.button = props.can_generate.then(|| btn(gen_x, gen_w));
        summary.popup = is_ready.then(|| btn(popup_x, popup_w));
    }
    lines.push(pad_line(title_spans, theme.bg, width));
    let compact_unavailable = !props.can_generate && matches!(props.summary, SummaryState::Idle);
    if !compact_unavailable {
        lines.push(pad_line(Vec::new(), theme.bg, width));
    }

    // Body: a fixed-height window whose rows = card height minus the title,
    // blank, and drag-handle chrome.
    let chrome_rows = if compact_unavailable { 2 } else { 3 };
    let rows = (rect.height as usize).saturating_sub(chrome_rows);
    match props.summary {
        SummaryState::Idle => {
            lines.push(pad_line(
                vec![Span::styled(
                    if compact_unavailable {
                        "  No agents detected"
                    } else {
                        "  No summary generated yet"
                    },
                    Style::default().fg(theme.muted).bg(theme.bg),
                )],
                theme.bg,
                width,
            ));
        }
        SummaryState::Generating => {
            let spinner = SUMMARY_SPINNER[props.spinner_idx % SUMMARY_SPINNER.len()];
            lines.push(pad_line(
                vec![Span::styled(
                    format!("  {spinner} Generating …"),
                    Style::default().fg(theme.yellow).bg(theme.bg),
                )],
                theme.bg,
                width,
            ));
        }
        SummaryState::Ready { text, .. } | SummaryState::Error(text) => {
            // Error text is non-scrolling (no scroll state): render it red,
            // pinned at the top, and don't publish a scroll range for it.
            let is_err = matches!(props.summary, SummaryState::Error(_));
            let fg = if is_err { theme.error } else { theme.text };
            let scroll = if is_err { 0 } else { props.summary_scroll };
            let content_w = width.saturating_sub(3); // 2 indent + 1 scrollbar
            let (row_spans, max_scroll) =
                markdown_window(text, rows, scroll, content_w, theme, fg, theme.bg);
            if !is_err {
                summary.max_scroll = max_scroll;
            }
            for spans in row_spans {
                let mut row = vec![Span::styled("  ", Style::default().bg(theme.bg))];
                row.extend(spans);
                lines.push(pad_line(row, theme.bg, width));
            }
        }
    }

    // Pad out the remaining card rows below the body.
    while lines.len() < rect.height as usize {
        lines.push(pad_line(Vec::new(), theme.bg, width));
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        rect,
    );
    summary
}

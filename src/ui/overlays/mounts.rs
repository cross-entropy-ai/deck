//! The mount picker: what a lane could mount, as reported by its system.
//!
//! Deliberately the same surface as the add-remote picker (a filter input over a
//! candidate list) so the two read as one idiom, plus two states add-remote never
//! has: a list still being fetched over the network, and a candidate that needs a
//! side effect outside Deck before it can be mounted.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::geometry::{ListItemHit, MountHits};
use crate::overlay::{MountBusy, MountPickerState};
use crate::theme::Theme;
use crate::ui::widgets::{
    field_row, hint_rect, list_item_line, modal_footer, modal_list_lines_windowed, ListViewport,
    ModalFrame, TextAreaColors,
};

const OVERLAY_WIDTH: u16 = 56;
/// The keys row of the footer, whose hints double as the picker's buttons.
const MOUNT_HINT: &str = "\u{23ce} mount";
const SORT_HINT: &str = "\u{21e5} sort";
const CANCEL_HINT: &str = "\u{238b} cancel";
const MOUNT_FOOTER: &str =
    " \u{23ce} mount \u{b7} \u{2191}\u{2193} select \u{b7} \u{21e5} sort \u{b7} \u{238b} cancel";
/// Rows of candidate list, before the input and footer.
const BODY_ROWS: u16 = 10;
/// Footer rows: keys, then the state of the list. Two short rows rather than
/// one long one — the hint glyphs are ambiguous-width and `modal_footer` clips
/// without wrapping, so a single row silently loses its tail.
const FOOTER_ROWS: u16 = 2;

pub fn draw_mount_picker(
    frame: &mut Frame,
    area: Rect,
    picker: &MountPickerState,
    lane_title: &str,
    theme: &Theme,
) -> MountHits {
    let width = OVERLAY_WIDTH.min(area.width.saturating_sub(4));
    // input + pad + list + pad + footer, plus the error/confirm row when shown.
    let extra = u16::from(picker.picker.error.is_some() || picker.confirming.is_some());
    let height = (BODY_ROWS + 5 + FOOTER_ROWS + extra).min(area.height);
    let title = format!(" Containers on {lane_title} ");
    let inner =
        ModalFrame::centered(width, height, Some(&title), theme).render(frame.buffer_mut(), area);

    let rows = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Length(1), // filter input
        ratatui::layout::Constraint::Length(1), // pad
        ratatui::layout::Constraint::Length(BODY_ROWS),
        ratatui::layout::Constraint::Length(extra), // error / confirm prompt
        ratatui::layout::Constraint::Length(1),     // pad
        ratatui::layout::Constraint::Length(FOOTER_ROWS),
        ratatui::layout::Constraint::Min(0),
    ])
    .split(inner);

    field_row(
        frame.buffer_mut(),
        rows[0],
        " ",
        Style::default().bg(theme.surface),
        &picker.picker.input,
        true,
        TextAreaColors::field(theme, theme.accent, theme.surface),
    );

    // One row carries the whole "why is the list not what you expected" story:
    // fetching, nothing found, or nothing left to add.
    let placeholder = match picker.busy {
        Some(MountBusy::Discovering) => Some("  looking for containers…"),
        Some(MountBusy::Activating) => Some("  starting…"),
        None if picker.candidates.is_empty() => {
            Some("  no containers found, or all of them are already lanes")
        }
        None if picker.picker.filtered.is_empty() => Some("  nothing matches"),
        None => None,
    };

    // Rows are published only when real candidates are on screen: a placeholder
    // ("looking for containers…") occupies the same rows but is not clickable.
    let mut candidate_rows = Vec::new();

    if let Some(text) = placeholder {
        ratatui::widgets::Widget::render(
            ratatui::widgets::Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(theme.muted).bg(theme.surface),
            ))),
            rows[2],
            frame.buffer_mut(),
        );
    } else {
        // `filtered` holds indices into `candidates`, so the list renders the
        // filtered view while each row still resolves back to its candidate.
        let (lines, start) = modal_list_lines_windowed(
            &picker.picker.filtered,
            rows[2].height as usize,
            ListViewport::FollowSelection(picker.picker.selected),
            |index, &candidate_index| {
                let Some(candidate) = picker.candidates.get(candidate_index) else {
                    return Line::raw("");
                };
                let selected = index == picker.picker.selected;
                let marker = if selected { "▸" } else { " " };
                // A stopped candidate is dimmed: it is offered, but choosing it
                // changes something outside Deck, so it must not look identical
                // to one that is simply ready.
                let line = list_item_line(
                    theme,
                    selected,
                    format!("  {marker} "),
                    candidate.label.clone(),
                    rows[2].width as usize,
                );
                if candidate.needs_activation && !selected {
                    Line::from(
                        line.spans
                            .into_iter()
                            .map(|span| {
                                let style = span.style.fg(theme.muted);
                                Span::styled(span.content, style)
                            })
                            .collect::<Vec<_>>(),
                    )
                } else {
                    line
                }
            },
        );
        // One hit per line actually drawn, taken from the window that drew
        // them, so a click resolves to the row under the cursor after any
        // amount of scrolling.
        candidate_rows = (0..lines.len() as u16)
            .map(|offset| ListItemHit {
                rect: Rect {
                    y: rows[2].y + offset,
                    height: 1,
                    ..rows[2]
                },
                index: start + offset as usize,
            })
            .collect();
        ratatui::widgets::Widget::render(
            ratatui::widgets::Paragraph::new(lines),
            rows[2],
            frame.buffer_mut(),
        );
    }

    if extra == 1 {
        let line = if let Some(pending) = picker.confirming.as_ref() {
            Line::from(vec![
                Span::styled(
                    "  start ",
                    Style::default().fg(theme.yellow).bg(theme.surface),
                ),
                Span::styled(
                    pending.label.clone(),
                    Style::default()
                        .fg(theme.yellow)
                        .bg(theme.surface)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "? Enter to confirm",
                    Style::default().fg(theme.yellow).bg(theme.surface),
                ),
            ])
        } else {
            Line::from(Span::styled(
                format!("  {}", picker.picker.error.clone().unwrap_or_default()),
                Style::default().fg(theme.error).bg(theme.surface),
            ))
        };
        ratatui::widgets::Widget::render(
            ratatui::widgets::Paragraph::new(line),
            rows[3],
            frame.buffer_mut(),
        );
    }

    // Keys first, then what the list is currently doing: the order in force
    // (which `⇥` cycles) and the reminder that a mount is session-scoped, since
    // nothing else on screen says a lane mounted here won't come back.
    let keys_row = Rect {
        height: 1,
        ..rows[5]
    };
    modal_footer(frame.buffer_mut(), keys_row, MOUNT_FOOTER, theme);
    modal_footer(
        frame.buffer_mut(),
        Rect {
            y: rows[5].y + 1,
            height: 1,
            ..rows[5]
        },
        &format!(" by {} · this session only", picker.sort.label()),
        theme,
    );

    MountHits {
        rows: candidate_rows,
        mount: hint_rect(keys_row, MOUNT_FOOTER, MOUNT_HINT),
        sort: hint_rect(keys_row, MOUNT_FOOTER, SORT_HINT),
        cancel: hint_rect(keys_row, MOUNT_FOOTER, CANCEL_HINT),
    }
}

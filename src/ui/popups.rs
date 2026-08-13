//! Thin modal configurators: each builds one `draw_filter_picker` /
//! `ModalFrame` call from a view struct. Grouped in one file because none
//! of them carries logic of its own.

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::add_remote::AddRemoteState;
use crate::theme::Theme;

use super::widgets::{
    draw_filter_picker, markdown_window, FilterPickerView, ModalFrame, PickerField,
};
use super::NewSessionView;

/// The footer hint that doubles as the picker's confirm button. Both footers
/// open with it at the same offset, so the click target is the same rect
/// whichever field has focus.
const CREATE_HINT: &str = "⏎ create";
/// Leading pad shared by every footer string, and thus the hint's x offset.
const FOOTER_PAD: u16 = 2;

/// What the drawn new-session picker published for this frame.
pub struct NewSessionHits {
    /// Visible directory rows, each carrying its filtered index.
    pub dirs: Vec<crate::geometry::ListItemHit>,
    /// The footer's `⏎ create` hint, clickable to confirm.
    pub create: Rect,
}

pub fn draw_new_session(
    frame: &mut Frame,
    area: Rect,
    view: &NewSessionView,
    theme: &Theme,
) -> NewSessionHits {
    // The title carries the non-primary lane label so the picker target stays
    // clear without exposing connection metadata to UI.
    let title = match view.lane_title {
        Some(lane_title) => format!("New session · {lane_title}"),
        None => "New session".to_string(),
    };
    // Browsing hints are listed only while the list has focus; `⏎ create` leads
    // both so the one thing that finishes the job never moves. Tightened to
    // `·` separators and paired arrows to make room for the mouse gesture,
    // which is the one thing here nothing on screen would otherwise reveal.
    // `⇥` is advertised on the name field only: reaching the path field is
    // what taught it. `modal_footer` clips rather than wraps, so a narrow
    // terminal drops the tail hints instead of breaking the layout.
    let footer = if view.focus_name {
        format!("  {CREATE_HINT} · ⇥ path · ←→ cursor · ⎋ cancel")
    } else {
        format!("  {CREATE_HINT} · →← folder · ↑↓ move · right-click create · ⎋")
    };

    let hits = draw_filter_picker(
        frame,
        area,
        theme,
        FilterPickerView {
            title: &title,
            width: 60,
            min_height: 8,
            max_visible: crate::new_session::DIRECTORY_VIEW_ROWS,
            fields: &[
                PickerField {
                    label: "  Name: ",
                    textarea: view.name,
                    focused: view.focus_name,
                },
                PickerField {
                    label: "  Path: ",
                    textarea: view.input,
                    focused: !view.focus_name,
                },
            ],
            filtered: view.filtered,
            selected: view.selected,
            scroll: view.scroll,
            list_focused: !view.focus_name,
            pinned: view.pinned,
            empty_msg: "    (no entries)",
            error: view.error,
            footer: &footer,
        },
        |idx| format!("{}/", view.entries[idx]),
    );

    // Carve the hint out of the footer it was just drawn into. Intersecting
    // keeps the target inside the popup on a terminal too narrow to hold the
    // whole footer, where the text is clipped.
    let create = hits.footer.intersection(Rect {
        x: hits.footer.x + FOOTER_PAD,
        y: hits.footer.y,
        width: unicode_width::UnicodeWidthStr::width(CREATE_HINT) as u16,
        height: 1,
    });

    NewSessionHits {
        dirs: hits.rows,
        create,
    }
}

pub fn draw_add_remote(frame: &mut Frame, area: Rect, state: &AddRemoteState, theme: &Theme) {
    let p = &state.picker;
    let empty_msg = if p.items.is_empty() {
        "    (no ~/.ssh/config hosts \u{2014} type a hostname)"
    } else {
        "    (no matches \u{2014} press \u{23ce} to add typed host)"
    };

    let _ = draw_filter_picker(
        frame,
        area,
        theme,
        FilterPickerView {
            title: "Add Remote Host",
            width: 56,
            min_height: 7,
            max_visible: 8,
            fields: &[PickerField {
                label: "  host: ",
                textarea: &p.input,
                focused: true,
            }],
            filtered: &p.filtered,
            selected: p.selected,
            scroll: super::widgets::scroll_window(p.selected, p.filtered.len(), 8),
            list_focused: true,
            pinned: 0,
            empty_msg,
            error: p.error.as_deref(),
            footer: "  \u{23ce} add   \u{2191}\u{2193} select   \u{238b} cancel",
        },
        |idx| p.items[idx].clone(),
    );
}

/// Draw the Agents-tab summary "big view" over `area` and return the max
/// scroll offset for the current text/size, so the caller can clamp scroll
/// input. Opened from the card's popup button; scrolled with wheel or keys.
pub fn draw_summary_popup(
    frame: &mut Frame,
    area: Rect,
    text: &str,
    scroll: usize,
    theme: &Theme,
) -> usize {
    // Large but not edge-to-edge: ~80% of the screen, with floors so it
    // stays usable on small terminals.
    let w = (area.width * 4 / 5).max(40).min(area.width);
    let h = (area.height * 4 / 5).max(8).min(area.height);
    let inner = ModalFrame::centered(w, h, Some("Summary"), theme).render(frame.buffer_mut(), area);

    let rows = inner.height as usize;
    let content_w = (inner.width as usize).saturating_sub(1).max(1); // 1 col bar
    let (row_spans, max_scroll) = markdown_window(
        text,
        rows,
        scroll,
        content_w,
        theme,
        theme.text,
        theme.surface,
    );
    let lines: Vec<Line> = row_spans.into_iter().map(Line::from).collect();

    frame.render_widget(Paragraph::new(lines), inner);
    max_scroll
}

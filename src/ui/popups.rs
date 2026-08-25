//! Thin modal configurators: each builds one `draw_filter_picker` /
//! `ModalFrame` call from a view struct. Grouped in one file because none
//! of them carries logic of its own.

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::add_remote::AddRemoteState;
use crate::theme::Theme;

use super::icons::{icon, Icon};
use super::widgets::{
    draw_filter_picker, hint_rect, markdown_window, FilterPickerView, ModalFrame, PickerField,
};
use super::NewSessionView;

/// The footer hint that doubles as the picker's confirm button. Both footers
/// open with it at the same offset, so the click target is the same rect
/// whichever field has focus.
const CREATE_HINT: &str = "⏎ create";
/// The two mouse buttons: a filled half-disc, flat side inward, which is the
/// outline of one button on the mouse icon leading the row.
const LEFT_CLICK: &str = "\u{25d6}";
const RIGHT_CLICK: &str = "\u{25d7}";

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
    // Keys on the first row, mouse on the second, each led by its device's
    // icon so the split reads at a glance, and each hint led by the key or
    // button that triggers it. Two shorter rows instead of one dense one:
    // every hint here is arrow and symbol glyphs of *ambiguous*
    // East Asian width, so a row measured to fit exactly can still overflow in
    // a terminal that paints them double-width — and `modal_footer` clips
    // silently, which cost the trailing `⎋ cancel`.
    //
    // `⏎ create` leads the keys row in both focus states, so the one hint that
    // is also a button never moves. The mouse row is identical in both, because
    // clicking a row works from either field.
    let (hints, alternate) = if view.focus_name {
        ("⇥ path · ←→ cursor", "→← folder · ↑↓ move")
    } else {
        ("→← folder · ↑↓ move", "⇥ path · ←→ cursor")
    };
    // Pad the narrower focus variant out to the wider one. The block is
    // centered on its widest row, so without this the whole footer — and the
    // `⏎ create` button inside it — would slide a column on every ⇥.
    let pad = " ".repeat(
        unicode_width::UnicodeWidthStr::width(alternate)
            .saturating_sub(unicode_width::UnicodeWidthStr::width(hints)),
    );
    let keys = format!(
        "{} {CREATE_HINT} · {hints} · ⎋ cancel{pad}",
        icon(Icon::Keyboard)
    );
    let mouse = format!(
        "{} {LEFT_CLICK} folder · {RIGHT_CLICK} create",
        icon(Icon::Mouse)
    );
    let footer = [keys.as_str(), mouse.as_str()];

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
                    label: "Name",
                    textarea: view.name,
                    focused: view.focus_name,
                },
                PickerField {
                    label: "Path",
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

    // Carve the hint out of the footer it was just drawn into: the block's own
    // rect supplies the centered offset, and locating the hint in the string
    // that was painted keeps the target on the text through any rewording.
    let create = hint_rect(
        Rect {
            height: 1,
            ..hits.footer
        },
        &keys,
        CREATE_HINT,
    )
    .unwrap_or_default();

    NewSessionHits {
        dirs: hits.rows,
        create,
    }
}

/// Draw a picker over plain names: one filter field, a scrolling list, and a
/// footer whose hints double as its buttons.
///
/// Add Remote and the hidden-session restore picker are the same gesture — pick
/// one name out of a list and act on it — so they share every measurement here
/// and differ only in their wording. Six numbers that had to be kept equal at
/// two call sites now cannot drift.
///
/// The mount picker looks like one of these but is not: it needs a placeholder
/// row while discovery is out and per-row dimming for candidates that need
/// activation, neither of which `FilterPickerView` carries. Folding it in means
/// teaching the shared view those two things first.
///
/// Returns the row hits and the single-row footer rect the caller resolves its
/// own hint rects against.
fn draw_name_picker(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    picker: &crate::picker::FilterPicker,
    words: NamePickerWords<'_>,
) -> (Vec<crate::geometry::ListItemHit>, Rect) {
    const MAX_VISIBLE: usize = 8;

    let hits = draw_filter_picker(
        frame,
        area,
        theme,
        FilterPickerView {
            title: words.title,
            width: 56,
            min_height: 7,
            max_visible: MAX_VISIBLE,
            fields: &[PickerField {
                label: words.field_label,
                textarea: &picker.input,
                focused: true,
            }],
            filtered: &picker.filtered,
            selected: picker.selected,
            scroll: super::widgets::scroll_window(
                picker.selected,
                picker.filtered.len(),
                MAX_VISIBLE,
            ),
            list_focused: true,
            pinned: 0,
            empty_msg: words.empty_msg,
            error: picker.error.as_deref(),
            footer: &[words.footer],
        },
        |idx| picker.items[idx].clone(),
    );

    let footer_row = Rect {
        height: 1,
        ..hits.footer
    };
    (hits.rows, footer_row)
}

/// The only thing two name pickers disagree about.
struct NamePickerWords<'a> {
    title: &'a str,
    field_label: &'a str,
    empty_msg: &'a str,
    footer: &'a str,
}

/// The add-remote footer, whose hints double as its buttons.
const ADD_HINT: &str = "[Enter] Add";
const CANCEL_HINT: &str = "[Esc] Cancel";
const ADD_REMOTE_FOOTER: &str = "[Enter] Add   [↑↓] Select   [Esc] Cancel";

pub fn draw_add_remote(
    frame: &mut Frame,
    area: Rect,
    state: &AddRemoteState,
    theme: &Theme,
) -> crate::geometry::AddRemoteHits {
    let (rows, footer_row) = draw_name_picker(
        frame,
        area,
        theme,
        &state.picker,
        NamePickerWords {
            title: "Add Remote",
            field_label: "Host",
            empty_msg: if state.picker.items.is_empty() {
                "    (no ~/.ssh/config hosts \u{2014} type a hostname)"
            } else {
                "    (no matches — press Enter to add the typed host)"
            },
            footer: ADD_REMOTE_FOOTER,
        },
    );

    crate::geometry::AddRemoteHits {
        hosts: rows,
        add: hint_rect(footer_row, ADD_REMOTE_FOOTER, ADD_HINT),
        cancel: hint_rect(footer_row, ADD_REMOTE_FOOTER, CANCEL_HINT),
    }
}

/// The restore picker's footer, whose hints double as its buttons.
const RESTORE_ALL_HINT: &str = "^A all";
const HIDDEN_FOOTER: &str = "\u{23ce} restore   \u{2191}\u{2193} select   ^A all   \u{238b} cancel";

/// Draw the "restore a hidden session" picker for one lane.
pub fn draw_hidden_sessions(
    frame: &mut Frame,
    area: Rect,
    state: &crate::overlay::HiddenSessionsState,
    lane_title: &str,
    theme: &Theme,
) -> crate::geometry::HiddenHits {
    let (rows, footer_row) = draw_name_picker(
        frame,
        area,
        theme,
        &state.picker,
        NamePickerWords {
            title: &format!("Hidden on {lane_title}"),
            field_label: "Filter",
            empty_msg: "    (no matches)",
            footer: HIDDEN_FOOTER,
        },
    );

    crate::geometry::HiddenHits {
        rows,
        restore_all: hint_rect(footer_row, HIDDEN_FOOTER, RESTORE_ALL_HINT),
        cancel: hint_rect(footer_row, HIDDEN_FOOTER, CANCEL_HINT),
    }
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
        theme.elevated,
    );
    let lines: Vec<Line> = row_spans.into_iter().map(Line::from).collect();

    frame.render_widget(Paragraph::new(lines), inner);
    max_scroll
}

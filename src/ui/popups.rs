//! Thin popup configurators: each builds one `draw_filter_picker` /
//! `popup_frame` call from a view struct. Grouped in one file because none
//! of them carries logic of its own.

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::add_remote::AddRemoteState;
use crate::theme::Theme;

use super::widgets::{
    centered_rect, draw_filter_picker, markdown_window, popup_frame, FilterPickerView, PickerField,
    PopupStyle,
};
use super::NewSessionView;

pub fn draw_new_session(frame: &mut Frame, area: Rect, view: &NewSessionView, theme: &Theme) {
    // Title carries the target host for remote creation so it's obvious the
    // dir browser is listing that host, not the local machine.
    let title = match view.host {
        Some(host) => format!(" New session · @{host} "),
        None => " New session ".to_string(),
    };
    let footer = if view.focus_name {
        "  ⏎ create   ⇥ switch   ←→ cursor   ⎋ cancel"
    } else {
        "  ⏎ create   ⇥ switch   ←→ nav   ⎋ cancel"
    };

    draw_filter_picker(
        frame,
        area,
        theme,
        FilterPickerView {
            title: &title,
            width: 60,
            min_height: 8,
            max_visible: 8,
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
            empty_msg: "    (no entries)",
            error: view.error,
            footer,
        },
        |idx| format!("{}/", view.entries[idx]),
    );
}

pub fn draw_add_remote(frame: &mut Frame, area: Rect, state: &AddRemoteState, theme: &Theme) {
    let p = &state.picker;
    let empty_msg = if p.items.is_empty() {
        "    (no ~/.ssh/config hosts \u{2014} type a hostname)"
    } else {
        "    (no matches \u{2014} press \u{23ce} to add typed host)"
    };

    draw_filter_picker(
        frame,
        area,
        theme,
        FilterPickerView {
            title: " Add Remote Host ",
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
    let popup = centered_rect(area, w, h);

    let inner = popup_frame(
        frame.buffer_mut(),
        popup,
        PopupStyle {
            title: Some(" Summary "),
            border_fg: theme.accent,
            bg: theme.surface,
        },
    );

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

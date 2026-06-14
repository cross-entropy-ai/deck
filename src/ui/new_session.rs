use ratatui::layout::Rect;
use ratatui::Frame;

use crate::theme::Theme;
use crate::ui::form::{draw_filter_picker, FilterPickerView, PickerField};

use super::NewSessionView;

const POPUP_WIDTH: u16 = 60;
const POPUP_MIN_HEIGHT: u16 = 8;
const MAX_VISIBLE_ENTRIES: usize = 8;

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
            width: POPUP_WIDTH,
            min_height: POPUP_MIN_HEIGHT,
            max_visible: MAX_VISIBLE_ENTRIES,
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

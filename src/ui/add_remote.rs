use ratatui::layout::Rect;
use ratatui::Frame;

use crate::add_remote::AddRemoteState;
use crate::theme::Theme;
use crate::ui::widgets::{draw_filter_picker, FilterPickerView, PickerField};

const POPUP_WIDTH: u16 = 56;
const MAX_VISIBLE: usize = 8;
const POPUP_MIN_HEIGHT: u16 = 7;

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
            width: POPUP_WIDTH,
            min_height: POPUP_MIN_HEIGHT,
            max_visible: MAX_VISIBLE,
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

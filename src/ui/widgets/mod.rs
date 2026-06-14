//! Small shared render helpers for the popup/overlay UIs, grouped by
//! concern: popup sizing/framing (`popup`), single-line text fields
//! (`textarea`), list/row rendering (`list`), and scrollable text with
//! scrollbars (`scroll`). Centralizing these removes hand-rolled copies and
//! keeps popups consistent (rounded corners, shared windowing).

mod list;
mod popup;
mod scroll;
mod textarea;

pub use list::{draw_picker_list, full_width_row, list_item_line, scroll_window};
pub use popup::{centered_rect, clamp_popup_height, popup_frame, popup_rect, PopupStyle};
pub use textarea::{style_textarea, TextAreaColors};

// `markdown_window` is consumed only within `ui` (the Summary card and its
// popup), so it stays restricted to the parent module rather than exported
// crate-wide.
pub(super) use scroll::markdown_window;

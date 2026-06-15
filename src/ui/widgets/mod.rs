//! Small shared render helpers for popup/overlay UIs: popup sizing/framing
//! (`popup`), text fields (`textarea`), label+field rows (`field`), the filter
//! picker (`picker`), list/row rendering (`list`), and scrollable text
//! (`scroll`). Centralizing keeps popups consistent (rounded corners, windowing).

mod field;
mod list;
mod picker;
mod popup;
mod scroll;
mod textarea;

pub use field::field_row;
pub use list::{full_width_row, list_item_line, scroll_window};
pub use picker::{draw_filter_picker, FilterPickerView, PickerField};
pub use popup::{centered_rect, clamp_popup_height, popup_frame, PopupStyle};
pub use textarea::{style_textarea, TextAreaColors};

// `markdown_window` is consumed only within `ui` (the Summary card and its
// popup), so it stays restricted to the parent module rather than exported
// crate-wide.
pub(super) use scroll::markdown_window;

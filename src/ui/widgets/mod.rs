//! Small shared render helpers for modal/overlay UIs: modal sizing/framing
//! (`popup`), text fields (`textarea`), label+field rows (`field`), the filter
//! picker (`picker`), list/row rendering (`list`), and scrollable text
//! (`scroll`). Centralizing keeps popups consistent (rounded corners, windowing).

mod field;
mod list;
mod picker;
mod popup;
mod scroll;
mod textarea;

pub use field::{field_row, form_field_row, form_label_span, FormFieldState};
pub use list::{
    contrasting_foreground, full_width_row, list_item_line, modal_list_lines,
    modal_list_lines_windowed, modal_selection_foreground, scroll_window, ListViewport,
};
pub use picker::{draw_filter_picker, FilterPickerView, PickerField};
pub use popup::{centered_rect, clamp_popup_height, hint_rect, modal_footer, ModalFrame};
pub use textarea::{style_textarea, TextAreaColors};

// `markdown_window` is consumed only within `ui` (the Summary card and its
// popup), so it stays restricted to the parent module rather than exported
// crate-wide.
pub(super) use scroll::markdown_window;

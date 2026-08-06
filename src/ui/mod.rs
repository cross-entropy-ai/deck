pub mod bridge;
mod menu;
pub mod overlays;
mod popups;
mod reload;
mod settings;
mod sidebar;
mod text;
pub mod widgets;

use crate::keybindings::Keybindings;

pub use menu::draw_context_menu;
pub use popups::{draw_add_remote, draw_new_session, draw_summary_popup};
pub use reload::{draw_reload_bar, reload_row_count};
pub use settings::{draw_settings_page, draw_theme_picker};
pub use sidebar::{draw_sidebar, SidebarProps};

pub struct ExcludeEditorView<'a> {
    pub patterns: &'a [String],
    pub selected: usize,
    pub adding: bool,
    pub input: &'a ratatui_textarea::TextArea<'static>,
    pub error: Option<&'a str>,
}

pub struct NewSessionView<'a> {
    pub name: &'a ratatui_textarea::TextArea<'static>,
    pub focus_name: bool,
    pub input: &'a ratatui_textarea::TextArea<'static>,
    pub entries: &'a [String],
    pub filtered: &'a [usize],
    pub selected: usize,
    pub error: Option<&'a str>,
    /// Optional non-primary lane label shown in the title.
    pub lane_title: Option<&'a str>,
}

/// One settings row, reduced to display strings by the render loop. The
/// `app::settings::setting_rows` sources label/value/help for the active page;
/// the loop runs each row's closures against `&AppState` and hands results here, keeping
/// `draw_settings_page` a pure `ui` fn that never sees `AppState`.
pub struct SettingRowView {
    pub label: &'static str,
    pub value: String,
    pub help: String,
}

pub struct SettingsView<'a> {
    pub selected: usize,
    pub rows: Vec<SettingRowView>,
    pub page: crate::state::SettingsPage,
    pub exclude_editor: Option<ExcludeEditorView<'a>>,
    pub keybindings: &'a Keybindings,
    pub keybindings_view_open: bool,
    pub keybindings_view_scroll: u16,
    /// When `Some`, the language input box is open over the settings page.
    pub summary_lang_input: Option<&'a ratatui_textarea::TextArea<'static>>,
}

mod add_remote;
pub mod bridge;
mod menu;
mod new_session;
pub mod overlays;
mod reload;
mod settings;
mod sidebar;
mod summary_popup;
mod text;
pub mod widgets;

use crate::keybindings::Keybindings;

pub use add_remote::draw_add_remote;
pub use menu::draw_context_menu;
pub use new_session::draw_new_session;
pub use reload::{draw_reload_bar, reload_row_count};
pub use settings::{draw_settings_page, draw_theme_picker};
pub use sidebar::{draw_sidebar, SidebarProps};
pub use summary_popup::draw_summary_popup;

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
    /// `Some(host)` when creating on a remote host — shown in the title,
    /// and the dir browser is listing that host over ssh.
    pub host: Option<&'a str>,
}

/// One settings row, reduced to display strings by the render loop. The
/// `app::settings::SETTING_ROWS` table sources label/value/help; the loop runs
/// each row's closures against `&AppState` and hands results here, keeping
/// `draw_settings_page` a pure `ui` fn that never sees `AppState`.
pub struct SettingRowView {
    pub label: &'static str,
    pub value: String,
    pub help: String,
}

pub struct SettingsView<'a> {
    pub selected: usize,
    pub rows: Vec<SettingRowView>,
    pub exclude_editor: Option<ExcludeEditorView<'a>>,
    pub keybindings: &'a Keybindings,
    pub keybindings_view_open: bool,
    pub keybindings_view_scroll: u16,
    /// When `Some`, the language input box is open over the settings page.
    pub summary_lang_input: Option<&'a ratatui_textarea::TextArea<'static>>,
}

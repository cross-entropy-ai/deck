pub mod bridge;
pub mod layout;
mod menu;
mod overlays;
mod settings;
mod sidebar;
mod text;
pub mod theme;

use crate::keybindings::Keybindings;
use crate::state::{LayoutMode, ViewMode};

pub use menu::draw_context_menu;
pub use settings::draw_settings_page;
pub use sidebar::draw_sidebar;

/// Minimal data needed to render one session row.
pub struct SessionView<'a> {
    pub name: &'a str,
    pub dir: &'a str,
    pub branch: &'a str,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub idle_seconds: u64,
}

pub struct ExcludeEditorView<'a> {
    pub patterns: &'a [String],
    pub selected: usize,
    pub adding: bool,
    pub input: &'a str,
    pub error: Option<&'a str>,
}

pub struct SettingsView<'a> {
    pub selected: usize,
    pub focus_main: bool,
    pub theme_name: &'a str,
    pub theme_picker_open: bool,
    pub theme_picker_selected: usize,
    pub theme_names: Vec<&'a str>,
    pub layout_mode: LayoutMode,
    pub show_borders: bool,
    pub view_mode: ViewMode,
    pub exclude_count: usize,
    pub exclude_editor: Option<ExcludeEditorView<'a>>,
    pub keybindings: &'a Keybindings,
    pub keybindings_view_open: bool,
    pub keybindings_view_scroll: u16,
    pub update_check_enabled: bool,
    pub update_check_help: String,
}

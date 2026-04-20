pub mod bridge;
pub mod layout;
mod menu;
mod overlays;
mod reload;
mod settings;
mod sidebar;
mod text;
pub mod theme;

use crate::keybindings::Keybindings;
use crate::state::{LayoutMode, SessionStatus, ViewMode};

pub use menu::draw_context_menu;
pub use reload::{draw_reload_bar, reload_row_count};
pub use settings::draw_settings_page;
pub use sidebar::draw_sidebar;

/// Runtime state of a configured plugin, used by the sidebar footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginStatus {
    Inactive,
    Background,
    Foreground,
}

/// Minimal data needed to render one plugin row in the sidebar footer.
pub struct PluginView<'a> {
    pub key: char,
    pub name: &'a str,
    pub status: PluginStatus,
}

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
    /// Effective status: the raw `SessionRow.status` after applying
    /// the Waiting-ack override (see `AppState::effective_status`).
    pub status: SessionStatus,
    /// True iff this session is the one tmux is currently attached to.
    /// The status icon is overridden to a "you are here" marker for
    /// the current session — anything live there is already visible
    /// in the main pane, so the icon's job is just to confirm focus.
    pub is_current: bool,
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

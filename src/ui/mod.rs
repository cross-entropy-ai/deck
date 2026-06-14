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
pub mod theme;
pub mod widgets;

use crate::keybindings::Keybindings;
use crate::state::{SessionEntry, SessionEntryKind};

pub use add_remote::draw_add_remote;
pub use menu::draw_context_menu;
pub use new_session::draw_new_session;
pub use reload::{draw_reload_bar, reload_row_count};
pub use settings::{draw_settings_page, draw_theme_picker};
pub use sidebar::{draw_sidebar, SidebarProps};
pub use summary_popup::draw_summary_popup;

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

/// Where a sidebar session lives. The sidebar renderer is otherwise
/// origin-agnostic; it only consults this to drive group dividers and
/// to dispatch row-level actions to the right backend.
#[derive(Debug, Clone, Copy)]
pub enum SessionOrigin<'a> {
    Local,
    Remote { host: &'a str },
}

/// The session-row abstraction the tabs-mode renderer consumes (the
/// Expanded/Compact list builds its rows from `SessionEntry` directly in
/// `model`). Tabs mode reads only origin/name/unreachable; the renderer
/// must not branch on concrete types.
pub trait SidebarSession {
    fn origin(&self) -> SessionOrigin<'_>;
    fn name(&self) -> &str;
    /// Reaching this session's source failed (timeout, auth, ...).
    /// Tab label is still drawn, just greyed out.
    fn unreachable(&self) -> bool {
        false
    }
}

// One impl for the unified store. Origin comes from `host` (None = local,
// Some = remote); the placeholder kinds paint their label. The renderer
// never asks "is this remote?".
impl SidebarSession for SessionEntry {
    fn origin(&self) -> SessionOrigin<'_> {
        match &self.host {
            None => SessionOrigin::Local,
            Some(host) => SessionOrigin::Remote { host },
        }
    }
    fn name(&self) -> &str {
        // The placeholder display strings live here, derived from `kind`,
        // not stored as magic session names.
        match self.kind {
            SessionEntryKind::Unreachable => crate::state::UNREACHABLE_LABEL,
            SessionEntryKind::NoSessions => crate::state::NO_SESSIONS_LABEL,
            SessionEntryKind::Live { .. } | SessionEntryKind::Connecting => &self.name,
        }
    }
    fn unreachable(&self) -> bool {
        matches!(self.kind, SessionEntryKind::Unreachable)
    }
}

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

/// One settings row, already reduced to display strings by the render
/// loop. The `app::settings::SETTING_ROWS` descriptor table is the source
/// of label/value/help; the loop calls each row's closures against
/// `&AppState` and hands the results here, keeping `draw_settings_page` a
/// pure `ui` fn that never sees `AppState`.
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

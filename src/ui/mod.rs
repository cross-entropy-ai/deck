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
use crate::state::{SessionEntry, SessionEntryKind};

pub use add_remote::draw_add_remote;
pub use menu::draw_context_menu;
pub use new_session::draw_new_session;
pub use reload::{draw_reload_bar, reload_row_count};
pub use settings::{draw_settings_page, draw_theme_picker};
pub use sidebar::{draw_sidebar, SidebarProps};
pub use summary_popup::draw_summary_popup;

/// The session-row abstraction the tabs-mode renderer consumes (the
/// Expanded/Compact list builds rows from `SessionEntry` directly in `model`).
/// Tabs mode reads only host/name/unreachable; the renderer must not branch
/// on concrete types. `host()` is `None` for a local session, `Some(host)`
/// for a remote one — the renderer passes it straight to `tab_label`.
pub trait SidebarSession {
    fn host(&self) -> Option<&str>;
    fn name(&self) -> &str;
    /// Whether this row is a real session rather than a connection/status
    /// placeholder. Used by the Header's Projects count.
    fn is_attachable(&self) -> bool {
        true
    }
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
    fn host(&self) -> Option<&str> {
        self.host.as_deref()
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
    fn is_attachable(&self) -> bool {
        SessionEntry::is_attachable(self)
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

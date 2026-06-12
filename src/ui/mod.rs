mod add_remote;
pub mod bridge;
pub mod form;
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
use crate::state::{RemoteSessionRow, SessionRow};

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
/// (later) to dispatch row-level actions to the right backend.
#[derive(Debug, Clone, Copy)]
pub enum SessionOrigin<'a> {
    Local,
    Remote { host: &'a str },
}

/// Activity signal for a session. Bundled into one `Option` on the
/// trait so backends that don't yet collect any of it return `None`
/// (an explicit "unknown") rather than a misleading `Idle/0/false`.
/// When real collection lands for a backend, flip its impl to `Some`
/// and the renderer picks it up uniformly.
#[derive(Debug, Clone, Copy)]
pub struct SessionActivity {
    pub idle_seconds: u64,
}

/// The single abstraction the sidebar renderer consumes. Anything that
/// can appear as a row — local tmux session, remote tmux session over
/// ssh, future alternate-socket sources — implements this. The renderer
/// must not branch on concrete types; if a row-level piece of info
/// isn't on this trait, it doesn't belong in the per-row UI.
pub trait SidebarSession {
    fn origin(&self) -> SessionOrigin<'_>;
    fn name(&self) -> &str;
    fn dir(&self) -> &str;
    /// `None` means the backend doesn't collect activity yet (renderer
    /// draws a neutral placeholder, no indicator + no idle badge).
    /// `Some` means render the indicator / idle badge from real data.
    fn activity(&self) -> Option<SessionActivity>;
    /// Synthetic placeholder shown before the first refresh round
    /// completes; renderer paints a muted "(connecting...)" instead of
    /// the session name.
    fn loading(&self) -> bool {
        false
    }
    /// Reaching this session's source failed (timeout, auth, ...).
    /// Row is still drawn, just greyed out.
    fn unreachable(&self) -> bool {
        false
    }
}

impl SidebarSession for SessionRow {
    fn origin(&self) -> SessionOrigin<'_> {
        SessionOrigin::Local
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn dir(&self) -> &str {
        &self.dir
    }
    fn activity(&self) -> Option<SessionActivity> {
        Some(SessionActivity {
            idle_seconds: self.idle_seconds,
        })
    }
}

impl SidebarSession for RemoteSessionRow {
    fn origin(&self) -> SessionOrigin<'_> {
        SessionOrigin::Remote { host: &self.host }
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn dir(&self) -> &str {
        &self.dir
    }
    // Remote refresh doesn't collect activity yet. Returning None
    // (rather than fake Idle/0/false) means the renderer paints a
    // neutral placeholder; once the refresh worker gathers real status
    // we just return Some(...) here and the indicator/idle badge light
    // up uniformly with local rows.
    fn activity(&self) -> Option<SessionActivity> {
        None
    }
    fn loading(&self) -> bool {
        self.loading
    }
    fn unreachable(&self) -> bool {
        self.unreachable
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

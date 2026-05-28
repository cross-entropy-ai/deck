use std::time::Instant;

use ratatui::layout::Rect;
use ratatui_textarea::TextArea;
use serde::{Deserialize, Serialize};

use crate::config::PluginConfig;
use crate::keybindings::Keybindings;
use crate::layout::{
    card_height, context_menu_width, plugin_block_rows, tab_col_ranges, BANNER_MIN_WIDTH,
};
use crate::new_session::NewSessionState;
use crate::update::{UpdateCheckMode, UpdateStatus};

// --- Constants ---

pub const SIDEBAR_MIN: u16 = 16;
pub const SIDEBAR_MAX: u16 = 60;
pub const SIDEBAR_HEIGHT: u16 = 4;
pub const SIDEBAR_HEIGHT_MIN: u16 = 2;
pub const SIDEBAR_HEIGHT_MAX: u16 = 4;
const SIDEBAR_HEIGHT_MIN_BORDERED: u16 = 4;
const SIDEBAR_HEIGHT_MAX_BORDERED: u16 = 6;
const MIN_MAIN_WIDTH: u16 = 10;
const MIN_MAIN_HEIGHT: u16 = 1;

// "Switch" is dropped — the focus already triggers the switch, so the
// menu item was redundant.
const SESSION_MENU_ITEMS: &'static [&'static str] =
    &["Rename", "Kill", "Move up", "Move down"];
// Remote sessions live on a different tmux server, so the
// deck-side `session_order` (which drives Move up/down) doesn't
// apply. Rename/Kill map to `ssh <host> tmux <cmd>` against the
// host's server; "Remove from list" detaches the host from deck's
// config (equivalent to `deck remote remove <host>`).
const REMOTE_SESSION_MENU_ITEMS: &'static [&'static str] =
    &["Rename", "Kill", "Remove from list"];
const HOST_DIVIDER_MENU_ITEMS: &'static [&'static str] = &["Port Forward"];
const GLOBAL_MENU_ITEMS: &'static [&'static str] = &[
    "New session",
    "Toggle layout",
    "Toggle borders",
    "Settings",
    "Quit",
];

// --- Enums ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusMode {
    Main,
    Sidebar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainView {
    Terminal,
    Settings,
    Plugin(usize),
    Upgrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Expanded,
    Compact,
}

/// Two-state session activity model.
///
/// - `Idle`: nothing demanding attention — shell at prompt or a passive program.
/// - `Working`: something is actively running in the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionStatus {
    #[default]
    Idle,
    Working,
}

pub const SETTINGS_ITEM_COUNT: usize = 7;

// --- Context menu ---

#[derive(Debug, Clone)]
pub enum MenuKind {
    /// Right-clicked a session row. `items` is decided at construction
    /// (e.g. local rows include `Move up/down`, remotes don't) so the
    /// reducer doesn't have to redo that lookup on every keypress.
    Session {
        focus: FocusTarget,
        items: &'static [&'static str],
    },
    Global,
    /// Click on the `[…]` button on a remote host divider. Single
    /// item today (`Port Forward`); extendable.
    HostDivider {
        host: String,
        items: &'static [&'static str],
    },
}

impl MenuKind {
    pub fn items(&self) -> &'static [&'static str] {
        match self {
            MenuKind::Session { items, .. } => items,
            MenuKind::Global => GLOBAL_MENU_ITEMS,
            MenuKind::HostDivider { items, .. } => items,
        }
    }
}

pub fn host_divider_menu_items() -> &'static [&'static str] {
    HOST_DIVIDER_MENU_ITEMS
}

/// Menu items shown after right-clicking a session row. The action
/// layer reads `session_target` to decide which list applies; the
/// renderer never needs to know.
pub fn session_menu_items(target: &SessionTargetRef<'_>) -> &'static [&'static str] {
    match target {
        SessionTargetRef::Local(_) => SESSION_MENU_ITEMS,
        SessionTargetRef::Remote(_) => REMOTE_SESSION_MENU_ITEMS,
    }
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub kind: MenuKind,
    pub x: u16,
    pub y: u16,
    pub selected: usize,
}

impl ContextMenu {
    pub fn items(&self) -> &'static [&'static str] {
        self.kind.items()
    }
}

// --- Session data ---

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub name: String,
    pub dir: String,
    pub is_current: bool,
    pub idle_seconds: u64,
    /// Raw activity status, pre-ack.
    pub status: SessionStatus,
}

/// One tmux session living on a remote host. Modeled separately from
/// `SessionRow` so the existing local-only invariants (session_order,
/// notification ack maps, validate_session_name, kill/rename dispatch)
/// don't have to grow an `origin` discriminator on every touchpoint.
#[derive(Debug, Clone)]
pub struct RemoteSessionRow {
    pub host: String,
    pub name: String,
    pub dir: String,
    /// True if reaching this host failed (timeout, auth error, etc.).
    /// The row is still rendered but greyed out and the name column
    /// shows a brief reason.
    pub unreachable: bool,
    /// True for the synthetic placeholder rows seeded at app startup,
    /// before the first remote refresh round completes. Renders as a
    /// muted "(connecting...)" so the user sees the group section
    /// appear immediately even if the ssh+tmux query takes a few
    /// seconds. Cleared (false) when a real refresh update lands.
    pub loading: bool,
}

/// Identifies a focused sidebar row by its flat index.
///
/// The flat index walks the visible row list in render order: local
/// rows first (`0..state.filtered.len()`), then remote rows
/// (`filtered.len()..filtered.len() + remote_sessions.len()`).
/// `AppState::session_target` decodes this back into the underlying
/// storage for action dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusTarget(pub usize);

/// A resolved reference into one of the two backing stores. The
/// renderer doesn't see this — it consumes the unified `SidebarSession`
/// trait — but reducers/action layer use it to keep the local vs
/// remote dispatch in exactly one place (`AppState::session_target`).
#[derive(Debug)]
pub enum SessionTargetRef<'a> {
    Local(&'a SessionRow),
    Remote(&'a RemoteSessionRow),
}

/// One renderable item in the sidebar layout — either a non-focusable
/// group header or a focusable session row. Both `SidebarLayout`
/// consumers — the sidebar renderer and `focus_at_row` — walk the
/// same list, so highlight, scroll, and mouse hit-testing agree about
/// where every row lives.
#[derive(Debug, Clone)]
pub struct SidebarItem {
    pub kind: SidebarItemKind,
    /// Number of terminal rows this item occupies.
    pub height: usize,
}

#[derive(Debug, Clone)]
pub enum SidebarItemKind {
    /// Group header (label). Not focusable. `host_idx` is the
    /// position of the host among distinct remote hosts in render
    /// order, used to cycle the divider accent color. Session rows
    /// don't carry it because they don't read it.
    Header { label: String, host_idx: usize },
    /// A session row at the given flat index — matches the
    /// `FocusTarget` numbering: local rows first, then remotes. The
    /// renderer pairs this index with a `&[&dyn SidebarSession]` slice
    /// built in render order; storage routing happens via
    /// `AppState::session_target` in the action layer.
    Session { session_idx: usize },
}

#[derive(Debug, Clone, Default)]
pub struct SidebarLayout {
    pub items: Vec<SidebarItem>,
}

/// Compute the scroll offset (in terminal rows) so the focused item
/// is fully visible inside a viewport `visible_height` rows tall.
/// Returns 0 if nothing is focused or the focused item already fits
/// without scrolling.
pub fn scroll_for_layout(
    layout: &SidebarLayout,
    target: Option<FocusTarget>,
    visible_height: usize,
) -> usize {
    let Some(target) = target else {
        return 0;
    };
    let Some((y_top, item)) = layout.locate(target) else {
        return 0;
    };
    let y_bottom = y_top + item.height;
    if y_bottom <= visible_height {
        // Whole item fits from the top with no scroll.
        return 0;
    }
    // Scroll just enough that the focused item's bottom is at the
    // viewport's bottom edge.
    y_bottom - visible_height
}

impl SidebarLayout {
    /// Walk the items, yielding the top y-offset (in terminal rows)
    /// for each item alongside the item itself.
    pub fn iter_with_y(&self) -> impl Iterator<Item = (usize, &SidebarItem)> {
        let mut y = 0usize;
        self.items.iter().map(move |it| {
            let cur = y;
            y += it.height;
            (cur, it)
        })
    }

    /// Find the layout item that matches the given focus target,
    /// returning its top y and the item itself. `None` if focus is
    /// out of range.
    pub fn locate(&self, target: FocusTarget) -> Option<(usize, &SidebarItem)> {
        self.iter_with_y().find(|(_, it)| match &it.kind {
            SidebarItemKind::Session { session_idx } => *session_idx == target.0,
            SidebarItemKind::Header { .. } => false,
        })
    }

    /// Map a vertical offset (in rows, relative to the sidebar's
    /// scrollable area top) to a FocusTarget if it falls on a
    /// session row. Header rows return None.
    pub fn target_at_y(&self, y: usize) -> Option<FocusTarget> {
        for (top, it) in self.iter_with_y() {
            if y >= top && y < top + it.height {
                return match it.kind {
                    SidebarItemKind::Session { session_idx } => Some(FocusTarget(session_idx)),
                    SidebarItemKind::Header { .. } => None,
                };
            }
        }
        None
    }
}

/// Click-region for the `[…]` button on a remote-host divider. The
/// sidebar renderer fills `divider_hits` after each render; mouse
/// hit-testing consults it before `focus_at_row()`.
#[derive(Debug, Clone)]
pub struct DividerHit {
    pub host: String,
    pub rect: Rect,
}

// --- Side effects ---

#[derive(Debug, Default)]
pub struct SideEffect {
    pub switch_session: Option<String>,
    /// Switch the main view to a remote session. Carries (host, name)
    /// — App's dispatch layer routes the `tmux switch-client` over ssh.
    pub switch_remote: Option<RemoteSwitchRequest>,
    pub kill_session: Option<KillRequest>,
    pub rename_session: Option<RenameRequest>,
    /// `Some(req)` means: create a new tmux session with `req.name` at
    /// `req.dir`. The picker fills both fields; the auto-naming loop that
    /// used to live in `App::create_new_session` now lives in
    /// `App::open_new_session_picker` and is editable via the name input.
    pub create_session: Option<CreateSessionRequest>,
    /// Detach a remote host from deck (equivalent to `deck remote
    /// remove <host>`). Dispatch sends `Op::StopHost` to the
    /// port-forward worker; the state mutation + config save happen
    /// in the reducer.
    pub remove_remote_host: Option<String>,
    /// Dispatch should open the new-session picker overlay. Fired by
    /// the global menu's "New session" item; uses the focused session's
    /// dir as the picker's starting point.
    pub open_new_session_picker: bool,
    /// Dispatch should re-run `read_dir` for the picker's current
    /// parent and refresh `entries`. Fired by any reducer arm that
    /// changes the effective parent.
    pub reread_new_session_entries: bool,
    pub resize_pty: bool,
    pub save_config: bool,
    pub apply_tmux_theme: bool,
    pub refresh_sessions: bool,
    pub quit: bool,
}

impl SideEffect {
    /// Fold another SideEffect into this one. Option fields from `other`
    /// overwrite Some values; bool fields are OR'd. Use this whenever a
    /// compound action delegates to a sub-action — it keeps new fx
    /// fields from silently being dropped.
    pub fn merge(&mut self, other: SideEffect) {
        if other.switch_session.is_some() {
            self.switch_session = other.switch_session;
        }
        if other.switch_remote.is_some() {
            self.switch_remote = other.switch_remote;
        }
        if other.kill_session.is_some() {
            self.kill_session = other.kill_session;
        }
        if other.rename_session.is_some() {
            self.rename_session = other.rename_session;
        }
        if other.create_session.is_some() {
            self.create_session = other.create_session;
        }
        if other.remove_remote_host.is_some() {
            self.remove_remote_host = other.remove_remote_host;
        }
        self.open_new_session_picker |= other.open_new_session_picker;
        self.reread_new_session_entries |= other.reread_new_session_entries;
        self.resize_pty |= other.resize_pty;
        self.save_config |= other.save_config;
        self.apply_tmux_theme |= other.apply_tmux_theme;
        self.refresh_sessions |= other.refresh_sessions;
        self.quit |= other.quit;
    }
}

/// Info needed to execute a kill: which session to kill, and optionally
/// which session to switch to first (if killing the current session).
#[derive(Debug)]
pub struct KillRequest {
    pub name: String,
    /// `Some(host)` targets the remote tmux server on that host;
    /// `None` targets the local tmux server.
    pub host: Option<String>,
    /// LOCAL session to switch to after the kill (only meaningful
    /// when killing the user's currently attached local session).
    /// For remote kills, dispatch returns the user to the local view
    /// instead, and this field is `None`.
    pub switch_to: Option<String>,
}

/// Info needed to execute a rename.
#[derive(Debug)]
pub struct RenameRequest {
    pub old_name: String,
    pub new_name: String,
    /// `Some(host)` targets the remote tmux server on that host.
    pub host: Option<String>,
}

/// Info needed to execute "create a new tmux session".
#[derive(Debug)]
pub struct CreateSessionRequest {
    pub name: String,
    pub dir: String,
}

/// Info needed to switch the main view to a remote tmux session.
#[derive(Debug)]
pub struct RemoteSwitchRequest {
    pub host: String,
    pub name: String,
}

/// UI state for an in-progress rename.
#[derive(Debug, Clone)]
pub struct RenameState {
    pub original_name: String,
    pub input: TextArea<'static>,
    /// `Some(host)` when the rename targets a remote session.
    pub host: Option<String>,
}

impl RenameState {
    pub fn new(original_name: String, initial: String, host: Option<String>) -> Self {
        let mut ta = TextArea::new(vec![initial]);
        ta.move_cursor(ratatui_textarea::CursorMove::End);
        Self {
            original_name,
            input: ta,
            host,
        }
    }
}

/// UI state for the exclude pattern editor popup.
#[derive(Debug, Clone)]
pub struct ExcludeEditorState {
    pub selected: usize,
    pub adding: bool,
    pub input: TextArea<'static>,
    pub error: Option<String>,
}

impl ExcludeEditorState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            adding: false,
            input: make_textarea(""),
            error: None,
        }
    }

    /// Read current add-input text.
    pub fn input_str(&self) -> &str {
        self.input.lines().first().map(String::as_str).unwrap_or("")
    }

    /// Reset the add input to empty (called on StartAdd / CancelAdd / Confirm).
    pub fn reset_input(&mut self) {
        self.input = make_textarea("");
    }
}

// --- Port forward overlay ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PfField {
    Mode,
    BindAddr,
    ListenPort,
    TargetHost,
    TargetPort,
}

/// One input field, backed by `ratatui-textarea`. Each field carries
/// its own cursor and edit history; the keyboard dispatcher feeds key
/// events to whichever one is focused.
#[derive(Debug, Clone)]
pub struct PfAddForm {
    pub mode: crate::config::ForwardMode,
    pub focus: PfField,
    pub bind_addr: TextArea<'static>,
    pub listen_port: TextArea<'static>,
    pub target_host: TextArea<'static>,
    pub target_port: TextArea<'static>,
    /// True while a validated spec is in flight to the worker. The
    /// form stays rendered (read-only) until `PfTaskResult` for this
    /// host's Forward op clears or fails the submission. Lazy
    /// persist: config is only written when the worker reports
    /// success.
    pub submitting: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PfFormError {
    ListenPortRange,
    TargetPortRange,
    TargetHostRequired,
}

impl PfFormError {
    pub fn message(&self) -> &'static str {
        match self {
            PfFormError::ListenPortRange => "listen_port must be 0-65535",
            PfFormError::TargetPortRange => "target_port must be 0-65535",
            PfFormError::TargetHostRequired => "target_host required for -L/-R",
        }
    }
}

/// Build a single-line `TextArea` pre-filled with `initial`, with the
/// cursor placed at the end.
fn make_textarea(initial: &str) -> TextArea<'static> {
    let mut ta = TextArea::new(vec![initial.to_string()]);
    // Move cursor to end of line so typing appends.
    ta.move_cursor(ratatui_textarea::CursorMove::End);
    ta
}

impl PfAddForm {
    pub fn default_for(mode: crate::config::ForwardMode) -> Self {
        Self {
            mode,
            focus: PfField::ListenPort,
            bind_addr: make_textarea("0.0.0.0"),
            listen_port: make_textarea(""),
            target_host: make_textarea("127.0.0.1"),
            target_port: make_textarea(""),
            submitting: false,
        }
    }

    /// Read the current text of a field. Returns `""` for `Mode`.
    pub fn field_text(&self, field: PfField) -> &str {
        match field {
            PfField::Mode => "",
            PfField::BindAddr => textarea_line(&self.bind_addr),
            PfField::ListenPort => textarea_line(&self.listen_port),
            PfField::TargetHost => textarea_line(&self.target_host),
            PfField::TargetPort => textarea_line(&self.target_port),
        }
    }

    /// Mutable handle to the focused field's textarea. `None` for `Mode`.
    pub fn focused_textarea_mut(&mut self) -> Option<&mut TextArea<'static>> {
        match self.focus {
            PfField::Mode => None,
            PfField::BindAddr => Some(&mut self.bind_addr),
            PfField::ListenPort => Some(&mut self.listen_port),
            PfField::TargetHost => Some(&mut self.target_host),
            PfField::TargetPort => Some(&mut self.target_port),
        }
    }

    pub fn validate(&self) -> Result<crate::config::ForwardSpec, PfFormError> {
        use crate::config::{ForwardMode, ForwardSpec};
        // Belt-and-braces: input filtering already blocks whitespace, but
        // trim defensively so any value that somehow made it through is
        // persisted clean. Port range is 0..=65535 — `u16::parse` already
        // enforces the upper bound; port 0 means "let kernel pick" and is
        // accepted.
        let listen_port: u16 = self
            .field_text(PfField::ListenPort)
            .trim()
            .parse()
            .map_err(|_| PfFormError::ListenPortRange)?;
        let bind_raw = self.field_text(PfField::BindAddr).trim();
        let bind_addr = if bind_raw.is_empty() {
            None
        } else {
            Some(bind_raw.to_string())
        };

        match self.mode {
            ForwardMode::Dynamic => Ok(ForwardSpec {
                mode: ForwardMode::Dynamic,
                bind_addr,
                listen_port,
                target_host: None,
                target_port: None,
            }),
            ForwardMode::Local | ForwardMode::Remote => {
                let target_host = self.field_text(PfField::TargetHost).trim();
                if target_host.is_empty() {
                    return Err(PfFormError::TargetHostRequired);
                }
                let target_port: u16 = self
                    .field_text(PfField::TargetPort)
                    .trim()
                    .parse()
                    .map_err(|_| PfFormError::TargetPortRange)?;
                Ok(ForwardSpec {
                    mode: self.mode,
                    bind_addr,
                    listen_port,
                    target_host: Some(target_host.to_string()),
                    target_port: Some(target_port),
                })
            }
        }
    }
}

/// First (only) line of a single-line `TextArea`, as a borrowed `&str`.
fn textarea_line<'a>(ta: &'a TextArea<'a>) -> &'a str {
    ta.lines().first().map(String::as_str).unwrap_or("")
}

#[derive(Debug, Clone)]
pub struct PortForwardOverlay {
    pub host: String,
    pub selected: usize,
    pub add_form: Option<PfAddForm>,
    pub status: Option<String>,
}

// --- Overlay state ---

/// UI state for transient sidebar overlays — help screen, kill-confirm
/// prompt, in-progress rename, right-click context menu, and the
/// exclude-pattern editor popup. Grouped so the renderer and key
/// dispatcher have a single place to ask "is any overlay active?".
///
/// `warning_state` (the nesting-detection banner) lives on `App` rather
/// than here because it is produced by the `NestingGuard` that App
/// owns, and the dispatch loop's "block actions while a warning is
/// up" gate reads it from App directly. Lifting it into AppState would
/// add an indirection without consolidating any logic.
#[derive(Debug, Default)]
pub struct OverlayState {
    pub show_help: bool,
    pub confirm_kill: bool,
    pub renaming: Option<RenameState>,
    pub context_menu: Option<ContextMenu>,
    pub exclude_editor: Option<ExcludeEditorState>,
    pub new_session: Option<NewSessionState>,
    /// Port-forward overlay for a single host. See `PortForwardOverlay`.
    pub port_forward: Option<PortForwardOverlay>,
}

// --- Settings page state ---

/// UI state for the settings page and its sub-popovers (theme picker,
/// keybindings viewer). Update-check fields stay on `AppState` because
/// they are read and written from many code paths outside the settings
/// page (refresh loop, banner rendering, mouse hit-testing).
#[derive(Debug, Default)]
pub struct SettingsState {
    /// Selected row in the settings page.
    pub selected: usize,

    /// Theme picker overlay (open inside the settings page).
    pub theme_picker_open: bool,
    pub theme_picker_selected: usize,

    /// Keybindings viewer overlay (read-only).
    pub keybindings_view_open: bool,
    pub keybindings_view_scroll: u16,
}

// --- AppState ---

pub struct AppState {
    // Session data
    pub sessions: Vec<SessionRow>,
    pub filtered: Vec<usize>,
    pub focused: usize,
    pub current_session: String,
    pub session_order: Vec<String>,
    /// Tmux sessions discovered on configured remote hosts. Rendered
    /// in the sidebar below local sessions. Focus into them goes via
    /// `FocusTarget` flat index after `filtered.len()`, not the local
    /// `filtered` index.
    pub remote_sessions: Vec<RemoteSessionRow>,

    // UI state
    pub main_view: MainView,
    pub focus_mode: FocusMode,
    pub theme_index: usize,
    /// Settings page navigation + theme picker / keybindings viewer
    /// overlays. See `SettingsState`.
    pub settings: SettingsState,
    pub layout_mode: LayoutMode,
    pub view_mode: ViewMode,
    pub sidebar_width: u16,
    pub sidebar_height: u16,
    pub show_borders: bool,
    pub dragging_separator: bool,

    /// Transient sidebar overlays — help, kill-confirm, rename, context
    /// menu, exclude editor. See `OverlayState`.
    pub overlay: OverlayState,

    // Terminal dimensions
    pub term_width: u16,
    pub term_height: u16,

    // Scroll throttle
    pub last_scroll: Instant,

    // Config
    pub exclude_patterns: Vec<String>,
    pub plugins: Vec<PluginConfig>,
    pub keybindings: Keybindings,

    // Update check
    pub update_check_mode: UpdateCheckMode,
    pub update_available: Option<UpdateStatus>,
    pub update_last_checked_secs: Option<u64>,
    /// Column range of the clickable "upgrade" span in the footer banner,
    /// captured during render for mouse hit-testing. (y, x_start, x_end).
    pub banner_upgrade_bounds: Option<Rect>,

    /// Result of the most recent manual config reload. Rendered in the
    /// sidebar footer and auto-cleared by the main loop after a short
    /// TTL — see `RELOAD_STATUS_OK_TTL` / `RELOAD_STATUS_ERR_TTL`.
    pub reload_status: Option<ReloadStatus>,
    pub reload_status_at: Option<Instant>,

    /// Click-regions for divider `[…]` buttons, refilled by the sidebar
    /// renderer each frame. Read by mouse dispatch.
    pub divider_hits: Vec<DividerHit>,

    /// Mirror of `Config.remotes` so reducers can read per-host forwards
    /// without round-tripping through dispatch. Kept in sync by startup
    /// and `reload_config`.
    pub config_remotes: Vec<crate::config::RemoteConfig>,
}

/// Auto-expiry windows for the sidebar reload banner. Success fades
/// fast; errors hang around long enough to read a parse message.
pub const RELOAD_STATUS_OK_TTL: std::time::Duration = std::time::Duration::from_secs(2);
pub const RELOAD_STATUS_ERR_TTL: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Debug, Clone)]
pub enum ReloadStatus {
    Ok,
    Err(String),
}

impl ReloadStatus {
    pub fn ttl(&self) -> std::time::Duration {
        match self {
            ReloadStatus::Ok => RELOAD_STATUS_OK_TTL,
            ReloadStatus::Err(_) => RELOAD_STATUS_ERR_TTL,
        }
    }
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        theme_index: usize,
        layout_mode: LayoutMode,
        view_mode: ViewMode,
        show_borders: bool,
        sidebar_width: u16,
        sidebar_height: u16,
        term_width: u16,
        term_height: u16,
        exclude_patterns: Vec<String>,
        plugins: Vec<PluginConfig>,
        keybindings: Keybindings,
        update_check_mode: UpdateCheckMode,
    ) -> Self {
        Self {
            sessions: Vec::new(),
            filtered: Vec::new(),
            focused: 0,
            current_session: String::new(),
            session_order: Vec::new(),
            remote_sessions: Vec::new(),
            main_view: MainView::Terminal,
            focus_mode: FocusMode::Main,
            theme_index,
            settings: SettingsState {
                selected: 0,
                theme_picker_open: false,
                theme_picker_selected: theme_index,
                keybindings_view_open: false,
                keybindings_view_scroll: 0,
            },
            layout_mode,
            view_mode,
            sidebar_width,
            sidebar_height,
            show_borders,
            dragging_separator: false,
            overlay: OverlayState::default(),
            term_width,
            term_height,
            last_scroll: Instant::now(),
            exclude_patterns,
            plugins,
            keybindings,
            update_check_mode,
            update_available: None,
            update_last_checked_secs: None,
            banner_upgrade_bounds: None,
            reload_status: None,
            reload_status_at: None,
            divider_hits: Vec::new(),
            config_remotes: Vec::new(),
        }
    }

    /// Drop the reload banner once its per-variant TTL has elapsed.
    /// Called from the main loop so rendering stays side-effect-free.
    pub fn tick_reload_status(&mut self, now: Instant) {
        if let (Some(status), Some(shown_at)) = (&self.reload_status, self.reload_status_at) {
            if now.saturating_duration_since(shown_at) >= status.ttl() {
                self.reload_status = None;
                self.reload_status_at = None;
            }
        }
    }

    pub fn banner_upgrade_at(&self, col: u16, row: u16) -> bool {
        match self.banner_upgrade_bounds {
            Some(r) => col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height,
            None => false,
        }
    }

    pub fn effective_sidebar_height(&self) -> u16 {
        let (min_height, max_height) = self.sidebar_height_bounds();
        self.sidebar_height.clamp(min_height, max_height)
    }

    fn sidebar_width_bounds(&self) -> (u16, u16) {
        let max_width = SIDEBAR_MAX.min(self.term_width.saturating_sub(MIN_MAIN_WIDTH));
        if max_width < SIDEBAR_MIN {
            let fallback = max_width.max(1);
            (fallback, fallback)
        } else {
            (SIDEBAR_MIN, max_width)
        }
    }

    fn sidebar_height_bounds(&self) -> (u16, u16) {
        let (min_height, max_height, available_height) = if self.show_borders {
            (
                SIDEBAR_HEIGHT_MIN_BORDERED,
                SIDEBAR_HEIGHT_MAX_BORDERED,
                self.term_height.saturating_sub(2 + MIN_MAIN_HEIGHT),
            )
        } else {
            (
                SIDEBAR_HEIGHT_MIN,
                SIDEBAR_HEIGHT_MAX,
                self.term_height.saturating_sub(MIN_MAIN_HEIGHT),
            )
        };
        let max_height = max_height.min(available_height);
        if max_height < min_height {
            let fallback = max_height.max(1);
            (fallback, fallback)
        } else {
            (min_height, max_height)
        }
    }

    pub fn pty_size(&self) -> (u16, u16) {
        let bo = if self.show_borders { 2u16 } else { 0 };
        match self.layout_mode {
            LayoutMode::Horizontal => {
                let cols = self
                    .term_width
                    .saturating_sub(self.sidebar_width + 1 + bo)
                    .max(1);
                let rows = self.term_height.saturating_sub(bo).max(1);
                (rows, cols)
            }
            LayoutMode::Vertical => {
                let cols = self.term_width.saturating_sub(bo).max(1);
                let rows = self
                    .term_height
                    .saturating_sub(self.effective_sidebar_height() + bo)
                    .max(1);
                (rows, cols)
            }
        }
    }

    /// Height of the sidebar footer in rows, mirroring what `draw_sidebar`
    /// allocates. Kept on AppState so mouse hit-testing doesn't drift
    /// from the renderer when plugins or the update banner change it.
    pub fn sidebar_footer_height(&self) -> u16 {
        let b = if self.show_borders { 2u16 } else { 0 };
        let content_width = match self.layout_mode {
            LayoutMode::Horizontal => self.sidebar_width.saturating_sub(b),
            LayoutMode::Vertical => self.term_width.saturating_sub(b),
        };
        let banner_visible = self.update_available.is_some() && content_width >= BANNER_MIN_WIDTH;
        3 + banner_visible as u16 + plugin_block_rows(self.plugins.len())
    }

    /// Map a screen row to a sidebar focus target. Walks the unified
    /// layout (local cards + remote groups + headers) so variable-
    /// height rows hit-test correctly.
    pub fn focus_at_row(&self, row: u16) -> Option<FocusTarget> {
        let b = if self.show_borders { 1u16 } else { 0 };
        let sidebar_h = match self.layout_mode {
            LayoutMode::Horizontal => self.term_height,
            LayoutMode::Vertical => self.effective_sidebar_height(),
        };
        let header_height = 2u16;
        let footer_height = self.sidebar_footer_height();
        let sessions_top = b + header_height;
        let sessions_bottom = sidebar_h.saturating_sub(b + footer_height);
        if row < sessions_top || row >= sessions_bottom {
            return None;
        }
        let visible_height = (sessions_bottom - sessions_top) as usize;

        let layout = self.sidebar_layout(self.view_mode);
        // Compute the same scroll offset the renderer uses (see
        // `scroll_for_layout`) so click coordinates and rendered
        // positions agree.
        let scroll = scroll_for_layout(&layout, self.focus_target(), visible_height);
        let clicked_y = row as usize - sessions_top as usize + scroll;
        layout.target_at_y(clicked_y)
    }

    /// Map a screen column to a tab index in vertical/tabs mode.
    pub fn session_at_col(&self, col: u16, row: u16) -> Option<usize> {
        let b = if self.show_borders { 1u16 } else { 0 };
        if row != b {
            return None;
        }
        let names: Vec<&str> = self
            .filtered
            .iter()
            .map(|&i| self.sessions[i].name.as_str())
            .collect();
        let ranges = tab_col_ranges(&names);
        let local_col = col.saturating_sub(b);
        for (i, &(start, end)) in ranges.iter().enumerate() {
            if local_col >= start && local_col < end {
                return Some(i);
            }
        }
        None
    }

    /// Total number of focusable rows in the sidebar: local sessions
    /// (after filtering) followed by remote sessions.
    pub fn focusable_count(&self) -> usize {
        self.filtered.len() + self.remote_sessions.len()
    }

    /// Build the unified sidebar layout: a flat list of header /
    /// session items in render order. Renderers and the mouse
    /// hit-tester share this so they can't disagree about which row
    /// lives where.
    pub fn sidebar_layout(&self, view_mode: ViewMode) -> SidebarLayout {
        let card_h = card_height(view_mode);
        let mut items = Vec::with_capacity(self.filtered.len() + self.remote_sessions.len() + 4);

        // Local group: no header, the fixed "Projects (N)" banner at
        // the top of the sidebar already labels this section. Flat
        // index for a local row equals its filtered_pos.
        for pos in 0..self.filtered.len() {
            items.push(SidebarItem {
                kind: SidebarItemKind::Session { session_idx: pos },
                height: card_h,
            });
        }

        // Remote groups: detect host transitions in render order
        // (which matches focus order — `remote_sessions` is already
        // grouped by host because the refresh worker emits hosts in
        // config order, one block at a time). Flat index for a remote
        // row is filtered.len() + remote_idx.
        let local_count = self.filtered.len();
        let mut host_idx: usize = 0;
        let mut prev_host: Option<&str> = None;
        let show_host_headers = matches!(view_mode, ViewMode::Expanded);
        for (remote_idx, r) in self.remote_sessions.iter().enumerate() {
            let new_host = Some(r.host.as_str()) != prev_host;
            if new_host {
                if prev_host.is_some() {
                    host_idx += 1;
                }
                if show_host_headers {
                    items.push(SidebarItem {
                        kind: SidebarItemKind::Header {
                            label: format!("  @{}", r.host),
                            host_idx,
                        },
                        height: 1,
                    });
                }
                prev_host = Some(r.host.as_str());
            }
            // Match the local card height so groups visually align
            // (3 rows in Expanded, 1 in Compact). The renderer pads
            // the bottom of each row with blank lines to fill the card.
            items.push(SidebarItem {
                kind: SidebarItemKind::Session {
                    session_idx: local_count + remote_idx,
                },
                height: card_h,
            });
        }

        SidebarLayout { items }
    }

    /// Resolve a focus target back to the backing row in either local
    /// or remote storage. This is the *only* place the rest of the app
    /// needs to do local-vs-remote dispatch — reducers and refresh
    /// match on the returned `SessionTargetRef` instead of taking apart
    /// the flat index themselves.
    pub fn session_target(&self, target: FocusTarget) -> Option<SessionTargetRef<'_>> {
        let idx = target.0;
        let local_count = self.filtered.len();
        if idx < local_count {
            let row_idx = *self.filtered.get(idx)?;
            self.sessions.get(row_idx).map(SessionTargetRef::Local)
        } else {
            self.remote_sessions
                .get(idx - local_count)
                .map(SessionTargetRef::Remote)
        }
    }

    /// Decode the current `focused` index into a structured target.
    /// Returns `None` if nothing is focusable (empty sidebar).
    pub fn focus_target(&self) -> Option<FocusTarget> {
        if self.focused < self.focusable_count() {
            Some(FocusTarget(self.focused))
        } else {
            None
        }
    }

    /// Map a screen position to a context menu item index.
    pub fn menu_item_at(&self, col: u16, row: u16) -> Option<usize> {
        let menu = self.overlay.context_menu.as_ref()?;
        let items = menu.items();
        let menu_width = context_menu_width(items);
        let menu_height = items.len() as u16 + 2;
        let mx = menu.x.min(self.term_width.saturating_sub(menu_width));
        let my = menu.y.min(self.term_height.saturating_sub(menu_height));
        if col > mx && col < mx + menu_width - 1 && row > my && row < my + menu_height - 1 {
            let idx = (row - my - 1) as usize;
            if idx < items.len() {
                return Some(idx);
            }
        }
        None
    }

    // --- Filtering and ordering ---

    pub fn recompute_filter(&mut self) {
        self.filtered = (0..self.sessions.len()).collect();
        // Clamp focused to the unified focusable range (local + remote).
        // Without remotes this collapses to the original local-only
        // behavior; with remotes it keeps focus inside the remote
        // section after the local list shrinks.
        let total = self.focusable_count();
        if total > 0 && self.focused >= total {
            self.focused = total - 1;
        }
    }

    pub fn sync_order(&mut self) {
        let names: Vec<String> = self.sessions.iter().map(|s| s.name.clone()).collect();
        self.session_order.retain(|n| names.contains(n));
        for name in &names {
            if !self.session_order.contains(name) {
                self.session_order.push(name.clone());
            }
        }
    }

    pub fn apply_order(&mut self) {
        let order = &self.session_order;
        self.sessions.sort_by_key(|s| {
            order
                .iter()
                .position(|n| n == &s.name)
                .unwrap_or(usize::MAX)
        });
    }

    /// Clamp and set sidebar width. Returns true if it changed.
    pub fn resize_sidebar(&mut self, new_width: u16) -> bool {
        let (min_width, max_width) = self.sidebar_width_bounds();
        let clamped = new_width.clamp(min_width, max_width);
        if clamped == self.sidebar_width {
            return false;
        }
        self.sidebar_width = clamped;
        true
    }

    /// Clamp and set sidebar height. Returns true if it changed.
    pub fn resize_sidebar_height(&mut self, new_height: u16) -> bool {
        let (min_height, max_height) = self.sidebar_height_bounds();
        let clamped = new_height.clamp(min_height, max_height);
        if clamped == self.sidebar_height {
            return false;
        }
        self.sidebar_height = clamped;
        true
    }
}

#[cfg(test)]
#[path = "../../tests/unit/model/state.rs"]
mod tests;

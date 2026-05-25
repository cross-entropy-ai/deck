use std::time::Instant;

use ratatui::layout::Rect;
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

const SESSION_MENU_ITEMS: &'static [&'static str] =
    &["Switch", "Rename", "Kill", "Move up", "Move down"];
// Remote sessions live on a different tmux server, so the
// deck-side `session_order` (which drives Move up/down) doesn't
// apply. Switch/Rename/Kill all map to `ssh <host> tmux <cmd>`
// against the host's server, where `(host, name)` uniquely
// identifies the session.
const REMOTE_SESSION_MENU_ITEMS: &'static [&'static str] = &["Switch", "Rename", "Kill"];
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
    Session(FocusTarget),
    Global,
}

impl MenuKind {
    pub fn items(&self) -> &'static [&'static str] {
        match self {
            MenuKind::Session(FocusTarget::Local(_)) => SESSION_MENU_ITEMS,
            MenuKind::Session(FocusTarget::Remote(_)) => REMOTE_SESSION_MENU_ITEMS,
            MenuKind::Global => GLOBAL_MENU_ITEMS,
        }
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
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub is_current: bool,
    pub idle_seconds: u64,
    /// Raw activity status, pre-ack.
    pub status: SessionStatus,
}

/// One tmux session living on a remote host. Modeled separately from
/// `SessionRow` so the existing local-only invariants (session_order,
/// notification ack maps, validate_session_name, kill/rename dispatch)
/// don't have to grow an `origin` discriminator on every touchpoint.
/// Phase 2 step 5 will revisit this once remote operations land.
#[derive(Debug, Clone)]
pub struct RemoteSessionRow {
    pub host: String,
    pub name: String,
    pub dir: String,
    pub idle_seconds: u64,
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

/// Identifies a sidebar row in the unified focus model. Local rows
/// follow the existing `filtered` order; remote rows pick up where
/// local ones end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    /// Index into `state.filtered` (which itself indexes into
    /// `state.sessions`).
    Local(usize),
    /// Index into `state.remote_sessions`.
    Remote(usize),
}

/// Logical sidebar group. Renderers use the group identity to pick
/// the bg color and the layout to decide where headers go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Local,
    /// Position of the host in the rendered list (NOT in the config
    /// list — the order matches the sequence of distinct hosts in
    /// `state.remote_sessions`). Used to pick a per-host bg tint.
    Remote(usize),
}

/// One renderable item in the sidebar layout — either a non-focusable
/// group header or a focusable session row. Both `SidebarLayout`
/// consumers — the sidebar renderer and `session_at_row` — walk the
/// same list, so highlight, scroll, and mouse hit-testing agree about
/// where every row lives.
#[derive(Debug, Clone)]
pub struct SidebarItem {
    pub kind: SidebarItemKind,
    pub group: GroupKind,
    /// Number of terminal rows this item occupies.
    pub height: usize,
}

#[derive(Debug, Clone)]
pub enum SidebarItemKind {
    /// Group header (label). Not focusable.
    Header { label: String },
    /// A local session at `filtered_pos` in `state.filtered`. The flat
    /// focus index equals `filtered_pos`.
    LocalSession { filtered_pos: usize },
    /// A remote session at `remote_idx` in `state.remote_sessions`.
    /// Flat focus index = `state.filtered.len() + remote_idx`.
    RemoteSession { remote_idx: usize },
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
        self.iter_with_y().find(|(_, it)| match (&it.kind, target) {
            (SidebarItemKind::LocalSession { filtered_pos }, FocusTarget::Local(pos)) => {
                *filtered_pos == pos
            }
            (SidebarItemKind::RemoteSession { remote_idx }, FocusTarget::Remote(idx)) => {
                *remote_idx == idx
            }
            _ => false,
        })
    }

    /// Map a vertical offset (in rows, relative to the sidebar's
    /// scrollable area top) to a FocusTarget if it falls on a
    /// session row. Header rows return None.
    pub fn target_at_y(&self, y: usize) -> Option<FocusTarget> {
        for (top, it) in self.iter_with_y() {
            if y >= top && y < top + it.height {
                return match it.kind {
                    SidebarItemKind::LocalSession { filtered_pos } => {
                        Some(FocusTarget::Local(filtered_pos))
                    }
                    SidebarItemKind::RemoteSession { remote_idx } => {
                        Some(FocusTarget::Remote(remote_idx))
                    }
                    SidebarItemKind::Header { .. } => None,
                };
            }
        }
        None
    }
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
    pub input: String,
    pub cursor: usize,
    /// `Some(host)` when the rename targets a remote session.
    pub host: Option<String>,
}

/// UI state for the exclude pattern editor popup.
#[derive(Debug, Clone)]
pub struct ExcludeEditorState {
    pub selected: usize,
    pub adding: bool,
    pub input: String,
    pub cursor: usize,
    pub error: Option<String>,
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
    /// in the sidebar below local sessions; not part of `filtered` /
    /// `focused` until Phase 2 step 5 wires real selection.
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

    /// Back-compat shim for callers that only expect a local index.
    /// Returns `None` for remote rows so existing dispatch arms that
    /// haven't grown a remote branch yet (e.g. vertical/tabs mode)
    /// fall through to safe defaults.
    #[allow(dead_code)]
    pub fn session_at_row(&self, row: u16) -> Option<usize> {
        match self.focus_at_row(row) {
            Some(FocusTarget::Local(pos)) => Some(pos),
            _ => None,
        }
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
        // the top of the sidebar already labels this section.
        for pos in 0..self.filtered.len() {
            items.push(SidebarItem {
                kind: SidebarItemKind::LocalSession { filtered_pos: pos },
                group: GroupKind::Local,
                height: card_h,
            });
        }

        // Remote groups: detect host transitions in render order
        // (which matches focus order — `remote_sessions` is already
        // grouped by host because the refresh worker emits hosts in
        // config order, one block at a time).
        let mut host_idx: usize = 0;
        let mut prev_host: Option<&str> = None;
        for (remote_idx, r) in self.remote_sessions.iter().enumerate() {
            let new_host = Some(r.host.as_str()) != prev_host;
            if new_host {
                if prev_host.is_some() {
                    host_idx += 1;
                }
                items.push(SidebarItem {
                    kind: SidebarItemKind::Header {
                        label: format!("  @{}", r.host),
                    },
                    group: GroupKind::Remote(host_idx),
                    height: 1,
                });
                prev_host = Some(r.host.as_str());
            }
            // Match the local card height so groups visually align
            // (5 rows in Expanded, 2 in Compact). The renderer pads
            // the bottom of each remote row with blank lines on the
            // group bg to fill the card.
            items.push(SidebarItem {
                kind: SidebarItemKind::RemoteSession { remote_idx },
                group: GroupKind::Remote(host_idx),
                height: card_h,
            });
        }

        SidebarLayout { items }
    }

    /// Decode the current `focused` index into a structured target.
    /// Returns `None` if nothing is focusable (empty sidebar).
    pub fn focus_target(&self) -> Option<FocusTarget> {
        if self.focused < self.filtered.len() {
            Some(FocusTarget::Local(self.focused))
        } else {
            let remote_idx = self.focused - self.filtered.len();
            if remote_idx < self.remote_sessions.len() {
                Some(FocusTarget::Remote(remote_idx))
            } else {
                None
            }
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

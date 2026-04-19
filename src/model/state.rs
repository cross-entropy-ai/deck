use std::collections::HashMap;
use std::time::Instant;

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

use crate::config::PluginConfig;
use crate::keybindings::Keybindings;
use crate::layout::{card_height, context_menu_width, plugin_block_rows, tab_col_ranges, BANNER_MIN_WIDTH};
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

pub const SESSION_MENU_ITEMS: &[&str] = &["Switch", "Rename", "Kill", "Move up", "Move down"];
pub const GLOBAL_MENU_ITEMS: &[&str] = &[
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

/// Three-state session activity model.
///
/// - `Idle`: nothing demanding attention — shell at prompt, or a
///   Claude agent between turns whose last Waiting has been acked.
/// - `Working`: something is actively running in the pane, or Claude
///   is executing a tool / processing a turn.
/// - `Waiting`: Claude fired Stop or Notification and the user hasn't
///   visited the session since. Non-Claude programs never produce
///   `Waiting` in this version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionStatus {
    #[default]
    Idle,
    Working,
    Waiting,
}

pub const SETTINGS_ITEM_COUNT: usize = 7;

// --- Context menu ---

#[derive(Debug, Clone)]
pub enum MenuKind {
    Session { filtered_idx: usize },
    Global,
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub kind: MenuKind,
    pub items: Vec<&'static str>,
    pub x: u16,
    pub y: u16,
    pub selected: usize,
}

impl ContextMenu {
    pub fn items(&self) -> &[&'static str] {
        &self.items
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
    /// Unix-ms timestamp of the hook event that produced `status`. Only
    /// set when the status came from a Claude state file; used by the
    /// ack-on-detach override to decide whether a Waiting has been seen.
    pub status_event_ts_ms: Option<u64>,
}

// --- Side effects ---

#[derive(Debug, Default)]
pub struct SideEffect {
    pub switch_session: Option<String>,
    pub kill_session: Option<KillRequest>,
    pub rename_session: Option<RenameRequest>,
    pub create_session: bool,
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
        if other.kill_session.is_some() {
            self.kill_session = other.kill_session;
        }
        if other.rename_session.is_some() {
            self.rename_session = other.rename_session;
        }
        self.create_session |= other.create_session;
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
    pub switch_to: Option<String>,
}

/// Info needed to execute a rename.
#[derive(Debug)]
pub struct RenameRequest {
    pub old_name: String,
    pub new_name: String,
}

/// UI state for an in-progress rename.
#[derive(Debug, Clone)]
pub struct RenameState {
    pub original_name: String,
    pub input: String,
    pub cursor: usize,
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

// --- AppState ---

pub struct AppState {
    // Session data
    pub sessions: Vec<SessionRow>,
    pub filtered: Vec<usize>,
    pub focused: usize,
    pub current_session: String,
    pub session_order: Vec<String>,

    // UI state
    pub main_view: MainView,
    pub focus_mode: FocusMode,
    pub theme_index: usize,
    pub settings_selected: usize,
    pub theme_picker_open: bool,
    pub theme_picker_selected: usize,
    pub layout_mode: LayoutMode,
    pub view_mode: ViewMode,
    pub sidebar_width: u16,
    pub sidebar_height: u16,
    pub show_help: bool,
    pub confirm_kill: bool,
    pub renaming: Option<RenameState>,
    pub show_borders: bool,
    pub context_menu: Option<ContextMenu>,
    pub exclude_editor: Option<ExcludeEditorState>,
    pub hover_separator: bool,
    pub dragging_separator: bool,

    // Terminal dimensions
    pub term_width: u16,
    pub term_height: u16,

    // Scroll throttle
    pub last_scroll: Instant,

    // Config
    pub exclude_patterns: Vec<String>,
    pub plugins: Vec<PluginConfig>,
    pub keybindings: Keybindings,

    // Keybindings viewer (read-only settings page)
    pub keybindings_view_open: bool,
    pub keybindings_view_scroll: u16,

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

    /// Unix-ms timestamp at which the user last detached from each
    /// session. Used by the Waiting-ack override: if the latest Claude
    /// hook event for session S is older than `acked_ts_ms[S]`, the
    /// Waiting status is downgraded to Idle in the UI. In-memory only,
    /// so ack resets on deck restart.
    pub acked_ts_ms: HashMap<String, u64>,

    /// Per-session ts of the most recent Waiting event we already fired
    /// a desktop notification for. Stops us from re-notifying every
    /// refresh cycle while a session sits in Waiting.
    pub last_notified_ts_ms: HashMap<String, u64>,

    /// First snapshot is used to seed `last_notified_ts_ms` without
    /// firing notifications — otherwise restarting deck while any
    /// session was already Waiting would dump a notification per
    /// session into the user's tray.
    pub notifications_armed: bool,

    /// Whether the host terminal (Ghostty / iTerm2 / etc.) currently
    /// has OS-level keyboard focus. Updated from crossterm's
    /// `FocusGained` / `FocusLost` events. Used to gate the "you're
    /// already attached, no notification needed" check — if you're
    /// attached but looking at another macOS app, we still notify.
    pub terminal_focused: bool,
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
            main_view: MainView::Terminal,
            focus_mode: FocusMode::Main,
            theme_index,
            settings_selected: 0,
            theme_picker_open: false,
            theme_picker_selected: theme_index,
            layout_mode,
            view_mode,
            sidebar_width,
            sidebar_height,
            show_help: false,
            confirm_kill: false,
            renaming: None,
            show_borders,
            context_menu: None,
            exclude_editor: None,
            hover_separator: false,
            dragging_separator: false,
            term_width,
            term_height,
            last_scroll: Instant::now(),
            exclude_patterns,
            plugins,
            keybindings,
            keybindings_view_open: false,
            keybindings_view_scroll: 0,
            update_check_mode,
            update_available: None,
            update_last_checked_secs: None,
            banner_upgrade_bounds: None,
            reload_status: None,
            reload_status_at: None,
            acked_ts_ms: HashMap::new(),
            last_notified_ts_ms: HashMap::new(),
            notifications_armed: false,
            // Assume focused until the terminal tells us otherwise. The
            // alternative (false default) would race the first
            // FocusGained event and could fire spurious notifications
            // immediately after launch.
            terminal_focused: true,
        }
    }

    /// Apply the Waiting-ack override. A Waiting status whose underlying
    /// hook event is older than the user's last visit to that session is
    /// downgraded to Idle — the user has seen it, so stop drawing
    /// attention until a fresh hook event bumps the timestamp.
    pub fn effective_status(&self, row: &SessionRow) -> SessionStatus {
        if row.status != SessionStatus::Waiting {
            return row.status;
        }
        let event_ts = row.status_event_ts_ms.unwrap_or(0);
        let ack_ts = self.acked_ts_ms.get(&row.name).copied().unwrap_or(0);
        if event_ts <= ack_ts {
            SessionStatus::Idle
        } else {
            SessionStatus::Waiting
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
        let banner_visible =
            self.update_available.is_some() && content_width >= BANNER_MIN_WIDTH;
        3 + banner_visible as u16 + plugin_block_rows(self.plugins.len())
    }

    /// Map a screen row to a filtered session index (horizontal/card mode).
    pub fn session_at_row(&self, row: u16) -> Option<usize> {
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
        let visible_height = sessions_bottom - sessions_top;
        let ch = card_height(self.view_mode);
        let focused_bottom = (self.focused + 1) * ch;
        let visible = visible_height as usize;
        let scroll = focused_bottom.saturating_sub(visible);
        let clicked_row = row as usize - sessions_top as usize + scroll;
        let idx = clicked_row / ch;
        if idx < self.filtered.len() {
            Some(idx)
        } else {
            None
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

    /// Map a screen position to a context menu item index.
    pub fn menu_item_at(&self, col: u16, row: u16) -> Option<usize> {
        let menu = self.context_menu.as_ref()?;
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
        if !self.filtered.is_empty() && self.focused >= self.filtered.len() {
            self.focused = self.filtered.len() - 1;
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

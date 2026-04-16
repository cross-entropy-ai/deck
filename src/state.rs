use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::config::{ExcludePattern, PluginConfig};
use crate::ui::{self, SessionView, card_height};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    All,
    Working,
    Idle,
}

impl FilterMode {
    pub fn next(self) -> Self {
        match self {
            FilterMode::All => FilterMode::Working,
            FilterMode::Working => FilterMode::Idle,
            FilterMode::Idle => FilterMode::All,
        }
    }

    pub fn tab_label(self) -> &'static str {
        match self {
            FilterMode::All => "All",
            FilterMode::Idle => "Idle",
            FilterMode::Working => "Working",
        }
    }
}

pub const FILTER_TABS: [FilterMode; 3] = [FilterMode::All, FilterMode::Idle, FilterMode::Working];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Expanded,
    Compact,
}

pub const SETTINGS_ITEM_COUNT: usize = 5;

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
    pub filter_mode: FilterMode,
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
    pub compiled_patterns: Vec<ExcludePattern>,
    pub plugins: Vec<PluginConfig>,
}

impl AppState {
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
        compiled_patterns: Vec<ExcludePattern>,
        plugins: Vec<PluginConfig>,
    ) -> Self {
        Self {
            sessions: Vec::new(),
            filtered: Vec::new(),
            focused: 0,
            current_session: String::new(),
            filter_mode: FilterMode::All,
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
            compiled_patterns,
            plugins,
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

    /// Map a screen row to a filtered session index (horizontal/card mode).
    pub fn session_at_row(&self, row: u16) -> Option<usize> {
        let b = if self.show_borders { 1u16 } else { 0 };
        let sidebar_h = match self.layout_mode {
            LayoutMode::Horizontal => self.term_height,
            LayoutMode::Vertical => self.effective_sidebar_height(),
        };
        let header_height = 3u16;
        let footer_height = 3u16;
        let sessions_top = b + header_height;
        let sessions_bottom = sidebar_h.saturating_sub(b + footer_height);
        if row < sessions_top || row >= sessions_bottom {
            return None;
        }
        let visible_height = sessions_bottom - sessions_top;
        let ch = card_height(self.view_mode);
        let focused_bottom = (self.focused + 1) * ch;
        let visible = visible_height as usize;
        let scroll = if focused_bottom > visible {
            focused_bottom - visible
        } else {
            0
        };
        let clicked_row = row as usize - sessions_top as usize + scroll;
        let idx = clicked_row / ch;
        if idx < self.filtered.len() {
            Some(idx)
        } else {
            None
        }
    }

    pub fn filter_tab_at(&self, col: u16, row: u16) -> Option<FilterMode> {
        if self.layout_mode != LayoutMode::Horizontal {
            return None;
        }

        let b = if self.show_borders { 1u16 } else { 0 };
        let tab_row = b + 1;
        if row != tab_row {
            return None;
        }

        let mut x = 2u16;
        let local_col = col.saturating_sub(b);
        for mode in FILTER_TABS {
            let width = mode.tab_label().len() as u16 + 2;
            if local_col >= x && local_col < x + width {
                return Some(mode);
            }
            x += width + 1;
        }

        None
    }

    /// Map a screen column to a tab index in vertical/tabs mode.
    pub fn session_at_col(&self, col: u16, row: u16) -> Option<usize> {
        let b = if self.show_borders { 1u16 } else { 0 };
        if row != b {
            return None;
        }
        let views: Vec<SessionView> = self
            .filtered
            .iter()
            .map(|&i| {
                let s = &self.sessions[i];
                SessionView {
                    name: s.name.as_str(),
                    dir: s.dir.as_str(),
                    branch: s.branch.as_str(),
                    ahead: s.ahead,
                    behind: s.behind,
                    staged: s.staged,
                    modified: s.modified,
                    untracked: s.untracked,
                    idle_seconds: s.idle_seconds,
                }
            })
            .collect();
        let ranges = ui::tab_col_ranges(&views);
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
        let menu_width = ui::context_menu_width(items);
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
        self.filtered = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| match self.filter_mode {
                FilterMode::All => true,
                FilterMode::Working => s.idle_seconds < 3,
                FilterMode::Idle => s.idle_seconds >= 3,
            })
            .map(|(i, _)| i)
            .collect();

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
mod tests {
    use super::*;

    fn make_session(name: &str) -> SessionRow {
        SessionRow {
            name: name.to_string(),
            dir: format!("/tmp/{name}"),
            branch: "main".to_string(),
            ahead: 0,
            behind: 0,
            staged: 0,
            modified: 0,
            untracked: 0,
            is_current: false,
            idle_seconds: 0,
        }
    }

    fn make_state(
        layout_mode: LayoutMode,
        show_borders: bool,
        term_width: u16,
        term_height: u16,
    ) -> AppState {
        let mut state = AppState::new(
            0,
            layout_mode,
            ViewMode::Expanded,
            show_borders,
            28,
            SIDEBAR_HEIGHT,
            term_width,
            term_height,
            vec![],
            vec![],
            vec![],
        );
        state.sessions = vec![make_session("alpha"), make_session("beta")];
        state.session_order = state.sessions.iter().map(|s| s.name.clone()).collect();
        state.recompute_filter();
        state
    }

    #[test]
    fn resize_sidebar_handles_small_terminals() {
        let mut state = make_state(LayoutMode::Horizontal, true, 20, 40);
        assert!(state.resize_sidebar(30));
        assert_eq!(state.sidebar_width, 10);
    }

    #[test]
    fn vertical_sidebar_height_affects_layout() {
        let mut state = make_state(LayoutMode::Vertical, true, 120, 40);
        assert_eq!(state.effective_sidebar_height(), 4);

        assert!(state.resize_sidebar_height(6));
        assert_eq!(state.effective_sidebar_height(), 6);
        assert_eq!(state.pty_size(), (32, 118));
    }

    #[test]
    fn vertical_tab_hit_testing_only_uses_tab_row() {
        let state = make_state(LayoutMode::Vertical, true, 120, 40);

        assert_eq!(state.session_at_col(2, 1), Some(0));
        assert_eq!(state.session_at_col(2, 2), None);
    }
}

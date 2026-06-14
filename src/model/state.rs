use std::collections::{HashMap, HashSet};
use std::time::Instant;

use ratatui::layout::Position;
use ratatui_sectioned_list::widget::BasicItem;
use ratatui_sectioned_list::ItemKind;
use serde::{Deserialize, Serialize};

use crate::config::PluginConfig;
use crate::geometry::{context_menu_rect, host_accent, shorten_dir, tab_col_ranges, tab_label};
use crate::host_key::{HostKey, HostQuery};
use crate::keybindings::Keybindings;
use crate::update::{UpdateCheckMode, UpdateStatus};

// Re-export the model types that were split out of this file so the
// pervasive `crate::state::X` references across the UI / app / test layers
// keep resolving without churn. Each type's real home is the named module.
pub use crate::effects::{
    CreateSessionRequest, Effect, KillRequest, RemoteSwitchRequest, RenameRequest, SideEffect,
};
pub use crate::forwards::{ForwardHealth, ForwardKey, PfAddForm, PfField, PortForwardOverlay};
pub use crate::geometry::{
    AgentHit, AgentRow, AgentTarget, BuiltLayout, DividerButton, DividerHit, HitKind, HitRegions,
    KillConfirmHits, SectionMeta, SidebarLayout, SummaryHits, TabRects,
};
pub use crate::menu::{session_menu_disabled, ContextMenu, MenuItem, MenuKind};
pub use crate::overlay::{ExcludeEditorState, Modal, OverlayState, RenameState, WarningState};
// The Summary card lives in `model::summary`; re-export its types + the
// height constants here so the pervasive `crate::state::SummaryState` /
// `crate::state::DEFAULT_SUMMARY_HEIGHT` references keep resolving.
pub use crate::summary_card::{
    SummaryCard, SummaryState, DEFAULT_SUMMARY_HEIGHT, SUMMARY_MAX_HEIGHT, SUMMARY_MIN_HEIGHT,
};

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

/// Which sidebar tab is active. `Projects` lists tmux sessions (the
/// default view); `Agents` lists the detected coding agents as the
/// primary, navigable list. Persisted to config so the choice survives
/// a restart. Agent detection in the refresh worker runs only while the
/// `Agents` tab is active (see `AppState::agents_tab_active`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SidebarTab {
    #[default]
    Projects,
    Agents,
}

pub const FRAME_RATE_LIMIT_OPTIONS: [u16; 4] = [2, 5, 10, 30];

/// How often the Agents tab probes for agents + their status, in seconds.
/// Cycled in settings; labelled fast / normal / slow / very slow.
pub const AGENTS_PROBE_INTERVAL_OPTIONS: [u64; 4] = [1, 2, 5, 10];
pub const DEFAULT_AGENTS_PROBE_INTERVAL: u64 = 2;

pub fn normalize_agents_probe_interval(secs: u64) -> u64 {
    if AGENTS_PROBE_INTERVAL_OPTIONS.contains(&secs) {
        secs
    } else {
        DEFAULT_AGENTS_PROBE_INTERVAL
    }
}

pub fn agents_probe_interval_label(secs: u64) -> &'static str {
    match normalize_agents_probe_interval(secs) {
        1 => "1s (fast)",
        2 => "2s (normal)",
        5 => "5s (slow)",
        10 => "10s (very slow)",
        _ => "2s (normal)",
    }
}

/// Step `delta` positions through `options` from `current`, wrapping at
/// both ends. A `current` not in the slice steps from the first option.
/// Shared by the settings cyclers and the port-forward form's field/mode
/// cycling so the wrap-around arithmetic lives once.
pub fn cycle_option<T: Copy + PartialEq>(options: &[T], current: T, delta: i32) -> T {
    let i = options.iter().position(|&o| o == current).unwrap_or(0) as i32;
    let n = options.len() as i32;
    options[(i + delta).rem_euclid(n) as usize]
}

/// Step `current` by `direction` (+1/-1) within `0..len`, clamped at both
/// ends (no wrap). `len == 0` yields 0. Shared by the bounded list cursors
/// (settings rows, theme picker, exclude editor, port-forward focus,
/// new-session / add-remote pickers) so the saturating arithmetic lives once.
pub fn step_clamped(current: usize, len: usize, direction: i32) -> usize {
    if len == 0 {
        return 0;
    }
    let last = len - 1;
    if direction >= 0 {
        current.saturating_add(direction as usize).min(last)
    } else {
        current.saturating_sub(direction.unsigned_abs() as usize)
    }
}

pub fn normalize_frame_rate_limit(fps: u16) -> u16 {
    if FRAME_RATE_LIMIT_OPTIONS.contains(&fps) {
        fps
    } else {
        5
    }
}

pub fn frame_rate_limit_label(fps: u16) -> &'static str {
    match normalize_frame_rate_limit(fps) {
        2 => "Power Saver 2 FPS",
        5 => "Balanced 5 FPS",
        10 => "Responsive 10 FPS",
        30 => "Smooth 30 FPS",
        _ => "Balanced 5 FPS",
    }
}

// --- Session data ---

/// One row in the unified sidebar session store. Local and remote sessions
/// share this single shape, keyed by `host` the deck way (`None` = the local
/// tmux server, `Some(host)` = a remote one over ssh) — applying the repo's
/// "one data type, key by `Option<String>` host" rule to what was its oldest
/// exception (two parallel `SessionRow` + `RemoteSessionRow` stores stitched
/// by flat-index arithmetic). `kind` replaces the old `loading`/`unreachable`
/// flags and the magic placeholder names — see [`SessionKind`].
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// `None` = local tmux server; `Some(host)` = a remote host over ssh.
    pub host: Option<String>,
    pub name: String,
    pub dir: String,
    pub kind: SessionKind,
}

/// What a [`SessionEntry`] represents. A `Live` row is a real attachable
/// tmux session; the other variants are the synthetic status placeholders
/// the sidebar shows for a remote group (one row per host) before/while a
/// real session list is available. These replace the old
/// `loading`/`unreachable` booleans *and* the `"(no sessions)"` /
/// `"(unreachable)"` magic session names — so a real session literally named
/// `(no sessions)` is no longer mistaken for a placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// A real tmux session deck can attach to. `is_current` is tracked for
    /// local sessions only (remote `is_current` was never tracked, so remote
    /// `Live` rows carry `false`).
    Live { is_current: bool },
    /// Synthetic placeholder: the host's ssh+tmux query hasn't returned yet
    /// (was `RemoteSessionRow.loading`). Renders muted "(connecting...)".
    Connecting,
    /// Synthetic placeholder: deck couldn't reach the host over ssh (was
    /// `RemoteSessionRow.unreachable`). Renders greyed, "(unreachable)".
    Unreachable,
    /// Synthetic placeholder: the host responded but its tmux server isn't
    /// up, so it has nothing to attach to (was the `"(no sessions)"` magic
    /// name). Renders "(no sessions)".
    NoSessions,
}

/// Display label for the synthetic "(no sessions)" placeholder.
pub const NO_SESSIONS_LABEL: &str = "(no sessions)";
/// Display label for the synthetic "(unreachable)" placeholder.
pub const UNREACHABLE_LABEL: &str = "(unreachable)";

impl SessionEntry {
    /// Whether deck can attach a PTY to this entry — i.e. it is a real
    /// (`Live`) session, not a synthetic Connecting/Unreachable/NoSessions
    /// placeholder. The attach/respawn machinery must skip placeholders;
    /// otherwise it spins forever trying to `tmux attach` a host with
    /// nothing to attach to, leaving the row stuck on "connecting…".
    pub fn is_attachable(&self) -> bool {
        matches!(self.kind, SessionKind::Live { .. })
    }

    /// True for the local tmux server (`host == None`).
    pub fn is_local(&self) -> bool {
        self.host.is_none()
    }

    /// Whether this `Live` row is the current (attached) session. Always
    /// false for placeholders and for remote rows (remote `is_current`
    /// isn't tracked).
    pub fn is_current(&self) -> bool {
        matches!(
            self.kind,
            SessionKind::Live {
                is_current: true,
                ..
            }
        )
    }
}

/// The attachable (`Live`) sessions on `host` (`None` = local), in display
/// order. The one filter behind the last-session-on-host kill policy and the
/// per-host name/order collectors, so the call sites can't drift.
pub fn attachable_on_host<'a>(
    entries: &'a [SessionEntry],
    host: Option<&'a str>,
) -> impl Iterator<Item = &'a SessionEntry> {
    entries
        .iter()
        .filter(move |e| e.host.as_deref() == host && e.is_attachable())
}

/// Identifies a focused sidebar row by its flat index.
///
/// The flat index walks the visible row list in render order, which is
/// exactly the order of `state.entries` (local entries first, then each
/// remote host's rows). `AppState::entry_at` decodes this back into the
/// entry for action dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusTarget(pub usize);

/// Connection state of a remote host, derived from its rows' reachability.
/// Drives the color of the divider's reconnect button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostStatus {
    Connected,
    Connecting,
    Unreachable,
}

/// Connection status carried by a single remote row. All rows of a host
/// share one status, so any row of the group represents it. Shared between
/// the divider header and `-R` forward-health derivation so both read the
/// host's state the same way.
fn host_status_of(e: &SessionEntry) -> HostStatus {
    match e.kind {
        SessionKind::Unreachable => HostStatus::Unreachable,
        SessionKind::Connecting => HostStatus::Connecting,
        SessionKind::Live { .. } | SessionKind::NoSessions => HostStatus::Connected,
    }
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

// --- Prefs ---

/// The round-trippable persisted preferences, grouped into one unit so the
/// config↔state mapping lives in exactly two places ([`Prefs::from_config`]
/// and [`Prefs::to_config`]) instead of being re-enumerated at every call
/// site. Each field mirrors a `Config` field (with the same default) and is
/// read widely across the UI/render/dispatch layers via `state.prefs.<f>`.
///
/// Deliberately NOT here (stay direct `AppState` / `App` fields):
/// - `keybindings` (compiled) / `raw_keybindings` (serializable, on `App`);
/// - `collapsed_sections` — runtime state seeded from config *once* at
///   startup and deliberately not re-applied on hot-reload (bug #14);
/// - `config_remotes` and the `summary_prompt_version` constant — App/runtime,
///   not user prefs.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub theme_index: usize,
    pub layout_mode: LayoutMode,
    pub show_borders: bool,
    /// Use the terminal's default (transparent) background instead of the
    /// theme's solid background color.
    pub transparent_bg: bool,
    /// Active sidebar tab. `Projects` lists tmux sessions; `Agents` lists
    /// detected agents as the primary list. Persisted to config. Agent
    /// detection in the refresh worker runs only while this is `Agents`
    /// (see `agents_tab_active`).
    pub sidebar_tab: SidebarTab,
    pub sidebar_width: u16,
    pub sidebar_height: u16,
    pub view_mode: ViewMode,
    pub frame_rate_limit: u16,
    pub exclude_patterns: Vec<String>,
    pub plugins: Vec<PluginConfig>,
    pub update_check_mode: UpdateCheckMode,
    /// The summary prompt template (from config), `{{SESSIONS}}` filled
    /// with the agent panes at generation time. Seeded in `App::new` and
    /// refreshed on config reload.
    pub summary_prompt: String,
    /// Model passed to `claude --model` for the summary (from config); empty
    /// follows the user's Claude Code default.
    pub summary_model: String,
    /// Body height (text rows) of the inline summary card, drag-adjustable
    /// from its bottom edge and persisted to config.
    pub summary_height: u16,
    /// Language the summary is asked to use (from config); empty = default.
    pub summary_language: String,
    /// How often the Agents tab probes (seconds). Drives the refresh cadence
    /// while that tab is active; see `App`'s run loop.
    pub agents_probe_interval_secs: u64,
}

impl Prefs {
    /// Build prefs from a loaded `Config`, applying the load-time
    /// clamps/normalizations. `theme_index` is resolved by the caller (the
    /// theme name → index lookup lives outside config). This is one of the
    /// two config↔prefs mapping sites; its inverse is [`Prefs::to_config`].
    ///
    /// `summary_height` is normalized via the same clamp `set_summary_height`
    /// uses (`SUMMARY_MIN_HEIGHT..=SUMMARY_MAX_HEIGHT`).
    pub fn from_config(cfg: &crate::config::Config, theme_index: usize) -> Self {
        Self {
            theme_index,
            layout_mode: cfg.layout,
            show_borders: cfg.show_borders,
            transparent_bg: cfg.transparent_bg,
            sidebar_tab: cfg.sidebar_tab,
            sidebar_width: cfg.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX),
            sidebar_height: cfg.sidebar_height,
            view_mode: cfg.view_mode,
            frame_rate_limit: normalize_frame_rate_limit(cfg.frame_rate_limit),
            exclude_patterns: cfg.exclude_patterns.clone(),
            plugins: cfg.plugins.clone(),
            update_check_mode: cfg.update_check,
            summary_prompt: cfg.summary_prompt.clone(),
            summary_model: cfg.summary_model.clone(),
            summary_height: cfg
                .summary_height
                .clamp(SUMMARY_MIN_HEIGHT, SUMMARY_MAX_HEIGHT),
            summary_language: cfg.summary_language.clone(),
            agents_probe_interval_secs: normalize_agents_probe_interval(cfg.agents_probe_interval),
        }
    }

    /// Map the prefs back into a `Config`, filling the App/runtime-level
    /// fields that don't live in `Prefs` from the caller-supplied arguments
    /// (`keybindings`, `remotes`, `collapsed`) and the
    /// `summary_prompt_version` constant. The inverse of [`Prefs::from_config`];
    /// `from_config(to_config(p)) == p` holds on the prefs fields.
    pub fn to_config(
        &self,
        keybindings: std::collections::BTreeMap<String, crate::config::KeyBindingValue>,
        remotes: Vec<crate::config::RemoteConfig>,
        collapsed: Vec<Option<String>>,
    ) -> crate::config::Config {
        crate::config::Config {
            theme: crate::theme::THEMES[self.theme_index].name.to_string(),
            layout: self.layout_mode,
            show_borders: self.show_borders,
            sidebar_tab: self.sidebar_tab,
            sidebar_width: self.sidebar_width,
            sidebar_height: self.sidebar_height,
            view_mode: self.view_mode,
            frame_rate_limit: self.frame_rate_limit,
            exclude_patterns: self.exclude_patterns.clone(),
            plugins: self.plugins.clone(),
            keybindings,
            update_check: self.update_check_mode,
            remotes,
            collapsed_sections: collapsed,
            summary_prompt: self.summary_prompt.clone(),
            summary_prompt_version: crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION,
            summary_model: self.summary_model.clone(),
            summary_height: self.summary_height,
            summary_language: self.summary_language.clone(),
            agents_probe_interval: self.agents_probe_interval_secs,
            transparent_bg: self.transparent_bg,
        }
    }
}

// --- AppState ---

pub struct AppState {
    // Session data
    /// The single unified session store: local entries first (`host ==
    /// None`), then each remote host's rows in config order — exactly the
    /// sidebar render/flat order. Replaces the old parallel `sessions` +
    /// `remote_sessions` stores; the `FocusTarget` flat index is a direct
    /// index into this vec, and per-row dispatch reads `entry.host` /
    /// `entry.kind` instead of decoding `idx - local_count`.
    pub entries: Vec<SessionEntry>,
    pub focused: usize,
    /// Name of the current (attached) *local* session. Remote current-
    /// session isn't tracked (a deliberate local-only invariant carried
    /// over from the two-store model).
    pub current_session: String,
    /// Manual display order of *local* sessions, keyed by name. Remote
    /// reorder persists per-host to each remote tmux server instead.
    pub session_order: Vec<String>,

    // UI state
    pub main_view: MainView,
    pub focus_mode: FocusMode,
    /// The round-trippable persisted preferences (theme, layout, sidebar
    /// geometry, summary settings, …). See [`Prefs`]. Read widely as
    /// `state.prefs.<field>`; loaded via [`AppState::apply_config`] and
    /// written back via [`Prefs::to_config`].
    pub prefs: Prefs,
    /// Settings page navigation + theme picker / keybindings viewer
    /// overlays. See `SettingsState`.
    pub settings: SettingsState,
    /// Cursor row for the Agents tab, kept separate from `focused` (the
    /// Projects cursor) so switching tabs preserves each one's position.
    /// Indexes into `agent_rows()`.
    pub agent_focused: usize,
    /// The Agents-tab "Summary" card's runtime state (generation state,
    /// scroll, drag, pre-generation snapshot). See [`SummaryCard`]. Persisted
    /// summary settings (prompt/model/height/language) live in `prefs`.
    pub summary: SummaryCard,
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
    pub keybindings: Keybindings,

    // Update check
    pub update_available: Option<UpdateStatus>,
    pub update_last_checked_secs: Option<u64>,

    /// Every clickable region the sidebar publishes, captured whole by the
    /// render loop each frame and consulted by mouse dispatch through
    /// [`HitRegions::hit`]. One field, not a dozen — geometry can't drift
    /// across the rect tests because they all decode from here.
    pub hit_regions: HitRegions,

    /// Result of the most recent manual config reload. Rendered in the
    /// sidebar footer and auto-cleared by the main loop after a short
    /// TTL — see `RELOAD_STATUS_OK_TTL` / `RELOAD_STATUS_ERR_TTL`.
    pub reload_status: Option<ReloadStatus>,
    pub reload_status_at: Option<Instant>,

    /// The agent deck last switched to (via an agent-line click). Its
    /// footer line renders highlighted as "you are here". Identified by
    /// `(host, pane_id)` so the highlight is uniform for local and remote
    /// — never branches on origin. Cleared by any non-agent switch.
    pub active_agent: Option<AgentTarget>,

    /// Mirror of `Config.remotes` so reducers can read per-host forwards
    /// without round-tripping through dispatch. Kept in sync by startup
    /// and `reload_config`.
    pub config_remotes: Vec<crate::config::RemoteConfig>,

    /// Per-forward liveness, refreshed each probe tick by the port-forward
    /// worker. Keyed by `ForwardKey`. Missing key = `Probing` (not yet seen).
    pub forward_health: HashMap<ForwardKey, ForwardHealth>,

    /// Interactive coding agents (Claude Code / Codex) detected per
    /// sidebar section, keyed by host the same way the rest of deck keys
    /// local-vs-remote: `None` = the local tmux server, `Some(host)` = a
    /// remote one. A key absent from the map hasn't been probed yet
    /// (rendered "claude …, codex …"). The layout/render just look a
    /// section up by key — they never branch on local vs remote. Each
    /// value lists the located agents (count + session/window/pane).
    /// See `crate::agent`.
    pub agents: HashMap<HostKey, Vec<crate::agent::DetectedAgent>>,

    /// Sidebar groups the user has collapsed (Expanded view only).
    /// `HostKey::local()` is the `@local` group; `HostKey::remote(host)`
    /// is a remote `@host` group. A collapsed group renders as just its
    /// divider — its session rows are hidden by the layout. Persisted to
    /// config (`collapsed_sections`) and restored at startup.
    pub collapsed_sections: HashSet<HostKey>,
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
    /// A fresh state with sensible defaults (mirroring `Config::default`).
    /// Callers apply the loaded config right after via [`apply_config`]
    /// (Self::apply_config); tests set the fields they care about directly.
    pub fn new(term_width: u16, term_height: u16) -> Self {
        Self {
            entries: Vec::new(),
            focused: 0,
            current_session: String::new(),
            session_order: Vec::new(),
            main_view: MainView::Terminal,
            focus_mode: FocusMode::Main,
            prefs: Prefs {
                theme_index: 0,
                layout_mode: LayoutMode::default(),
                show_borders: true,
                transparent_bg: false,
                sidebar_tab: SidebarTab::default(),
                sidebar_width: 28,
                sidebar_height: SIDEBAR_HEIGHT,
                view_mode: ViewMode::default(),
                frame_rate_limit: 5,
                exclude_patterns: Vec::new(),
                plugins: Vec::new(),
                update_check_mode: UpdateCheckMode::default(),
                summary_prompt: String::new(),
                summary_model: String::new(),
                summary_height: DEFAULT_SUMMARY_HEIGHT,
                summary_language: String::new(),
                agents_probe_interval_secs: DEFAULT_AGENTS_PROBE_INTERVAL,
            },
            settings: SettingsState::default(),
            agent_focused: 0,
            summary: SummaryCard::default(),
            dragging_separator: false,
            overlay: OverlayState::default(),
            term_width,
            term_height,
            last_scroll: Instant::now(),
            keybindings: Keybindings::default(),
            update_available: None,
            update_last_checked_secs: None,
            hit_regions: HitRegions::default(),
            reload_status: None,
            reload_status_at: None,
            config_remotes: Vec::new(),
            forward_health: HashMap::new(),
            agents: HashMap::new(),
            active_agent: None,
            collapsed_sections: HashSet::new(),
        }
    }

    /// Apply every config-derived field shared by startup (`App::new`) and
    /// hot-reload (`reload_config`). One list, so a new config field can't
    /// be applied at startup but silently missed on reload (or vice versa).
    ///
    /// Deliberately NOT covered here:
    /// - `config_remotes` — reload diffs old vs new forwards/hosts around
    ///   this call and commits the new list itself;
    /// - `collapsed_sections` — runtime state seeded from config once at
    ///   startup; a reload must not stomp the user's live collapse state.
    pub fn apply_config(
        &mut self,
        cfg: &crate::config::Config,
        theme_index: usize,
        keybindings: Keybindings,
    ) {
        // The single Config→prefs mapping site (its inverse is
        // `Prefs::to_config`). Clamps/normalizations live in `from_config`.
        self.prefs = Prefs::from_config(cfg, theme_index);
        self.keybindings = keybindings;
        // Theme indices may have shifted; keep the picker's cursor valid.
        self.settings.theme_picker_selected = theme_index;
    }

    /// Drop the reload banner once its per-variant TTL has elapsed.
    /// Called from the main loop so rendering stays side-effect-free.
    pub fn tick_reload_status(&mut self, now: Instant) -> bool {
        if let (Some(status), Some(shown_at)) = (&self.reload_status, self.reload_status_at) {
            if now.saturating_duration_since(shown_at) >= status.ttl() {
                self.reload_status = None;
                self.reload_status_at = None;
                return true;
            }
        }
        false
    }

    pub fn cycle_frame_rate_limit(&mut self, direction: i32) {
        self.prefs.frame_rate_limit = cycle_option(
            &FRAME_RATE_LIMIT_OPTIONS,
            normalize_frame_rate_limit(self.prefs.frame_rate_limit),
            direction,
        );
    }

    pub fn cycle_agents_probe_interval(&mut self, direction: i32) {
        self.prefs.agents_probe_interval_secs = cycle_option(
            &AGENTS_PROBE_INTERVAL_OPTIONS,
            normalize_agents_probe_interval(self.prefs.agents_probe_interval_secs),
            direction,
        );
    }

    /// Whether the Agents tab is the active sidebar view. The tab selector
    /// only exists in the Horizontal layout (the Vertical layout is a
    /// session tab-bar with no header), so the Agents view is gated to
    /// Horizontal — everything stays the Projects view in Vertical even if
    /// `sidebar_tab` happens to be `Agents`. Gates agent detection in the
    /// refresh worker and selects the agents layout / focus space.
    pub fn agents_tab_active(&self) -> bool {
        self.prefs.sidebar_tab == SidebarTab::Agents
            && self.prefs.layout_mode == LayoutMode::Horizontal
    }

    /// The active view's cursor: `focused` on Projects, `agent_focused`
    /// on Agents. Centralizes the per-tab focus split so navigation code
    /// doesn't branch on the tab everywhere.
    pub fn cursor(&self) -> usize {
        if self.agents_tab_active() {
            self.agent_focused
        } else {
            self.focused
        }
    }

    /// Set the active view's cursor (see [`cursor`](Self::cursor)).
    pub fn set_cursor(&mut self, n: usize) {
        if self.agents_tab_active() {
            self.agent_focused = n;
        } else {
            self.focused = n;
        }
    }

    pub fn effective_sidebar_height(&self) -> u16 {
        // Vertical layout is a single tab-switching row — there is no
        // second detail row to resize into, so the sidebar is pinned to
        // exactly the tab bar (plus top/bottom border when shown) and
        // the stored `sidebar_height` is ignored.
        if self.prefs.layout_mode == LayoutMode::Vertical {
            return if self.prefs.show_borders { 3 } else { 1 };
        }
        let (min_height, max_height) = self.sidebar_height_bounds();
        self.prefs.sidebar_height.clamp(min_height, max_height)
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
        let (min_height, max_height, available_height) = if self.prefs.show_borders {
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
        let bo = if self.prefs.show_borders { 2u16 } else { 0 };
        match self.prefs.layout_mode {
            LayoutMode::Horizontal => {
                let cols = self
                    .term_width
                    .saturating_sub(self.prefs.sidebar_width + 1 + bo)
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

    /// Height of the sidebar footer in rows, for mouse hit-testing. The
    /// formula itself lives in `crate::geometry::sidebar_footer_height`,
    /// shared with the renderer (`ui::sidebar::draw_sidebar`) so the two
    /// can't drift — when they did, the bottom visible session row was
    /// click-dead.
    pub fn sidebar_footer_height(&self) -> u16 {
        let b = if self.prefs.show_borders { 2u16 } else { 0 };
        let content_width = match self.prefs.layout_mode {
            LayoutMode::Horizontal => self.prefs.sidebar_width.saturating_sub(b),
            LayoutMode::Vertical => self.term_width.saturating_sub(b),
        };
        crate::geometry::sidebar_footer_height(
            crate::geometry::banner_visible(self.update_available.is_some(), content_width),
            self.prefs.plugins.len(),
        )
    }

    /// Resolve a screen row inside the sidebar's scrollable session area
    /// into `(layout, viewport_y, scroll, visible_height)`. `None` when
    /// the row falls in the header banner, the footer, or outside the
    /// sidebar. Shared by the row and divider hit-testers so they agree
    /// on geometry and the scroll offset the renderer applied.
    fn session_row_hit(&self, row: u16) -> Option<(BuiltLayout, u16, u16, u16)> {
        let b = if self.prefs.show_borders { 1u16 } else { 0 };
        let sidebar_h = match self.prefs.layout_mode {
            LayoutMode::Horizontal => self.term_height,
            LayoutMode::Vertical => self.effective_sidebar_height(),
        };
        let header_height = crate::geometry::SIDEBAR_HEADER_HEIGHT;
        let footer_height = self.sidebar_footer_height();
        let sessions_top = b + header_height;
        let sessions_bottom = sidebar_h.saturating_sub(b + footer_height);
        if row < sessions_top || row >= sessions_bottom {
            return None;
        }
        // The list viewport sits below the Summary card on the Agents tab,
        // which is rendered above it (not part of the sectioned list).
        let summary_h = if self.agents_tab_active() {
            self.summary_card_height()
        } else {
            0
        };
        let list_top = sessions_top + summary_h;
        if row < list_top || row >= sessions_bottom {
            return None;
        }
        let visible_height = sessions_bottom - list_top;
        let built = self.current_layout(self.prefs.view_mode);
        let scroll = built
            .layout
            .scroll_offset(self.focus_target().map(|f| f.0), visible_height);
        let viewport_y = row - list_top;
        Some((built, viewport_y, scroll, visible_height))
    }

    /// Map a screen row to a sidebar focus target. Walks the unified
    /// layout (local cards + remote groups + headers) so variable-
    /// height rows hit-test correctly.
    pub fn focus_at_row(&self, row: u16) -> Option<FocusTarget> {
        let (built, viewport_y, scroll, _) = self.session_row_hit(row)?;
        built.layout.row_at_y(viewport_y, scroll).map(FocusTarget)
    }

    /// Whether `row` falls on a group divider header (`@local` / `@host`).
    /// Used to swallow right-clicks on dividers — their actions live on
    /// the divider's own `[…]` button, not a context menu.
    pub fn is_divider_at_row(&self, row: u16) -> bool {
        let Some((built, viewport_y, scroll, visible_height)) = self.session_row_hit(row) else {
            return false;
        };
        // Bind before the block ends so the `VisibleIter` (which borrows the
        // layout) drops before `built` does — its Drop impl otherwise
        // outlives the borrow.
        let hit = built.layout.visible_items(scroll, visible_height).any(|v| {
            v.item.kind == ItemKind::Header
                && viewport_y >= v.viewport_y
                && viewport_y < v.viewport_y + v.visible_height
        });
        hit
    }

    /// Map a screen row on a group divider to that group's section key
    /// (`None` = `@local`, `Some(host)` = a remote `@host`). Returns
    /// `None` when the row isn't on a real group divider (e.g. a no-agents
    /// placeholder header, which isn't a collapse target). Used by the
    /// mouse layer to toggle a group when its divider is clicked.
    pub fn divider_section_key_at(&self, row: u16) -> Option<Option<String>> {
        let (built, viewport_y, scroll, _) = self.session_row_hit(row)?;
        // header_at_y returns the 0-based header section index, which is a
        // direct index into `sections`. Only real dividers toggle collapse.
        let section_idx = built.layout.header_at_y(viewport_y, scroll)?;
        let meta = built.sections.get(section_idx)?;
        if meta.divider {
            Some(meta.host.clone())
        } else {
            None
        }
    }

    /// Map a screen column to a tab index in vertical/tabs mode.
    pub fn session_at_col(&self, col: u16, row: u16) -> Option<usize> {
        let b = if self.prefs.show_borders { 1u16 } else { 0 };
        if row != b {
            return None;
        }
        // Build labels in the same flat order the tab renderer walks —
        // local rows first, then remotes as `host:session` — so a hit
        // maps straight to a `FocusTarget` flat index.
        let labels: Vec<String> = self
            .entries
            .iter()
            .map(|e| tab_label(e.host.as_deref(), &e.name))
            .collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let ranges = tab_col_ranges(&label_refs);
        let local_col = col.saturating_sub(b);
        for (i, &(start, end)) in ranges.iter().enumerate() {
            if local_col >= start && local_col < end {
                return Some(i);
            }
        }
        None
    }

    /// The entry at flat focus index `idx`, or `None` if out of range. The
    /// `FocusTarget` numbering is a direct index into `entries` (local rows
    /// first, then remotes), so this is the single decode the reducers /
    /// action layer use for per-row dispatch — they read `entry.host` /
    /// `entry.kind` instead of taking apart the index.
    pub fn entry_at(&self, target: FocusTarget) -> Option<&SessionEntry> {
        self.entries.get(target.0)
    }

    /// Local entries (`host == None`), in order. The local-only invariants
    /// (`session_order`, `current_session`, the last-local-session kill
    /// guard) operate over these.
    pub fn local_entries(&self) -> impl Iterator<Item = &SessionEntry> {
        self.entries.iter().filter(|e| e.is_local())
    }

    /// Number of local entries (`host == None`). Local rows occupy the
    /// front of `entries`, so this is also the flat index where the first
    /// remote row begins.
    pub fn local_count(&self) -> usize {
        self.local_entries().count()
    }

    /// Total number of focusable rows in the active sidebar tab. Projects:
    /// every session entry (local rows then remote rows). Agents: the
    /// flattened agent list.
    pub fn focusable_count(&self) -> usize {
        if self.agents_tab_active() {
            self.agent_count()
        } else {
            self.entries.len()
        }
    }

    /// Flat focusable index of the row for `session` on `host` (`None` =
    /// local), or `None` if no such row is currently listed. Lets the
    /// sidebar move its single highlight onto a session switched to
    /// out-of-band — e.g. an agent-footer click — so the highlight tracks
    /// the viewed session the same way keyboard navigation does (j/k moves
    /// the cursor *and* switches). Mirrors the `FocusTarget` numbering:
    /// local rows first, then remotes.
    pub fn focusable_index_for(&self, host: Option<&str>, session: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.host.as_deref() == host && e.name == session)
    }

    /// Optimistically mark a host's rows as reconnecting so the sidebar
    /// shows "(connecting...)" the instant the user hits the divider's
    /// reconnect button, before the refresh round returns.
    pub fn mark_host_reconnecting(&mut self, host: &str) {
        for e in &mut self.entries {
            if e.host.as_deref() == Some(host) {
                e.kind = SessionKind::Connecting;
            }
        }
    }

    /// Agents detected in a sidebar section, addressed uniformly by host
    /// (`None` = local). `None` result = not probed yet. The layout uses
    /// this without caring whether the section is local or remote.
    pub fn section_agents(&self, host: Option<&str>) -> Option<&[crate::agent::DetectedAgent]> {
        self.agents
            .get(HostQuery::from_host(host))
            .map(Vec::as_slice)
    }

    /// Fold a remote refresh round's agent detection into `agents`.
    /// `covered_hosts` is every host the round queried; `fresh` is the
    /// per-host result for hosts whose probe succeeded. A host covered
    /// this round but missing from `fresh` had a failed probe, so its
    /// stale list is dropped (otherwise dead pane ids keep rendering as
    /// clickable footer lines). The local `None` key is untouched, and
    /// hosts no longer configured are pruned.
    pub fn apply_remote_agents(
        &mut self,
        covered_hosts: std::collections::HashSet<String>,
        fresh: HashMap<String, Vec<crate::agent::DetectedAgent>>,
    ) {
        for host in covered_hosts {
            if !fresh.contains_key(&host) {
                self.agents.remove(HostQuery::from_host(Some(&host)));
            }
        }
        for (host, list) in fresh {
            self.agents.insert(HostKey::remote(&host), list);
        }
        let configured: std::collections::HashSet<&str> = self
            .config_remotes
            .iter()
            .map(|r| r.host.as_str())
            .collect();
        self.agents
            .retain(|k, _| k.host().is_none_or(|h| configured.contains(h)));
    }

    /// Build the unified sidebar layout: a flat list of header /
    /// session items in render order. Renderers and the mouse
    /// hit-tester share this so they can't disagree about which row
    /// lives where.
    /// The active theme, resolved from the saved index. The layout builders
    /// bake divider accents and row marker colors into their `BasicItem`s, so
    /// they read the theme here rather than taking it as a parameter (the
    /// hit-test call sites build the same layout and don't carry a theme).
    pub fn active_theme(&self) -> &'static crate::theme::Theme {
        &crate::theme::THEMES[self.prefs.theme_index]
    }

    /// `BasicItem` for one session row. Expanded carries the name plus a dim
    /// `dir` line (height 2); Compact is a single `origin:name` line.
    fn session_item(&self, e: &SessionEntry, view_mode: ViewMode) -> BasicItem {
        let loading = matches!(e.kind, SessionKind::Connecting);
        let name = if loading {
            "(connecting…)".to_string()
        } else {
            match e.kind {
                SessionKind::Unreachable => UNREACHABLE_LABEL.to_string(),
                SessionKind::NoSessions => NO_SESSIONS_LABEL.to_string(),
                _ => e.name.clone(),
            }
        };
        match view_mode {
            ViewMode::Compact => {
                let prefix = e.host.as_deref().unwrap_or("local");
                BasicItem::new(format!("{prefix}:{name}"))
            }
            ViewMode::Expanded => {
                let dir = if loading || e.dir.is_empty() {
                    String::new()
                } else {
                    shorten_dir(&e.dir)
                };
                // One secondary line (even when blank) so every Expanded row
                // is a uniform 2 rows tall.
                BasicItem::new(name).line(dir)
            }
        }
    }

    /// `@local` divider: accent fill + a single `[…]` menu button.
    fn local_divider(&self) -> BasicItem {
        BasicItem::new("@local")
            .separator("─")
            .color(self.active_theme().accent)
            .button("…")
    }

    /// `@host` divider: per-host accent fill + `[⟳]` reconnect and `[…]`
    /// menu buttons.
    fn remote_divider(&self, host: &str, host_idx: usize) -> BasicItem {
        BasicItem::new(format!("@{host}"))
            .separator("─")
            .color(host_accent(self.active_theme(), host_idx))
            .button("⟳")
            .button("…")
    }

    /// Build the unified Projects-tab layout: a flat `BasicItem` list of
    /// `@local` / `@host` dividers (Expanded only) interleaved with session
    /// rows, plus the per-divider [`SectionMeta`] the hit-tester resolves
    /// clicks against. Renderer and hit-tester share this so they can't
    /// disagree about which row lives where.
    pub fn sidebar_layout(&self, view_mode: ViewMode) -> BuiltLayout {
        let mut layout = SidebarLayout::new();
        let mut sections: Vec<SectionMeta> = Vec::new();
        // Group dividers (`@local`, `@host`) are an Expanded-view adornment;
        // Compact rows already carry an origin prefix.
        let show_headers = matches!(view_mode, ViewMode::Expanded);

        // Collapse is an Expanded-view feature. Track each group header's
        // (section_idx, key) so we can flip its collapsed flag after the list
        // is built; section_idx counts every pushed header in push order,
        // matching the crate's section numbering (and `sections`).
        layout.set_collapsible(show_headers);
        let mut group_headers: Vec<(usize, HostKey)> = Vec::new();

        let local_count = self.local_count();
        if show_headers {
            group_headers.push((sections.len(), HostKey::local()));
            layout.push_header_auto(self.local_divider());
            sections.push(SectionMeta {
                host: None,
                buttons: vec![DividerButton::LocalMore],
                divider: true,
            });
        }
        for pos in 0..local_count {
            layout.push_row_auto(self.session_item(&self.entries[pos], view_mode));
        }

        // Remote groups: detect host transitions in render order (which
        // matches focus order — `entries` is grouped by host). Flat row index
        // is the row's position in `entries`. Each group gets an `@host`
        // divider above.
        let mut host_idx: usize = 0;
        let mut prev_host: Option<&str> = None;
        for e in self.entries.iter() {
            let Some(host) = e.host.as_deref() else {
                continue; // local rows already pushed above
            };
            let new_host = Some(host) != prev_host;
            if new_host {
                if prev_host.is_some() {
                    host_idx += 1;
                }
                if show_headers {
                    group_headers.push((sections.len(), HostKey::remote(host)));
                    layout.push_header_auto(self.remote_divider(host, host_idx));
                    sections.push(SectionMeta {
                        host: Some(host.to_string()),
                        buttons: vec![DividerButton::Reconnect, DividerButton::More],
                        divider: true,
                    });
                }
                prev_host = Some(host);
            }
            layout.push_row_auto(self.session_item(e, view_mode));
        }

        // Flip each group header's collapsed flag so the widget hides its
        // rows and the geometry/scroll/hit-test all honor the collapse.
        for (section_idx, key) in group_headers {
            layout.set_collapsed(section_idx, self.collapsed_sections.contains(&key));
        }

        BuiltLayout { layout, sections }
    }

    /// Distinct remote hosts in the order their rows first appear in
    /// `remote_sessions` (the refresh worker emits hosts in config order,
    /// one contiguous block each). Shared by `agent_rows` and
    /// `agents_layout` so both walk sections identically.
    fn remote_hosts_in_order(&self) -> Vec<String> {
        self.remote_hosts_in_order_ref()
            .map(str::to_string)
            .collect()
    }

    /// Borrowing twin of [`remote_hosts_in_order`](Self::remote_hosts_in_order):
    /// yields each distinct remote host as `&str` in first-appearance order,
    /// without the per-host `String` clone. Hot callers (`agent_rows`,
    /// `agent_count`) that only read the host name take this path (D17).
    fn remote_hosts_in_order_ref(&self) -> impl Iterator<Item = &str> {
        let mut seen: HashSet<&str> = HashSet::new();
        self.entries
            .iter()
            .filter_map(|e| e.host.as_deref())
            .filter(move |host| seen.insert(host))
    }

    /// The flat list of detected agents for the Agents tab, in display
    /// order: local agents first, then each remote host's agents in
    /// section order. `Agent { row_idx }` items and the renderer index
    /// into this, so its order is the Agents-tab `FocusTarget` numbering.
    pub fn agent_rows(&self) -> Vec<AgentRow<'_>> {
        let mut rows = Vec::new();
        if let Some(list) = self.section_agents(None) {
            for agent in list {
                rows.push(AgentRow { host: None, agent });
            }
        }
        // Borrow each host key straight out of `agents` so the rows can
        // hold `&str` without cloning the host name per row.
        for host in self.remote_hosts_in_order_ref() {
            if let Some(list) = self.section_agents(Some(host)) {
                for agent in list {
                    rows.push(AgentRow {
                        host: Some(host),
                        agent,
                    });
                }
            }
        }
        rows
    }

    /// Number of focusable agent rows, without building (or cloning into)
    /// the `agent_rows()` vec. `focusable_count` runs per keystroke and
    /// only ever wanted the length, so it takes this path (D17).
    pub fn agent_count(&self) -> usize {
        let local = self.section_agents(None).map_or(0, <[_]>::len);
        let remote: usize = self
            .remote_hosts_in_order_ref()
            .filter_map(|host| self.section_agents(Some(host)))
            .map(<[_]>::len)
            .sum();
        local + remote
    }

    /// Flat focusable index of the agent row matching `target`, or `None`
    /// if it isn't currently listed. Lets the Agents-tab cursor track the
    /// pane switched to via a click, the way `focusable_index_for` does
    /// for the Projects tab.
    pub fn agent_row_index_for(&self, target: &AgentTarget) -> Option<usize> {
        self.agent_rows().iter().position(|row| {
            row.host == target.host.as_deref() && row.agent.pane_id == target.pane_id
        })
    }

    /// Row height the Summary card reserves: title + blank + a
    /// `summary_height` body area + a drag-handle row. A fixed-size window
    /// for every state, so overflowing Ready text scrolls inside it rather
    /// than growing the card; the user resizes it by dragging the handle.
    pub fn summary_card_height(&self) -> u16 {
        3 + self.prefs.summary_height
    }

    /// Set the card body height (rows), clamped to the drag-resize bounds.
    /// Returns whether it changed.
    pub fn set_summary_height(&mut self, rows: u16) -> bool {
        let clamped = rows.clamp(SUMMARY_MIN_HEIGHT, SUMMARY_MAX_HEIGHT);
        if clamped != self.prefs.summary_height {
            self.prefs.summary_height = clamped;
            true
        } else {
            false
        }
    }

    /// Whether `pos` falls anywhere on the Summary card. Used by the wheel
    /// path to route scroll events to the card text. Checked directly,
    /// independent of `HitRegions::hit` priority, because the card rect
    /// spans the whole Agents-tab viewport and the agent rows/dividers
    /// drawn over it outrank the card for *clicks* but not for the wheel.
    pub fn summary_card_at(&self, col: u16, row: u16) -> bool {
        let pos = Position::new(col, row);
        self.hit_regions
            .summary
            .card
            .is_some_and(|r| r.contains(pos))
    }

    /// Whether `(col, row)` is on the card's bottom drag-handle row.
    pub fn summary_resize_at(&self, col: u16, row: u16) -> bool {
        self.hit_regions.summary.card.is_some_and(|r| {
            let handle_y = r.y + r.height.saturating_sub(1);
            row == handle_y && col >= r.x && col < r.x + r.width
        })
    }

    /// New body height implied by dragging the handle to `row` — the rows
    /// between the card top and the pointer, minus the title/blank/handle
    /// chrome. Clamped by `set_summary_height`.
    pub fn summary_height_for_drag(&self, row: u16) -> u16 {
        let top = self.hit_regions.summary.card.map_or(0, |r| r.y);
        // total = row - top + 1; body rows = total - 3 (title, blank, handle).
        row.saturating_sub(top).saturating_sub(2)
    }

    /// Apply a wheel/keyboard scroll delta to the Summary text, clamped to
    /// the captured max offset.
    pub fn scroll_summary(&mut self, delta: i32) {
        let max = self.hit_regions.summary.max_scroll as i32;
        self.summary.scroll = (self.summary.scroll as i32 + delta).clamp(0, max) as usize;
    }

    /// Move the summary card off `Generating` back to the state it held
    /// before generation started (Idle / a prior Ready / Error), used when
    /// the user cancels mid-flight. The App side drops the worker (killing
    /// the `claude` child); this is the pure state half. No-op unless
    /// currently generating.
    pub fn cancel_summary(&mut self) {
        if self.summary.state != SummaryState::Generating {
            return;
        }
        self.summary.state = self.summary.before_generating.take().unwrap_or_default();
        self.summary.scroll = 0;
    }

    /// Apply a scroll delta to the summary popup, clamped to its max.
    pub fn scroll_summary_popup(&mut self, delta: i32) {
        let max = self.summary.popup_max_scroll as i32;
        self.summary.popup_scroll =
            (self.summary.popup_scroll as i32 + delta).clamp(0, max) as usize;
    }

    /// Build the Agents-tab layout: an `@local` / `@host` divider per
    /// section (in `agent_rows` order) with that section's agents as
    /// focusable rows beneath it, or a non-focusable placeholder when a
    /// section has no agents. `row_idx` on each `Agent` item matches the
    /// `agent_rows()` position so focus/scroll/hit-test stay in sync.
    pub fn agents_layout(&self) -> BuiltLayout {
        let mut layout = SidebarLayout::new();
        let mut sections: Vec<SectionMeta> = Vec::new();
        // No collapse on the Agents tab — sections are informational and
        // always expanded, so the focus index maps straight to a row. The
        // Summary card is no longer in the list: it renders as a separate
        // widget pinned above, so the list is pure `BasicItem`.
        layout.set_collapsible(false);

        // Push a divider header for `host`, then either its agent rows or a
        // single inert placeholder header (no agents / still detecting).
        let push_section = |layout: &mut SidebarLayout,
                            sections: &mut Vec<SectionMeta>,
                            header: BasicItem,
                            meta: SectionMeta,
                            host: Option<&str>,
                            first: bool| {
            if first {
                layout.push_header_auto(header);
            } else {
                // A 1-row top margin sets each remote section off from
                // what's above; local stays flush at the top.
                layout.push_header_margin(header, 1);
            }
            sections.push(meta);
            match self.section_agents(host) {
                Some(list) if !list.is_empty() => {
                    for agent in list {
                        // A status glyph prefix, plus a filled marker on
                        // the agent deck is currently focused on (the
                        // "you are here" pane), so switching shows where
                        // you landed even without per-row coloring.
                        let here = self.active_agent.as_ref().is_some_and(|t| {
                            t.host.as_deref() == host && t.pane_id == agent.pane_id
                        });
                        let dot = match agent.status {
                            crate::agent::AgentStatus::Working => "●",
                            crate::agent::AgentStatus::Idle => "○",
                            crate::agent::AgentStatus::Waiting => "◐",
                            crate::agent::AgentStatus::Unknown => "·",
                        };
                        let lead = if here { "▶" } else { " " };
                        // No accent color: like session rows, focus shows
                        // only via the highlight background bar. The status
                        // glyph + ▶ marker carry the per-row state.
                        layout.push_row_auto(BasicItem::new(format!(
                            "{lead} {dot} {}",
                            agent.location()
                        )));
                    }
                }
                other => {
                    let label = if other.is_some() {
                        "  no agents"
                    } else {
                        "  detecting…"
                    };
                    layout.push_header_auto(BasicItem::new(label));
                    sections.push(SectionMeta {
                        host: host.map(str::to_string),
                        buttons: Vec::new(),
                        divider: false,
                    });
                }
            }
        };

        push_section(
            &mut layout,
            &mut sections,
            self.local_divider(),
            SectionMeta {
                host: None,
                buttons: vec![DividerButton::LocalMore],
                divider: true,
            },
            None,
            true,
        );

        for (host_idx, host) in self.remote_hosts_in_order().into_iter().enumerate() {
            push_section(
                &mut layout,
                &mut sections,
                self.remote_divider(&host, host_idx),
                SectionMeta {
                    host: Some(host.clone()),
                    buttons: vec![DividerButton::Reconnect, DividerButton::More],
                    divider: true,
                },
                Some(&host),
                false,
            );
        }

        BuiltLayout { layout, sections }
    }

    /// The layout for the active sidebar tab. Projects → the session
    /// list; Agents → the agent list. Callers (renderer, hit-testers,
    /// scroll) use this so they all see the same rows for the active tab.
    pub fn current_layout(&self, view_mode: ViewMode) -> BuiltLayout {
        if self.agents_tab_active() {
            self.agents_layout()
        } else {
            self.sidebar_layout(view_mode)
        }
    }

    /// Drop health entries whose forward no longer exists in config (after a
    /// reload that removed forwards), so the map doesn't accrete dead keys.
    pub fn prune_forward_health(&mut self) {
        let valid: std::collections::HashSet<ForwardKey> = self
            .config_remotes
            .iter()
            .flat_map(|r| r.forwards.iter().map(|f| ForwardKey::from_spec(&r.host, f)))
            .collect();
        self.forward_health.retain(|k, _| valid.contains(k));
    }

    /// The connection status shown on a host's divider, derived from its
    /// remote rows. `None` until the host has any row (pre-refresh).
    pub fn host_conn_status(&self, host: &str) -> Option<HostStatus> {
        self.entries
            .iter()
            .find(|e| e.host.as_deref() == Some(host))
            .map(host_status_of)
    }

    /// Refresh `-R` forward health from each host's connection status. A
    /// remote-forward listener lives on the far side and can't be probed
    /// locally, so it simply mirrors reachability: connected → Up, unreachable
    /// → Down, still-connecting → Probing. `-L`/`-D` entries are owned by the
    /// worker probe and left untouched. Called whenever remote status changes
    /// so `-R` and the divider always agree.
    pub fn sync_remote_forward_health(&mut self) {
        let updates: Vec<(ForwardKey, ForwardHealth)> = self
            .config_remotes
            .iter()
            .flat_map(|r| {
                r.forwards
                    .iter()
                    .filter(|f| matches!(f.mode, crate::config::ForwardMode::Remote))
                    .map(|f| {
                        let health = match self.host_conn_status(&r.host) {
                            Some(HostStatus::Connected) => ForwardHealth::Up,
                            Some(HostStatus::Unreachable) => ForwardHealth::Down,
                            Some(HostStatus::Connecting) | None => ForwardHealth::Probing,
                        };
                        (ForwardKey::from_spec(&r.host, f), health)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        for (key, health) in updates {
            self.forward_health.insert(key, health);
        }
    }

    /// Focused remote placeholder row, if any. These rows occupy normal
    /// focus slots so users can land on a host with no attachable session,
    /// but the main pane must render an explicit status instead of a stale
    /// terminal screen.
    pub fn focused_remote_placeholder(&self) -> Option<&SessionEntry> {
        if self.agents_tab_active() {
            return None;
        }
        let entry = self.entry_at(self.focus_target()?)?;
        (!entry.is_local() && !entry.is_attachable()).then_some(entry)
    }

    /// Section key of the group the flat focus index `idx` lives in:
    /// `None` for a local row, `Some(host)` for a remote one. Used by the
    /// section-toggle keybinding and focus-skip logic. For an out-of-range
    /// index this falls back to `None`.
    pub fn section_key_of_focus(&self, idx: usize) -> Option<String> {
        self.entries.get(idx).and_then(|e| e.host.clone())
    }

    /// Whether the row at flat focus index `idx` sits in a collapsed group
    /// (so keyboard focus should skip over it).
    pub fn is_focus_collapsed(&self, idx: usize) -> bool {
        // The Agents tab is never collapsible — its rows map straight to
        // `agent_rows`, and `section_key_of_focus` assumes session indexing.
        if self.agents_tab_active() {
            return false;
        }
        idx < self.focusable_count()
            && self.collapsed_sections.contains(HostQuery::from_host(
                self.section_key_of_focus(idx).as_deref(),
            ))
    }

    /// Decode the active tab's cursor into a focus target. Returns `None`
    /// if nothing is focusable (empty list). The index is into the active
    /// tab's row space — sessions on Projects, agents on Agents.
    pub fn focus_target(&self) -> Option<FocusTarget> {
        if self.cursor() < self.focusable_count() {
            Some(FocusTarget(self.cursor()))
        } else {
            None
        }
    }

    /// The agent under the Agents-tab cursor, or `None` when off-tab or
    /// no agent is focused. Resolves the cursor through `agent_rows`.
    pub fn focused_agent(&self) -> Option<AgentTarget> {
        if !self.agents_tab_active() {
            return None;
        }
        let row = self.agent_rows().into_iter().nth(self.agent_focused)?;
        Some(AgentTarget {
            host: row.host.map(str::to_string),
            session: row.agent.session.clone(),
            pane_id: row.agent.pane_id.clone(),
        })
    }

    /// Name to show in the kill-confirmation overlay: the focused row's
    /// session name, or `None` when no kill is pending or focus has no
    /// valid target. The renderer gates the overlay on this being `Some`.
    ///
    /// Resolves through `entry_at` so a focused *remote* row reports its
    /// name too — the unified store treats local and remote rows the same
    /// here (issue #41).
    /// The highest-priority full-input modal currently open, or `None` when
    /// the sidebar/PTY is free to take input directly.
    ///
    /// The order below is the single source of truth for input routing and
    /// **must mirror `keyboard::key_to_action`'s early-return chain exactly**
    /// — both mappers consult this first, so the priority here decides which
    /// overlay swallows a key or click when more than one flag is set.
    ///
    /// The settings sub-modals (KeybindingsView / ExcludeEditor /
    /// SummaryLang) only count while the settings page itself owns focus
    /// (`MainView::Settings` + `FocusMode::Main`); elsewhere their backing
    /// fields are stale and must not gate input. Everything above them is a
    /// standalone overlay that can be opened straight from the sidebar.
    pub fn active_modal(&self) -> Option<Modal> {
        if self.overlay.summary_popup {
            return Some(Modal::SummaryPopup);
        }
        if self.overlay.new_session.is_some() {
            return Some(Modal::NewSession);
        }
        if self.overlay.add_remote.is_some() {
            return Some(Modal::AddRemote);
        }
        if self.overlay.renaming.is_some() {
            return Some(Modal::Rename);
        }
        if self.overlay.context_menu.is_some() {
            return Some(Modal::ContextMenu);
        }
        if self.overlay.port_forward.is_some() {
            return Some(Modal::PortForward);
        }
        if self.settings.theme_picker_open {
            return Some(Modal::ThemePicker);
        }
        if self.main_view == MainView::Settings && self.focus_mode == FocusMode::Main {
            if self.settings.keybindings_view_open {
                return Some(Modal::KeybindingsView);
            }
            if self.overlay.exclude_editor.is_some() {
                return Some(Modal::ExcludeEditor);
            }
            if self.overlay.summary_lang_input.is_some() {
                return Some(Modal::SummaryLang);
            }
        }
        if self.overlay.show_help {
            return Some(Modal::Help);
        }
        if self.overlay.confirm_kill {
            return Some(Modal::ConfirmKill);
        }
        None
    }

    pub fn confirm_kill_name(&self) -> Option<String> {
        if !self.overlay.confirm_kill {
            return None;
        }
        Some(self.entry_at(self.focus_target()?)?.name.clone())
    }

    /// Why the focused kill `target` can't be killed, or `None` if it can.
    /// Single source of truth shared by the `x`-key path (`KillSession` /
    /// `ConfirmKill`) and the context menu's "Kill" greying so the two
    /// can't drift (the keyboard path used to skip these checks):
    ///  - a synthetic placeholder remote row (loading / unreachable /
    ///    "(no sessions)") has no real session to kill — a kill would send
    ///    `ssh tmux kill-session` with a placeholder/empty name;
    ///  - a host's last live session would tear that host's tmux server
    ///    down;
    ///  - the last local session would leave deck attached to nothing.
    pub fn kill_blocked_reason(&self, entry: &SessionEntry) -> Option<&'static str> {
        match &entry.host {
            Some(_) if !entry.is_attachable() => Some("no session to kill"),
            Some(host)
                if attachable_on_host(&self.entries, Some(host))
                    .nth(1)
                    .is_none() =>
            {
                Some("last session on host")
            }
            None if self.local_count() <= 1 => Some("last local session"),
            _ => None,
        }
    }

    /// Whether the focused kill `entry` may be killed. See
    /// [`AppState::kill_blocked_reason`].
    pub fn can_kill(&self, entry: &SessionEntry) -> bool {
        self.kill_blocked_reason(entry).is_none()
    }

    /// Map a screen position to a context menu item index.
    pub fn menu_item_at(&self, col: u16, row: u16) -> Option<usize> {
        let menu = self.overlay.context_menu.as_ref()?;
        let items = menu.items();
        // Same rect the renderer draws into (`ui::menu::draw_context_menu`).
        let r = context_menu_rect(items, menu.x, menu.y, self.term_width, self.term_height);
        // Interior only: clicks on the border select nothing.
        if col > r.x && col < r.x + r.width - 1 && row > r.y && row < r.y + r.height - 1 {
            let idx = (row - r.y - 1) as usize;
            if idx < items.len() {
                return Some(idx);
            }
        }
        None
    }

    // --- Focus clamping and ordering ---

    /// Keep the Projects-tab cursor (`focused`) inside the current row range
    /// (local sessions followed by remote rows) after that list changes —
    /// e.g. focus was parked on a placeholder/session row that just
    /// disappeared. Clamps against the Projects row space specifically (not
    /// the tab-aware `focusable_count`, which would use the agent count when
    /// the Agents tab is active and corrupt the Projects cursor).
    pub fn clamp_projects_focus(&mut self) {
        let total = self.entries.len();
        if total > 0 && self.focused >= total {
            self.focused = total - 1;
        }
    }

    /// Keep the Agents-tab cursor inside the current agent list after the
    /// detected agents change (agents come and go between refresh rounds).
    pub fn clamp_agent_focus(&mut self) {
        let total = self.agent_count();
        if total == 0 {
            self.agent_focused = 0;
        } else if self.agent_focused >= total {
            self.agent_focused = total - 1;
        }
    }

    /// Identity (host, `%N` pane id) of the agent under the Agents-tab
    /// cursor. Captured *before* a refresh rebuilds the agent list so the
    /// cursor can be re-anchored to the same agent afterwards — see
    /// [`reanchor_agent_focus`](Self::reanchor_agent_focus).
    pub fn focused_agent_key(&self) -> Option<(Option<String>, String)> {
        self.agent_rows()
            .into_iter()
            .nth(self.agent_focused)
            .map(|row| (row.host.map(str::to_string), row.agent.pane_id.clone()))
    }

    /// Re-point the Agents-tab cursor at the agent identified by `key` (its
    /// position before the list was rebuilt), so the highlighted row keeps
    /// tracking the same agent — and thus the pane shown on the right
    /// (`active_agent`) — instead of whatever now sits at the old index.
    /// `agent_focused` is a positional index, but the detected-agent list
    /// reorders and gains/loses entries between refresh rounds, so a bare
    /// `clamp_agent_focus` lets the cursor slide onto a different agent than
    /// the one the pane is showing. Falls back to clamping when the agent is
    /// gone (finished, went idle, or its host dropped). Use this instead of
    /// `clamp_agent_focus` after the agent list changes.
    pub fn reanchor_agent_focus(&mut self, key: Option<(Option<String>, String)>) {
        let rows = self.agent_rows();
        if let Some((host, pane_id)) = key {
            if let Some(idx) = rows
                .iter()
                .position(|row| row.host == host.as_deref() && row.agent.pane_id == pane_id)
            {
                self.agent_focused = idx;
                return;
            }
        }
        let total = rows.len();
        self.agent_focused = if total == 0 {
            0
        } else {
            self.agent_focused.min(total - 1)
        };
    }

    pub fn sync_order(&mut self) {
        let names: Vec<String> = self.local_entries().map(|e| e.name.clone()).collect();
        self.session_order.retain(|n| names.contains(n));
        for name in &names {
            if !self.session_order.contains(name) {
                self.session_order.push(name.clone());
            }
        }
    }

    /// Reorder the local entries (the `host == None` prefix of `entries`) to
    /// match `session_order`. Remote entries follow the local block in
    /// `entries`, so sorting only the local prefix keeps the unified store's
    /// "local first, then remotes (in config order)" invariant intact.
    pub fn apply_order(&mut self) {
        let order = &self.session_order;
        let rank = |e: &SessionEntry| -> usize {
            order
                .iter()
                .position(|n| n == &e.name)
                .unwrap_or(usize::MAX)
        };
        // Stable sort with remote rows pinned after locals by giving them a
        // monotonically-increasing rank above any local one; their relative
        // order (config order) is preserved by the stable sort.
        let local_count = self.local_count();
        self.entries.sort_by_key(|e| {
            if e.is_local() {
                (0usize, rank(e))
            } else {
                (1usize, local_count)
            }
        });
    }

    /// Clamp and set sidebar width. Returns true if it changed.
    pub fn resize_sidebar(&mut self, new_width: u16) -> bool {
        let (min_width, max_width) = self.sidebar_width_bounds();
        let clamped = new_width.clamp(min_width, max_width);
        if clamped == self.prefs.sidebar_width {
            return false;
        }
        self.prefs.sidebar_width = clamped;
        true
    }

    /// Clamp and set sidebar height. Returns true if it changed.
    pub fn resize_sidebar_height(&mut self, new_height: u16) -> bool {
        let (min_height, max_height) = self.sidebar_height_bounds();
        let clamped = new_height.clamp(min_height, max_height);
        if clamped == self.prefs.sidebar_height {
            return false;
        }
        self.prefs.sidebar_height = clamped;
        true
    }
}

#[cfg(test)]
#[path = "../../tests/unit/model/state.rs"]
mod tests;

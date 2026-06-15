use std::collections::{HashMap, HashSet};
use std::time::Instant;

use ratatui::layout::Position;
use ratatui_sectioned_list::widget::BasicItem;
use ratatui_sectioned_list::ItemKind;
use serde::{Deserialize, Serialize};

use crate::config::PluginConfig;
use crate::geometry::{context_menu_rect, host_accent, shorten_dir, tab_col_ranges, tab_label};
use crate::lane::LaneId;
use crate::system::tmux::lane;
use crate::keybindings::Keybindings;
use crate::update::{UpdateCheckMode, UpdateStatus};

// Re-export the model types so the pervasive `crate::state::X` references
// across the UI / app / test layers keep resolving. Each type's real home
// is the named module.
pub use crate::effects::{
    CreateSessionRequest, Effect, KillRequest, RemoteSwitchRequest, RenameRequest, SideEffect,
};
pub use crate::forwards::{
    ForwardHealth, ForwardKey, PfAddForm, PfField,
    PortForwardOverlay,
};
pub use crate::geometry::{
    AgentEntry, AgentEntryKind, AgentHit, AgentTarget, BuiltLayout, DividerHit,
    HitKind, HitRegions, KillConfirmHits, SectionLayoutOpts, SectionMeta, SidebarLayout,
    SummaryHits, TabRects,
};
pub use crate::menu::{session_menu_disabled, ContextMenu, MenuItem, MenuKind};
pub use crate::overlay::{ExcludeEditorState, Modal, OverlayState, RenameState, WarningState};
// The Summary card lives in `model::summary`; re-export its types + the
// height constants here so the `crate::state::SummaryState` /
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

/// At or below this width, Horizontal split cramps the main pane, so layout
/// is forced to the stacked (Vertical) tab-bar regardless of stored pref.
/// See [`AppState::effective_layout_mode`].
pub const NARROW_LAYOUT_MAX_WIDTH: u16 = 80;

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

/// Active sidebar tab. `Projects` lists tmux sessions (default); `Agents`
/// lists detected coding agents as the navigable list. Persisted to config.
/// Agent detection in the refresh worker runs only while `Agents` is active
/// (see `AppState::agents_tab_active`).
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

/// Step `delta` positions through `options` from `current`, wrapping at both
/// ends; a `current` not in the slice steps from the first option. Shared by
/// the settings cyclers and the port-forward form's field/mode cycling.
pub fn cycle_option<T: Copy + PartialEq>(options: &[T], current: T, delta: i32) -> T {
    let i = options.iter().position(|&o| o == current).unwrap_or(0) as i32;
    let n = options.len() as i32;
    options[(i + delta).rem_euclid(n) as usize]
}

/// Step `current` by `direction` (+1/-1) within `0..len`, clamped at both
/// ends (no wrap); `len == 0` yields 0. Shared by the bounded list cursors
/// (settings rows, theme picker, exclude editor, port-forward focus, pickers).
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

/// Apply a scroll `delta` to `current`, clamped to `0..=max`. Shared by the
/// summary card and its popup so the i32 round-trip lives in one place.
pub fn scroll_clamped(current: usize, delta: i32, max: usize) -> usize {
    (current as i32 + delta).clamp(0, max as i32) as usize
}

/// Clamp `value` to `min..=max` and store it in `*target` if it differs,
/// returning whether `*target` changed. Shared by the drag-resize setters.
pub fn clamp_set(target: &mut u16, value: u16, min: u16, max: u16) -> bool {
    let clamped = value.clamp(min, max);
    let changed = clamped != *target;
    *target = clamped;
    changed
}

/// Clamp `*cursor` to the last valid index of a `len`-length list (0 when
/// empty). Shared by the Projects/Agents focus clamps.
pub fn clamp_cursor(cursor: &mut usize, len: usize) {
    *cursor = if len == 0 { 0 } else { (*cursor).min(len - 1) };
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

/// One row in the unified sidebar session store. Local and remote share this
/// shape, keyed by `host` (`None` = local tmux server, `Some(host)` = remote
/// over ssh) per the "one data type, key by `Option<String>` host" rule.
/// `kind` carries the liveness/placeholder distinction — see [`SessionEntryKind`].
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// `None` = local tmux server; `Some(host)` = a remote host over ssh.
    pub host: Option<String>,
    pub name: String,
    pub dir: String,
    pub kind: SessionEntryKind,
}

/// What a [`SessionEntry`] represents. `Live` is a real attachable tmux
/// session; the other variants are synthetic status placeholders shown for a
/// remote group (one row per host) before/while its real session list arrives.
/// The variant (not a magic session name) marks a placeholder, so a real
/// session named `(no sessions)` is never mistaken for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEntryKind {
    /// A real tmux session deck can attach to. `is_current` is tracked for
    /// local sessions only; remote `Live` rows carry `false`.
    Live { is_current: bool },
    /// Synthetic placeholder: the host's ssh+tmux query hasn't returned yet.
    /// Renders muted "(connecting...)".
    Connecting,
    /// Synthetic placeholder: deck couldn't reach the host over ssh. Renders
    /// greyed, "(unreachable)".
    Unreachable,
    /// Synthetic placeholder: the host responded but its tmux server isn't
    /// up, so it has nothing to attach to. Renders "(no sessions)".
    NoSessions,
}

/// Display label for the synthetic "(no sessions)" placeholder.
pub const NO_SESSIONS_LABEL: &str = "(no sessions)";
/// Display label for the synthetic "(unreachable)" placeholder.
pub const UNREACHABLE_LABEL: &str = "(unreachable)";

impl SessionEntry {
    /// Whether deck can attach a PTY: a real `Live` session, not a synthetic
    /// Connecting/Unreachable/NoSessions placeholder. Attach/respawn must skip
    /// placeholders, else it spins forever trying to `tmux attach` a host with
    /// nothing to attach to, leaving the row stuck on "connecting…".
    pub fn is_attachable(&self) -> bool {
        matches!(self.kind, SessionEntryKind::Live { .. })
    }

    /// True for the local tmux server (`host == None`).
    pub fn is_local(&self) -> bool {
        self.host.is_none()
    }

    /// Whether this `Live` row is the current (attached) session. Always
    /// false for placeholders and for remote rows (remote `is_current` isn't
    /// tracked).
    pub fn is_current(&self) -> bool {
        matches!(
            self.kind,
            SessionEntryKind::Live {
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

/// Identifies a focused sidebar row by its flat index. The index walks rows
/// in render order, exactly the order of `state.entries` (locals first, then
/// each remote host's rows). `AppState::entry_at` decodes it back to the entry.
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
        SessionEntryKind::Unreachable => HostStatus::Unreachable,
        SessionEntryKind::Connecting => HostStatus::Connecting,
        SessionEntryKind::Live { .. } | SessionEntryKind::NoSessions => HostStatus::Connected,
    }
}

// --- Settings page state ---

/// UI state for the settings page and its sub-popovers (theme picker,
/// keybindings viewer). Update-check fields stay on `AppState` since many
/// paths outside settings touch them (refresh loop, banner, hit-testing).
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

/// The round-trippable persisted preferences in one unit, so the config↔state
/// mapping lives in exactly two places ([`Prefs::from_config`] /
/// [`Prefs::to_config`]). Each field mirrors a `Config` field (same default),
/// read widely as `state.prefs.<f>`.
///
/// Deliberately NOT here (stay direct `AppState`/`App` fields):
/// - `keybindings` (compiled) / `raw_keybindings` (serializable, on `App`);
/// - `collapsed_sections` — runtime state seeded from config *once* at startup,
///   not re-applied on hot-reload (bug #14);
/// - `config_remotes` and the `summary_prompt_version` constant — App/runtime.
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
    /// The Projects-tab summary prompt (session-framed), used when Generate is
    /// triggered on the Projects tab; `summary_prompt` is used on Agents.
    pub summary_prompt_projects: String,
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
    /// Whether the inline Summary card is shown. Off collapses the card to
    /// zero height (`summary_card_height`) so the list reclaims the rows.
    pub summary_enabled: bool,
}

impl Prefs {
    /// Build prefs from a loaded `Config`, applying load-time clamps/normalizations.
    /// `theme_index` is resolved by the caller (theme name → index lives outside
    /// config). One of the two config↔prefs mapping sites; inverse is
    /// [`Prefs::to_config`]. `summary_height` clamps to
    /// `SUMMARY_MIN_HEIGHT..=SUMMARY_MAX_HEIGHT`, same as `set_summary_height`.
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
            summary_prompt_projects: cfg.summary_prompt_projects.clone(),
            summary_model: cfg.summary_model.clone(),
            summary_height: cfg
                .summary_height
                .clamp(SUMMARY_MIN_HEIGHT, SUMMARY_MAX_HEIGHT),
            summary_language: cfg.summary_language.clone(),
            agents_probe_interval_secs: normalize_agents_probe_interval(cfg.agents_probe_interval),
            summary_enabled: cfg.summary_enabled,
        }
    }

    /// Map prefs back into a `Config`, filling the App/runtime fields not in
    /// `Prefs` from the caller args (`keybindings`, `remotes`, `collapsed`) and
    /// the `summary_prompt_version` constant. Inverse of [`Prefs::from_config`];
    /// `from_config(to_config(p)) == p` holds on the prefs fields.
    pub fn to_config(
        &self,
        keybindings: std::collections::BTreeMap<String, crate::config::KeyBindingValue>,
        remotes: Vec<crate::config::RemoteConfig>,
        collapsed: Vec<Option<String>>,
        collapsed_agents: Vec<Option<String>>,
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
            collapsed_agent_sections: collapsed_agents,
            summary_prompt: self.summary_prompt.clone(),
            summary_prompt_version: crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION,
            summary_prompt_projects: self.summary_prompt_projects.clone(),
            summary_prompt_projects_version: crate::summary::DEFAULT_SUMMARY_PROMPT_PROJECTS_VERSION,
            summary_model: self.summary_model.clone(),
            summary_height: self.summary_height,
            summary_language: self.summary_language.clone(),
            agents_probe_interval: self.agents_probe_interval_secs,
            summary_enabled: self.summary_enabled,
            transparent_bg: self.transparent_bg,
        }
    }
}

// --- AppState ---

pub struct AppState {
    // Session data
    /// Unified session store: local entries first (`host == None`), then each
    /// remote host's rows in config order — exactly the sidebar render/flat
    /// order. The `FocusTarget` flat index indexes this vec directly; per-row
    /// dispatch reads `entry.host` / `entry.kind`.
    pub entries: Vec<SessionEntry>,
    pub focused: usize,
    /// Name of the current (attached) *local* session. Remote current-
    /// session isn't tracked — a deliberate local-only invariant.
    pub current_session: String,
    /// Manual display order of *local* sessions, keyed by name. Remote
    /// reorder persists per-host to each remote tmux server instead.
    pub session_order: Vec<String>,

    // UI state
    pub main_view: MainView,
    pub focus_mode: FocusMode,
    /// Persisted preferences (theme, layout, sidebar geometry, summary, …).
    /// See [`Prefs`]. Read widely as `state.prefs.<field>`; loaded via
    /// [`AppState::apply_config`], written back via [`Prefs::to_config`].
    pub prefs: Prefs,
    /// Settings page navigation + theme picker / keybindings viewer
    /// overlays. See `SettingsState`.
    pub settings: SettingsState,
    /// Cursor row for the Agents tab, kept separate from `focused` (the
    /// Projects cursor) so switching tabs preserves each one's position.
    /// Indexes into `agent_entries()`.
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

    /// Every clickable region the sidebar publishes, captured whole each frame
    /// by the render loop and consulted by mouse dispatch via [`HitRegions::hit`].
    /// One field, not a dozen — geometry can't drift since all rect tests decode
    /// from here.
    pub hit_regions: HitRegions,

    /// Result of the most recent manual config reload. Rendered in the
    /// sidebar footer and auto-cleared by the main loop after a short
    /// TTL — see `RELOAD_STATUS_OK_TTL` / `RELOAD_STATUS_ERR_TTL`.
    pub reload_status: Option<ReloadStatus>,
    pub reload_status_at: Option<Instant>,

    /// The agent deck last switched to (via an agent-line click); its footer
    /// renders highlighted as "you are here". Identified by `(host, pane_id)`
    /// so the highlight is uniform local/remote (never branches on origin).
    /// Cleared by any non-agent switch.
    pub active_agent: Option<AgentTarget>,

    /// Mirror of `Config.remotes` so reducers can read per-host forwards
    /// without round-tripping through dispatch. Kept in sync by startup
    /// and `reload_config`.
    pub config_remotes: Vec<crate::config::RemoteConfig>,

    /// Per-forward liveness, refreshed each probe tick by the port-forward
    /// worker. Keyed by `ForwardKey`. Missing key = `Probing` (not yet seen).
    pub forward_health: HashMap<ForwardKey, ForwardHealth>,

    /// Interactive coding agents (Claude Code / Codex) detected per sidebar
    /// section, keyed by host (`None` = local, `Some(host)` = remote). An
    /// absent key hasn't been probed yet (rendered "claude …, codex …").
    /// Layout/render look a section up by key, never branching on local vs
    /// remote. Each value lists located agents (count + session/window/pane).
    /// See `crate::agent`.
    pub agents: HashMap<LaneId, Vec<crate::agent::DetectedAgent>>,

    /// The flattened Agents-tab list, twin of `entries` — stored, not derived
    /// per frame. Built from `agents` + host order (`rebuild_agent_entries`)
    /// when a refresh round settles, so renderer/layout/focus index a stable
    /// list. Local section first, then each remote host in
    /// `remote_hosts_in_order`; each section a run of agents or one placeholder.
    pub agent_entries: Vec<AgentEntry>,

    /// Sidebar groups the user has collapsed (Expanded view only).
    /// `lane(None)` = `@local`, `lane(Some(host))` = a `@host` group.
    /// A collapsed group renders as just its divider (rows hidden by the layout).
    /// Persisted to config (`collapsed_sections`), restored at startup.
    pub collapsed_sections: HashSet<LaneId>,

    /// Agents-tab twin of `collapsed_sections`, keyed the same way. Separate so
    /// a host collapsed on Projects doesn't hide its agent rows (and vice versa)
    /// — the two tabs fold independently. Persisted to config
    /// (`collapsed_agent_sections`), restored at startup.
    pub collapsed_agent_sections: HashSet<LaneId>,
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
                summary_prompt_projects: String::new(),
                summary_model: String::new(),
                summary_height: DEFAULT_SUMMARY_HEIGHT,
                summary_language: String::new(),
                agents_probe_interval_secs: DEFAULT_AGENTS_PROBE_INTERVAL,
                summary_enabled: true,
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
            agent_entries: Vec::new(),
            active_agent: None,
            collapsed_sections: HashSet::new(),
            collapsed_agent_sections: HashSet::new(),
        }
    }

    /// Apply every config-derived field shared by startup (`App::new`) and
    /// hot-reload (`reload_config`). One list, so a new config field can't be
    /// applied at startup but silently missed on reload (or vice versa).
    ///
    /// Deliberately NOT covered here:
    /// - `config_remotes` — reload diffs old vs new forwards/hosts around this
    ///   call and commits the new list itself;
    /// - `collapsed_sections` — runtime state seeded from config once at startup;
    ///   a reload must not stomp the user's live collapse state.
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

    /// Whether the Agents tab is the active sidebar view. The tab selector only
    /// exists in Horizontal layout (Vertical is a session tab-bar with no
    /// header), so Agents is gated to Horizontal — Vertical stays Projects even
    /// if `sidebar_tab` is `Agents`. Gates agent detection in the refresh worker
    /// and selects the agents layout / focus space.
    pub fn agents_tab_active(&self) -> bool {
        self.prefs.sidebar_tab == SidebarTab::Agents
            && self.effective_layout_mode() == LayoutMode::Horizontal
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

    /// The layout actually used for rendering, sizing, and hit-testing — vs
    /// `prefs.layout_mode`, the user's stored choice. On a narrow terminal
    /// ([`NARROW_LAYOUT_MAX_WIDTH`] columns or fewer) it's forced to
    /// [`LayoutMode::Vertical`], leaving the pref untouched (it reapplies once
    /// wide enough). Every layout-dependent branch (renderer, `pty_size`, mouse
    /// hit-testers) must read this, not the raw pref, or split and click
    /// geometry drift apart.
    pub fn effective_layout_mode(&self) -> LayoutMode {
        if self.term_width <= NARROW_LAYOUT_MAX_WIDTH {
            LayoutMode::Vertical
        } else {
            self.prefs.layout_mode
        }
    }

    pub fn effective_sidebar_height(&self) -> u16 {
        // Vertical layout is a single tab-switching row with no second detail
        // row to resize into, so the sidebar is pinned to exactly the tab bar
        // (plus border when shown) and the stored `sidebar_height` is ignored.
        if self.effective_layout_mode() == LayoutMode::Vertical {
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
        match self.effective_layout_mode() {
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

    /// Height of the sidebar footer in rows, for mouse hit-testing. The formula
    /// lives in `crate::geometry::sidebar_footer_height`, shared with the
    /// renderer (`ui::sidebar::draw_sidebar`) so the two can't drift — when they
    /// did, the bottom visible session row was click-dead.
    pub fn sidebar_footer_height(&self) -> u16 {
        let b = if self.prefs.show_borders { 2u16 } else { 0 };
        let content_width = match self.effective_layout_mode() {
            LayoutMode::Horizontal => self.prefs.sidebar_width.saturating_sub(b),
            LayoutMode::Vertical => self.term_width.saturating_sub(b),
        };
        crate::geometry::sidebar_footer_height(
            crate::geometry::banner_visible(self.update_available.is_some(), content_width),
            self.prefs.plugins.len(),
        )
    }

    /// Resolve a screen row in the scrollable session area into
    /// `(layout, viewport_y, scroll, visible_height)`. `None` when the row is in
    /// the header banner, footer, or outside the sidebar. Shared by the row and
    /// divider hit-testers so they agree on geometry and the applied scroll offset.
    fn session_row_hit(&self, row: u16) -> Option<(BuiltLayout, u16, u16, u16)> {
        let b = if self.prefs.show_borders { 1u16 } else { 0 };
        let sidebar_h = match self.effective_layout_mode() {
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
        // The list viewport sits above the Summary card (pinned to the bottom
        // of both tabs, between the list and the footer, not part of the
        // sectioned list).
        let list_bottom = sessions_bottom.saturating_sub(self.summary_card_height());
        if row < sessions_top || row >= list_bottom {
            return None;
        }
        let list_top = sessions_top;
        let visible_height = list_bottom - list_top;
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

    /// Map a screen row on a group divider to its section key (`None` =
    /// `@local`, `Some(host)` = remote `@host`). `None` when the row isn't on a
    /// real divider (e.g. a no-agents placeholder header, not a collapse target).
    /// Used by the mouse layer to toggle a group when its divider is clicked.
    pub fn divider_section_key_at(&self, row: u16) -> Option<Option<String>> {
        let (built, viewport_y, scroll, _) = self.session_row_hit(row)?;
        // header_at_y returns the 0-based header section index, which is a
        // direct index into `sections`. Only real dividers toggle collapse.
        let section_idx = built.layout.header_at_y(viewport_y, scroll)?;
        let meta = built.sections.get(section_idx)?;
        if meta.divider {
            Some(crate::system::tmux::TmuxSystem::host_of(&meta.lane).map(str::to_string))
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

    /// The entry at flat focus index `idx`, or `None` if out of range.
    /// `FocusTarget` numbering indexes `entries` directly (locals first, then
    /// remotes), so this is the single decode the reducers/action layer use for
    /// per-row dispatch — they read `entry.host` / `entry.kind`, not the index.
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

    /// Flat focusable index of the row for `session` on `host` (`None` = local),
    /// or `None` if not currently listed. Lets the sidebar move its highlight
    /// onto a session switched to out-of-band (e.g. an agent-footer click), so
    /// the highlight tracks the viewed session like keyboard nav does (j/k moves
    /// cursor *and* switches). Mirrors `FocusTarget` numbering: locals, then remotes.
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
                e.kind = SessionEntryKind::Connecting;
            }
        }
    }

    /// Agents detected in a sidebar section, addressed uniformly by host
    /// (`None` = local). `None` result = not probed yet. The layout uses
    /// this without caring whether the section is local or remote.
    pub fn section_agents(&self, host: Option<&str>) -> Option<&[crate::agent::DetectedAgent]> {
        self.agents
            .get(lane(host).as_str())
            .map(Vec::as_slice)
    }

    /// Fold a remote refresh round's agent detection into `agents`.
    /// `covered_hosts` = every host queried; `fresh` = per-host result for
    /// hosts whose probe succeeded. A covered host missing from `fresh` had a
    /// failed probe, so its stale list is dropped (else dead pane ids keep
    /// rendering as clickable footer lines). The local `None` key is untouched;
    /// hosts no longer configured are pruned.
    pub fn apply_remote_agents(
        &mut self,
        covered_hosts: std::collections::HashSet<String>,
        fresh: HashMap<String, Vec<crate::agent::DetectedAgent>>,
    ) {
        for host in covered_hosts {
            if !fresh.contains_key(&host) {
                self.agents.remove(lane(Some(&host)).as_str());
            }
        }
        for (host, list) in fresh {
            self.agents.insert(lane(Some(&host)), list);
        }
        let configured: std::collections::HashSet<&str> = self
            .config_remotes
            .iter()
            .map(|r| r.host.as_str())
            .collect();
        self.agents.retain(|k, _| {
            let l = k.lane();
            l == "local" || configured.contains(l)
        });
    }

    /// The active theme, resolved from the saved index. Layout builders bake
    /// divider accents and row marker colors into their `BasicItem`s, so they
    /// read the theme here rather than taking it as a parameter (hit-test call
    /// sites build the same layout and don't carry a theme).
    pub fn active_theme(&self) -> &'static crate::theme::Theme {
        &crate::theme::THEMES[self.prefs.theme_index]
    }

    /// `BasicItem` for one session row. Expanded carries the name plus a dim
    /// `dir` line (height 2); Compact is a single `origin:name` line.
    fn session_item(&self, e: &SessionEntry, view_mode: ViewMode) -> BasicItem {
        let loading = matches!(e.kind, SessionEntryKind::Connecting);
        let name = if loading {
            "(connecting…)".to_string()
        } else {
            match e.kind {
                SessionEntryKind::Unreachable => UNREACHABLE_LABEL.to_string(),
                SessionEntryKind::NoSessions => NO_SESSIONS_LABEL.to_string(),
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

    /// `BasicItem` for one Agents-tab row, twin of `session_item`. A real agent
    /// shows a status glyph (`●` working / `○` idle / `◐` waiting / `○` unknown)
    /// before its location; the renderer tints the glyph by `AgentStatus`
    /// (`recolor_agent_dot`) — color keys off status, not glyph, so two statuses
    /// may reuse a glyph. The current pane isn't marked here: the row highlight
    /// (cursor follows the active pane, see `steer_marker_to_pane`) carries "you
    /// are here". Empty-section placeholder shows `detecting…` (not yet probed)
    /// or `no agents` (probed, none found).
    fn agent_item(&self, entry: &AgentEntry) -> BasicItem {
        match &entry.kind {
            AgentEntryKind::Agent(agent) => {
                let dot = match agent.status {
                    crate::agent::AgentStatus::Working => "●",
                    crate::agent::AgentStatus::Idle => "○",
                    crate::agent::AgentStatus::Waiting => "◐",
                    crate::agent::AgentStatus::Unknown => "○",
                };
                BasicItem::new(format!("{dot} {}", agent.location()))
            }
            AgentEntryKind::Placeholder { probed } => {
                BasicItem::new(if *probed { "no agents" } else { "detecting…" })
            }
        }
    }

    /// Shared skeleton behind both sidebar tabs: a local section then one per
    /// remote host (in `remote_hosts_in_order`). Each gets its `@local`/`@host`
    /// divider plus matching [`SectionMeta`] (when `opts.show_headers`), then
    /// `push_rows` fills the body (`host` `None` = local, `Some` = remote). The
    /// tabs are structurally identical, differing only in `opts` and the rows
    /// `push_rows` emits.
    ///
    /// `push_rows` may also push its own placeholder header + `SectionMeta` (the
    /// Agents tab does this for empty sections); those land after the divider,
    /// keeping `sections` parallel to the crate's section numbering.
    fn build_sections(
        &self,
        opts: SectionLayoutOpts,
        collapsed: &HashSet<LaneId>,
        mut push_rows: impl FnMut(&mut SidebarLayout, &mut Vec<SectionMeta>, &LaneId),
    ) -> BuiltLayout {
        use crate::system::tmux::TmuxSystem;

        let mut layout = SidebarLayout::new();
        let mut sections: Vec<SectionMeta> = Vec::new();
        layout.set_collapsible(opts.collapsible);

        // Section headers in push order: (section_idx, lane), so collapse flags
        // can be flipped once the list is built.
        let mut group_headers: Vec<(usize, LaneId)> = Vec::new();

        // Lanes to lay out, in display order: the local lane, then each remote
        // host as it first appears in `entries` (config order). The *shell*
        // enumerates the lanes — not the system — so every session row keeps a
        // section even before the system would list that lane.
        let mut lanes = vec![TmuxSystem::local_lane()];
        lanes.extend(self.remote_hosts_in_order_ref().map(TmuxSystem::host_lane));

        let theme = self.active_theme();
        let ctx = crate::system::SectionCtx {
            remotes: &self.config_remotes,
            forward_health: &self.forward_health,
        };

        for lane_id in &lanes {
            // The lane's owning system styles the divider: title, accent,
            // buttons, badge.
            let def = crate::system::for_lane(lane_id).section_for(lane_id, &ctx);
            if opts.show_headers {
                let color = if def.accent == usize::MAX {
                    theme.accent
                } else {
                    host_accent(theme, def.accent)
                };
                let mut header = BasicItem::new(def.title.clone())
                    .separator("─")
                    .color(color);
                for b in &def.buttons {
                    header = header.button(b.glyph.clone());
                }
                group_headers.push((sections.len(), def.lane.clone()));
                // The local section stays flush; remote sections take a 1-row
                // top margin when the tab asks for it.
                if def.top_margin && opts.remote_header_margin {
                    layout.push_header_margin(header, 1);
                } else {
                    layout.push_header_auto(header);
                }
                sections.push(SectionMeta {
                    lane: def.lane.clone(),
                    title: def.title,
                    buttons: def.buttons,
                    divider: true,
                    badge: def.badge,
                });
            }
            push_rows(&mut layout, &mut sections, lane_id);
        }

        // Flip each header's collapsed flag (from the caller's collapse set:
        // `collapsed_sections` for Projects, `collapsed_agent_sections` for
        // Agents — folds independently) so the widget hides its rows and
        // geometry/scroll/hit-test all honor the collapse.
        if opts.collapsible {
            for (section_idx, key) in group_headers {
                layout.set_collapsed(section_idx, collapsed.contains(&key));
            }
        }

        BuiltLayout { layout, sections }
    }

    /// Build the unified Projects-tab layout: a flat `BasicItem` list of
    /// `@local`/`@host` dividers (Expanded only) interleaved with session rows,
    /// plus the per-divider [`SectionMeta`] the hit-tester resolves clicks
    /// against. Renderer and hit-tester share this so they can't disagree.
    pub fn sidebar_layout(&self, view_mode: ViewMode) -> BuiltLayout {
        // Group dividers (`@local`, `@host`) are an Expanded-view adornment;
        // Compact rows already carry an origin prefix. Collapse is likewise an
        // Expanded-only feature.
        let show_headers = matches!(view_mode, ViewMode::Expanded);
        self.build_sections(
            SectionLayoutOpts {
                show_headers,
                collapsible: show_headers,
                remote_header_margin: show_headers,
            },
            &self.collapsed_sections,
            |layout, _sections, lane_id| {
                // `entries` is grouped by host and contiguous, so filtering by
                // the lane's host yields each section's rows in flat-index order.
                let host = crate::system::tmux::TmuxSystem::host_of(lane_id);
                for e in self.entries.iter().filter(|e| e.host.as_deref() == host) {
                    layout.push_row_auto(self.session_item(e, view_mode));
                }
            },
        )
    }

    /// Distinct remote hosts as `&str` in first-appearance order in `entries`
    /// (the refresh worker emits hosts in config order, one contiguous block
    /// each). Shared by `build_sections` and `build_agent_entries` so the
    /// sidebar sections and the flattened agent list walk hosts identically.
    fn remote_hosts_in_order_ref(&self) -> impl Iterator<Item = &str> {
        let mut seen: HashSet<&str> = HashSet::new();
        self.entries
            .iter()
            .filter_map(|e| e.host.as_deref())
            .filter(move |host| seen.insert(host))
    }

    /// Agents-tab entries for one section (`None` = local): one
    /// [`AgentEntryKind::Agent`] per detected agent, or a single
    /// [`AgentEntryKind::Placeholder`] when empty (probed, none found) or not
    /// yet probed (`detecting…`). Every section yields at least one entry — like
    /// a Projects host always carrying a `NoSessions` row — so it always holds a
    /// focus slot. `agent_entries` and the layout both walk this.
    fn agent_entries_for(&self, host: Option<&str>) -> Vec<AgentEntry> {
        let mk = |kind| AgentEntry {
            host: host.map(str::to_string),
            kind,
        };
        match self.section_agents(host) {
            Some(list) if !list.is_empty() => list
                .iter()
                .cloned()
                .map(|agent| mk(AgentEntryKind::Agent(agent)))
                .collect(),
            other => vec![mk(AgentEntryKind::Placeholder {
                probed: other.is_some(),
            })],
        }
    }

    /// Recompute the stored [`agent_entries`](Self::agent_entries) from the
    /// `agents` map and current host order. Called when a refresh round settles
    /// (`App::apply_update`) — the one point where both detection and host order
    /// are fresh — mirroring how `entries` is rebuilt. Cheap, but not per-frame:
    /// layout/focus then read the stored list directly.
    pub fn rebuild_agent_entries(&mut self) {
        self.agent_entries = self.build_agent_entries();
    }

    /// Build the flattened entry list: the local section first, then each
    /// remote host in section order. Each section is a run of detected agents,
    /// or a single placeholder entry when empty.
    fn build_agent_entries(&self) -> Vec<AgentEntry> {
        let mut entries = self.agent_entries_for(None);
        for host in self.remote_hosts_in_order_ref() {
            entries.extend(self.agent_entries_for(Some(host)));
        }
        entries
    }

    /// Number of focusable Agents-tab entries — just the stored list's length,
    /// since every section contributes at least a placeholder entry.
    pub fn agent_count(&self) -> usize {
        self.agent_entries.len()
    }

    /// Flat focusable index of the agent entry matching `target`, or `None` if
    /// not listed. Lets the Agents-tab cursor track the pane switched to via a
    /// click, like `focusable_index_for` does for Projects. Placeholder entries
    /// never match a real target.
    pub fn agent_entry_index_for(&self, target: &AgentTarget) -> Option<usize> {
        self.agent_entries.iter().position(|entry| {
            entry.host.as_deref() == target.host.as_deref()
                && entry.agent().is_some_and(|a| a.pane_id == target.pane_id)
        })
    }

    /// Row height the Summary card reserves: title + blank + `summary_height`
    /// body + a drag-handle row. Fixed-size for every state, so overflowing
    /// Ready text scrolls inside it rather than growing the card; the user
    /// resizes by dragging the handle.
    pub fn summary_card_height(&self) -> u16 {
        if !self.prefs.summary_enabled {
            return 0;
        }
        3 + self.prefs.summary_height
    }

    /// Set the card body height (rows), clamped to the drag-resize bounds.
    /// Returns whether it changed.
    pub fn set_summary_height(&mut self, rows: u16) -> bool {
        clamp_set(
            &mut self.prefs.summary_height,
            rows,
            SUMMARY_MIN_HEIGHT,
            SUMMARY_MAX_HEIGHT,
        )
    }

    /// Whether `pos` falls anywhere on the Summary card. Used by the wheel path
    /// to route scroll to the card text. Checked directly, not via
    /// `HitRegions::hit` priority: the card rect spans the whole Agents-tab
    /// viewport, and the rows/dividers over it outrank it for *clicks* but not
    /// the wheel.
    pub fn summary_card_at(&self, col: u16, row: u16) -> bool {
        let pos = Position::new(col, row);
        self.hit_regions
            .summary
            .card
            .is_some_and(|r| r.contains(pos))
    }

    /// Whether `(col, row)` is on the card's top drag-handle row. The card
    /// is pinned to the bottom, so its top edge is the resize boundary.
    pub fn summary_resize_at(&self, col: u16, row: u16) -> bool {
        self.hit_regions.summary.card.is_some_and(|r| {
            row == r.y && col >= r.x && col < r.x + r.width
        })
    }

    /// New body height implied by dragging the top handle to `row`. The card
    /// bottom is anchored to the footer, so dragging the top up grows the card:
    /// `body = (card_bottom - row) - chrome` (chrome = handle, title, blank).
    /// Clamped by `set_summary_height`.
    pub fn summary_height_for_drag(&self, row: u16) -> u16 {
        let bottom = self
            .hit_regions
            .summary
            .card
            .map_or(0, |r| r.y + r.height);
        bottom.saturating_sub(row).saturating_sub(3)
    }

    /// Apply a wheel/keyboard scroll delta to the Summary text, clamped to
    /// the captured max offset.
    pub fn scroll_summary(&mut self, delta: i32) {
        self.summary.scroll =
            scroll_clamped(self.summary.scroll, delta, self.hit_regions.summary.max_scroll);
    }

    /// Move the summary card off `Generating` back to the pre-generation state
    /// (Idle / prior Ready / Error), used on a mid-flight cancel. The App side
    /// drops the worker (killing the `claude` child); this is the pure state
    /// half. No-op unless currently generating.
    pub fn cancel_summary(&mut self) {
        if self.summary.state != SummaryState::Generating {
            return;
        }
        self.summary.state = self.summary.before_generating.take().unwrap_or_default();
        self.summary.scroll = 0;
    }

    /// Apply a scroll delta to the summary popup, clamped to its max.
    pub fn scroll_summary_popup(&mut self, delta: i32) {
        self.summary.popup_scroll =
            scroll_clamped(self.summary.popup_scroll, delta, self.summary.popup_max_scroll);
    }

    /// Build the Agents-tab layout: an `@local`/`@host` divider per section with
    /// its rows beneath — a focusable row per detected agent, or one placeholder
    /// when empty (`detecting…` / `no agents`). Every row maps 1:1 to a stored
    /// `agent_entries` element so focus/scroll/hit-test stay in sync.
    pub fn agents_layout(&self) -> BuiltLayout {
        // Sections fold independently of Projects via `collapsed_agent_sections`.
        // Remote sections take a 1-row top margin. The Summary card is a separate
        // widget pinned above, so the list is pure `BasicItem`. Body mirrors
        // `sidebar_layout`: filter the stored list by host, build each entry into
        // a `BasicItem` via the `agent_item` twin of `session_item`.
        self.build_sections(
            SectionLayoutOpts {
                show_headers: true,
                collapsible: true,
                remote_header_margin: true,
            },
            &self.collapsed_agent_sections,
            |layout, _sections, lane_id| {
                let host = crate::system::tmux::TmuxSystem::host_of(lane_id);
                for entry in self
                    .agent_entries
                    .iter()
                    .filter(|e| e.host.as_deref() == host)
                {
                    layout.push_row_auto(self.agent_item(entry));
                }
            },
        )
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
    /// remote-forward listener lives on the far side, can't be probed locally,
    /// so it mirrors reachability: connected → Up, unreachable → Down,
    /// connecting → Probing. `-L`/`-D` are owned by the worker probe and left
    /// untouched. Called on remote status change so `-R` and the divider agree.
    pub fn sync_remote_forward_health(&mut self) {
        let updates: Vec<(ForwardKey, ForwardHealth)> = self
            .config_remotes
            .iter()
            .flat_map(|r| {
                r.forwards
                    .iter()
                    .filter(|f| matches!(f.mode, crate::forwards::ForwardMode::Remote))
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

    /// Focused remote placeholder row, if any. These occupy normal focus slots
    /// so users can land on a host with no attachable session, but the main pane
    /// must render an explicit status instead of a stale terminal screen.
    pub fn focused_remote_placeholder(&self) -> Option<&SessionEntry> {
        if self.agents_tab_active() {
            return None;
        }
        let entry = self.entry_at(self.focus_target()?)?;
        (!entry.is_local() && !entry.is_attachable()).then_some(entry)
    }

    /// Section key of the group flat focus index `idx` lives in: `None` local,
    /// `Some(host)` remote. Used by the section-toggle keybinding and focus-skip
    /// logic; out-of-range falls back to `None`.
    pub fn section_key_of_focus(&self, idx: usize) -> Option<String> {
        self.entries.get(idx).and_then(|e| e.host.clone())
    }

    /// Host of the group the Agents-tab cursor row lives in (`None` = local),
    /// the agent twin of `section_key_of_focus`. Used by the section-toggle
    /// keybinding and focus-skip logic on the Agents tab.
    pub fn agent_section_key_of_focus(&self) -> Option<String> {
        self.agent_entries
            .get(self.agent_focused)
            .and_then(|e| e.host.clone())
    }

    /// Whether the row at flat focus index `idx` sits in a collapsed group
    /// (so keyboard focus should skip over it). Tab-aware: each tab folds
    /// against its own collapse set.
    pub fn is_focus_collapsed(&self, idx: usize) -> bool {
        if self.agents_tab_active() {
            return self.agent_entries.get(idx).is_some_and(|e| {
                self.collapsed_agent_sections
                    .contains(lane(e.host.as_deref()).as_str())
            });
        }
        idx < self.focusable_count()
            && self
                .collapsed_sections
                .contains(lane(self.section_key_of_focus(idx).as_deref()).as_str())
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

    /// Move both section cursors onto `target`: the Projects `focused` session
    /// and the Agents `agent_focused` row, so the highlight tracks whatever pane
    /// is active — whether deck drove the switch (`commit_focus`) or it follows
    /// the real active pane (`steer_marker_to_pane`). Each cursor moves only if
    /// the target is in that list.
    pub fn focus_cursors_on(&mut self, target: &AgentTarget) {
        if let Some(idx) = self.focusable_index_for(target.host.as_deref(), &target.session) {
            self.focused = idx;
        }
        if let Some(idx) = self.agent_entry_index_for(target) {
            self.agent_focused = idx;
        }
    }

    /// Track the active pane on `host` (`None` = local): set `active_agent` to
    /// the agent occupying `pane_id`, or clear it when that pane has no agent —
    /// so active-agent state follows the real active pane even when the user
    /// switches panes outside Deck. When an agent is found the section cursor
    /// follows it (`focus_cursors_on`); a pane with no agent only clears
    /// `active_agent` and leaves the cursor put. No-op when the host's agents
    /// aren't probed yet, so a probe racing ahead of detection can't blank a
    /// valid highlight (absence = "not known", not "no agent here").
    pub fn steer_marker_to_pane(&mut self, host: Option<&str>, pane_id: &str) {
        let target = match self.agents.get(lane(host).as_str()) {
            None => return,
            Some(list) => list
                .iter()
                .find(|a| a.pane_id == pane_id)
                .map(|a| AgentTarget {
                    host: host.map(str::to_string),
                    session: a.session.clone(),
                    pane_id: a.pane_id.clone(),
                }),
        };
        if let Some(t) = &target {
            self.focus_cursors_on(t);
        }
        self.active_agent = target;
    }

    /// The agent under the Agents-tab cursor, or `None` when off-tab or
    /// no agent is focused. Resolves the cursor through `agent_entries`.
    pub fn focused_agent(&self) -> Option<AgentTarget> {
        if !self.agents_tab_active() {
            return None;
        }
        let entry = self.agent_entries.get(self.agent_focused)?;
        // The guard that makes a placeholder entry inert: there's no pane to
        // switch to, so the cursor can land on it but Enter/click no-op —
        // mirroring how Projects guards a `NoSessions` row (`is_attachable`).
        match &entry.kind {
            AgentEntryKind::Agent(agent) => Some(AgentTarget {
                host: entry.host.clone(),
                session: agent.session.clone(),
                pane_id: agent.pane_id.clone(),
            }),
            AgentEntryKind::Placeholder { .. } => None,
        }
    }

    /// The highest-priority full-input modal currently open, or `None` when the
    /// sidebar/PTY takes input directly. The order below is the source of truth
    /// for input routing and **must mirror `keyboard::key_to_action`'s
    /// early-return chain exactly** — both consult this first, so priority here
    /// decides which overlay swallows a key/click when several flags are set.
    ///
    /// The settings sub-modals (KeybindingsView / ExcludeEditor / SummaryLang)
    /// count only while the settings page owns focus (`MainView::Settings` +
    /// `FocusMode::Main`); elsewhere their backing fields are stale and must not
    /// gate input. Everything above them is a standalone overlay openable from
    /// the sidebar.
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

    /// Session name for the kill-confirmation overlay: the focused row's name,
    /// or `None` when no kill is pending or focus has no valid target (the
    /// renderer gates the overlay on `Some`). Resolves via `entry_at`, so a
    /// remote row reports its name too — local and remote treated alike.
    pub fn confirm_kill_name(&self) -> Option<String> {
        if !self.overlay.confirm_kill {
            return None;
        }
        Some(self.entry_at(self.focus_target()?)?.name.clone())
    }

    /// Why the focused kill `target` can't be killed, or `None` if it can.
    /// Shared by the `x`-key path (`KillSession` / `ConfirmKill`) and the
    /// context menu's "Kill" greying so they can't drift:
    ///  - a synthetic placeholder remote row (loading / unreachable /
    ///    "(no sessions)") has no real session — a kill would send
    ///    `ssh tmux kill-session` with a placeholder/empty name;
    ///  - a host's last live session would tear that host's tmux server down;
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
    /// (locals then remotes) after the list changes — e.g. a focused row
    /// disappeared. Clamps against the Projects row space specifically, not the
    /// tab-aware `focusable_count` (which would use the agent count on the
    /// Agents tab and corrupt the Projects cursor).
    pub fn clamp_projects_focus(&mut self) {
        clamp_cursor(&mut self.focused, self.entries.len());
    }

    /// Identity (host, session name) of the focused Projects row, captured
    /// before a refresh rebuilds `entries` so the cursor can re-anchor to the
    /// same session afterwards. The Projects twin of `focused_agent_key`.
    pub fn focused_session_key(&self) -> Option<(Option<String>, String)> {
        self.entries
            .get(self.focused)
            .map(|e| (e.host.clone(), e.name.clone()))
    }

    /// Re-point the Projects cursor at the session `key` (its position before
    /// `entries` was rebuilt), so the highlight keeps tracking the same session
    /// across a refresh that reordered/resized the list instead of sliding onto
    /// a neighbor. Falls back to clamping when the session is gone. Projects twin
    /// of `reanchor_agent_focus`; use instead of `clamp_projects_focus` after a
    /// refresh rebuilds the rows.
    pub fn reanchor_projects_focus(&mut self, key: Option<(Option<String>, String)>) {
        match key.and_then(|(host, name)| self.focusable_index_for(host.as_deref(), &name)) {
            Some(idx) => self.focused = idx,
            None => self.clamp_projects_focus(),
        }
    }

    /// Keep the Agents-tab cursor inside the current agent list after the
    /// detected agents change (agents come and go between refresh rounds).
    pub fn clamp_agent_focus(&mut self) {
        let total = self.agent_count();
        clamp_cursor(&mut self.agent_focused, total);
    }

    /// Identity (host, `%N` pane id) of the agent under the Agents-tab cursor.
    /// Captured *before* a refresh rebuilds the agent list so the cursor can be
    /// re-anchored afterwards — see
    /// [`reanchor_agent_focus`](Self::reanchor_agent_focus).
    pub fn focused_agent_key(&self) -> Option<(Option<String>, String)> {
        let entry = self.agent_entries.get(self.agent_focused)?;
        match &entry.kind {
            AgentEntryKind::Agent(agent) => Some((entry.host.clone(), agent.pane_id.clone())),
            AgentEntryKind::Placeholder { .. } => None,
        }
    }

    /// Re-point the Agents-tab cursor at the agent `key` (its position before
    /// the list was rebuilt), so the highlighted row keeps tracking the same
    /// agent — and thus the pane shown on the right (`active_agent`). The
    /// detected-agent list reorders and gains/loses entries between rounds, so a
    /// bare `clamp_agent_focus` on the positional `agent_focused` would slide
    /// onto a different agent than the pane shows. Falls back to clamping when
    /// the agent is gone (finished, idle, or host dropped). Use instead of
    /// `clamp_agent_focus` after the agent list changes.
    pub fn reanchor_agent_focus(&mut self, key: Option<(Option<String>, String)>) {
        let found = key.and_then(|(host, pane_id)| {
            self.agent_entries.iter().position(|entry| {
                entry.host.as_deref() == host.as_deref()
                    && entry.agent().is_some_and(|a| a.pane_id == pane_id)
            })
        });
        let total = self.agent_entries.len();
        match found {
            Some(idx) => self.agent_focused = idx,
            None => clamp_cursor(&mut self.agent_focused, total),
        }
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
    /// match `session_order`. Remotes follow the local block, so sorting only
    /// the local prefix keeps the "locals first, then remotes (config order)"
    /// invariant intact.
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
        let (min, max) = self.sidebar_width_bounds();
        clamp_set(&mut self.prefs.sidebar_width, new_width, min, max)
    }

    /// Clamp and set sidebar height. Returns true if it changed.
    pub fn resize_sidebar_height(&mut self, new_height: u16) -> bool {
        let (min, max) = self.sidebar_height_bounds();
        clamp_set(&mut self.prefs.sidebar_height, new_height, min, max)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/model/state.rs"]
mod tests;

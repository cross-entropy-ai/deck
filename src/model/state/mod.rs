use std::collections::{HashMap, HashSet};
use std::time::Instant;

use ratatui::layout::Position;
use ratatui_sectioned_list::widget::BasicItem;
use ratatui_sectioned_list::{ItemKind, RowDragState};
use serde::{Deserialize, Serialize};

use crate::geometry::{
    context_menu_rect, shorten_dir, tab_bar_layout, tab_label, AgentEntry, AgentEntryKind,
    AgentTarget, BuiltLayout, HitRegions, SectionLayoutOpts, SectionMeta, SidebarLayout,
};
use crate::keybindings::Keybindings;
use crate::lane::LaneId;
use crate::overlay::{Modal, OverlayState};
use crate::summary_card::{SummaryCard, SummaryState, SUMMARY_MAX_HEIGHT, SUMMARY_MIN_HEIGHT};
use crate::system::tmux::lane;
use crate::update::{UpdateCheckMode, UpdateStatus};

mod focus;
mod layout;

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

/// How long a press must be held before the project-drag `↕`/`▸` markers
/// appear. Every click on a row starts a drag (release decides whether the
/// gesture was a click or a reorder), so showing them immediately flashed
/// drag affordances at users who were only switching sessions. Crossing to
/// another row still shows them at once — see
/// [`AppState::update_project_drag`].
pub const PROJECT_DRAG_INDICATOR_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

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

/// Frame-rate options as `(fps, settings label)`. Cycling order — the array is
/// a cycle, so it's rotated to put the default first and `option_row` can fall
/// back to row 0.
pub const FRAME_RATE_LIMIT_OPTIONS: [(u16, &str); 4] = [
    (30, "Smooth 30 FPS"),
    (2, "Power Saver 2 FPS"),
    (5, "Balanced 5 FPS"),
    (10, "Responsive 10 FPS"),
];
pub const DEFAULT_FRAME_RATE_LIMIT: u16 = FRAME_RATE_LIMIT_OPTIONS[0].0;

/// How often the Agents tab probes for agents + their status, in seconds, as
/// `(secs, settings label)`. Cycled in settings; default first as above.
pub const AGENTS_PROBE_INTERVAL_OPTIONS: [(u64, &str); 4] = [
    (2, "2s (normal)"),
    (5, "5s (slow)"),
    (10, "10s (very slow)"),
    (1, "1s (fast)"),
];
pub const DEFAULT_AGENTS_PROBE_INTERVAL: u64 = AGENTS_PROBE_INTERVAL_OPTIONS[0].0;

/// The `(value, label)` row for `value`, falling back to the default row when
/// `value` isn't one of the options.
fn option_row<T: Copy + PartialEq>(table: &[(T, &'static str)], value: T) -> (T, &'static str) {
    table
        .iter()
        .copied()
        .find(|&(v, _)| v == value)
        .unwrap_or(table[0])
}

pub fn normalize_agents_probe_interval(secs: u64) -> u64 {
    option_row(&AGENTS_PROBE_INTERVAL_OPTIONS, secs).0
}

pub fn agents_probe_interval_label(secs: u64) -> &'static str {
    option_row(&AGENTS_PROBE_INTERVAL_OPTIONS, secs).1
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
    let Some(last) = len.checked_sub(1) else {
        return 0;
    };
    if direction >= 0 {
        current.saturating_add(direction as usize).min(last)
    } else {
        // Backward steps don't clamp to `last`: an already out-of-range cursor
        // walks back into range rather than jumping there.
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
    *cursor = (*cursor).min(len.saturating_sub(1));
}

pub fn normalize_frame_rate_limit(fps: u16) -> u16 {
    option_row(&FRAME_RATE_LIMIT_OPTIONS, fps).0
}

pub fn frame_rate_limit_label(fps: u16) -> &'static str {
    option_row(&FRAME_RATE_LIMIT_OPTIONS, fps).1
}

// --- Session data ---

/// One row in the unified sidebar session store. Local and remote share this
/// shape, keyed by `host` (`None` = local tmux server, `Some(host)` = remote
/// over ssh) per the "one data type, key by `Option<String>` host" rule.
/// `kind` carries the liveness/placeholder distinction — see [`SessionEntryKind`].
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// Stable routing identity. `host` remains temporarily for tmux-specific
    /// labels and configuration lookups, but control paths use this lane.
    pub lane: crate::lane::LaneId,
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
    /// A synthetic status row for a remote host (`Connecting`/`Unreachable`/
    /// `NoSessions`): no session name or dir, the label comes from `kind`.
    pub fn placeholder(host: &str, kind: SessionEntryKind) -> Self {
        Self {
            lane: crate::system::tmux::TmuxSystem::host_lane(host),
            host: Some(host.to_string()),
            name: String::new(),
            dir: String::new(),
            kind,
        }
    }

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

    /// The row's display label. The placeholder strings are derived from
    /// `kind` here rather than stored as magic session names, so a real
    /// session called `(no sessions)` is never mistaken for a placeholder.
    /// `Connecting` shows its raw name — the sectioned list substitutes
    /// "(connecting…)" itself, while tabs mode draws the name.
    pub fn display_name(&self) -> &str {
        match self.kind {
            SessionEntryKind::Unreachable => UNREACHABLE_LABEL,
            SessionEntryKind::NoSessions => NO_SESSIONS_LABEL,
            SessionEntryKind::Live { .. } | SessionEntryKind::Connecting => &self.name,
        }
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

// --- Settings page state ---

/// UI state for the settings page and its sub-popovers (theme picker,
/// keybindings viewer). Update-check fields stay on `AppState` since many
/// paths outside settings touch them (refresh loop, banner, hit-testing).
#[derive(Debug, Default)]
pub struct SettingsState {
    pub selected: usize,

    /// Theme picker overlay (open inside the settings page).
    pub theme_picker_open: bool,
    pub theme_picker_selected: usize,
    /// Which theme the open picker is choosing: the fixed one, or the
    /// dark/light slot "follow terminal" mode picks from.
    pub theme_picker_slot: crate::theme::ThemeSlot,

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
    /// Follow the host terminal's appearance: use `dark_theme_index` or
    /// `light_theme_index` (per the probed terminal background, see
    /// `AppState::terminal_is_dark`) instead of `theme_index`.
    pub theme_auto: bool,
    pub dark_theme_index: usize,
    pub light_theme_index: usize,
    pub layout_mode: LayoutMode,
    pub show_borders: bool,
    /// Use the terminal's default (transparent) background instead of the
    /// theme's solid background color.
    pub transparent_bg: bool,
    /// Active sidebar tab; see [`SidebarTab`]. Persisted to config.
    pub sidebar_tab: SidebarTab,
    pub sidebar_width: u16,
    pub sidebar_height: u16,
    pub view_mode: ViewMode,
    pub frame_rate_limit: u16,
    pub exclude_patterns: Vec<String>,
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
            theme_auto: cfg.theme_auto,
            // Resolved here rather than by the caller (unlike `theme_index`,
            // which callers also diff against the live prefs): nothing outside
            // needs these two indices before the prefs exist.
            dark_theme_index: crate::theme::index_of(&cfg.dark_theme),
            light_theme_index: crate::theme::index_of(&cfg.light_theme),
            layout_mode: cfg.layout,
            show_borders: cfg.show_borders,
            transparent_bg: cfg.transparent_bg,
            sidebar_tab: cfg.sidebar_tab,
            sidebar_width: cfg.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX),
            sidebar_height: cfg.sidebar_height,
            view_mode: cfg.view_mode,
            frame_rate_limit: normalize_frame_rate_limit(cfg.frame_rate_limit),
            exclude_patterns: cfg.exclude_patterns.clone(),
            update_check_mode: cfg.update_check,
            summary_prompt: cfg.summary_prompt.clone(),
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
            theme_auto: self.theme_auto,
            dark_theme: crate::theme::THEMES[self.dark_theme_index].name.to_string(),
            light_theme: crate::theme::THEMES[self.light_theme_index]
                .name
                .to_string(),
            layout: self.layout_mode,
            show_borders: self.show_borders,
            sidebar_tab: self.sidebar_tab,
            sidebar_width: self.sidebar_width,
            sidebar_height: self.sidebar_height,
            view_mode: self.view_mode,
            frame_rate_limit: self.frame_rate_limit,
            exclude_patterns: self.exclude_patterns.clone(),
            keybindings,
            update_check: self.update_check_mode,
            remotes,
            collapsed_sections: collapsed,
            collapsed_agent_sections: collapsed_agents,
            summary_prompt: self.summary_prompt.clone(),
            summary_prompt_version: crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION,
            summary_model: self.summary_model.clone(),
            summary_height: self.summary_height,
            summary_language: self.summary_language.clone(),
            agents_probe_interval: self.agents_probe_interval_secs,
            summary_enabled: self.summary_enabled,
            transparent_bg: self.transparent_bg,
        }
    }

    /// The theme index a picker slot currently holds.
    pub fn theme_slot(&self, slot: crate::theme::ThemeSlot) -> usize {
        match slot {
            crate::theme::ThemeSlot::Fixed => self.theme_index,
            crate::theme::ThemeSlot::Dark => self.dark_theme_index,
            crate::theme::ThemeSlot::Light => self.light_theme_index,
        }
    }

    /// Point a picker slot at `index`. Choosing a fixed theme also leaves
    /// "follow terminal" mode — otherwise the pick would have no visible
    /// effect and the picker would look broken.
    pub fn set_theme_slot(&mut self, slot: crate::theme::ThemeSlot, index: usize) {
        match slot {
            crate::theme::ThemeSlot::Fixed => {
                self.theme_index = index;
                self.theme_auto = false;
            }
            crate::theme::ThemeSlot::Dark => self.dark_theme_index = index,
            crate::theme::ThemeSlot::Light => self.light_theme_index = index,
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
    /// Whether the *host terminal's* background is dark, as answered by the
    /// OSC 11 probe (`infra::termbg`). Runtime-only, not persisted; drives
    /// `active_theme` when `prefs.theme_auto` is on. Assumed dark, which is
    /// what a terminal that never answers the probe gets.
    pub terminal_is_dark: bool,
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
    /// Press/drag/release state for direct project-row reordering. Geometry
    /// and hit-testing are owned by `ratatui-sectioned-list`.
    pub project_drag: RowDragState,
    /// Grab time while the drag indicators are still *pending*, cleared once
    /// they become visible. So `is_active() && this.is_none()` means "draw the
    /// `↕`/`▸` markers" — see [`AppState::project_drag_indicators`].
    project_drag_pending: Option<Instant>,

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

    /// Backend-provided sidebar section definitions, materialized by the
    /// injected `SystemRegistry`. Layout consumes these values without global
    /// backend lookup; tests may leave the list empty and derive plain fallback
    /// sections from their fixture entries.
    pub system_sections: Vec<crate::system::SectionDef>,

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
            // Mirror `Config::default()` through the single config→prefs
            // mapping rather than re-listing every default here (which had
            // already drifted — e.g. `transparent_bg`). `apply_config` resets
            // this from the loaded config before any read in production; tests
            // that build a bare state override the fields they care about.
            prefs: Prefs::from_config(&crate::config::Config::default(), 0),
            terminal_is_dark: true,
            settings: SettingsState::default(),
            agent_focused: 0,
            summary: SummaryCard::default(),
            dragging_separator: false,
            project_drag: RowDragState::new(),
            project_drag_pending: None,
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
            system_sections: Vec::new(),
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

    /// This host's entry in the mirrored `Config.remotes`, if configured.
    pub fn remote_config(&self, host: &str) -> Option<&crate::config::RemoteConfig> {
        self.config_remotes.iter().find(|r| r.host == host)
    }

    /// Presentation/connection host associated with `lane`, when this is a
    /// configured remote lane. Generic routing uses the lane itself; this
    /// compatibility value is consulted only by tmux/SSH-specific workflows.
    pub fn host_for_lane(&self, lane: &LaneId) -> Option<&str> {
        self.system_sections
            .iter()
            .find(|section| section.lane == *lane)
            .and_then(|section| section.runtime_key.as_deref())
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|entry| entry.lane == *lane)
                    .and_then(|entry| entry.host.as_deref())
            })
            .or_else(|| {
                self.config_remotes
                    .iter()
                    .find(|remote| remote.host == lane.lane())
                    .map(|remote| remote.host.as_str())
            })
    }

    /// The lane attached to Deck's embedded local terminal, if mounted.
    pub fn primary_lane(&self) -> Option<&LaneId> {
        self.system_sections
            .iter()
            .find(|section| section.primary)
            .map(|section| &section.lane)
    }

    pub fn is_primary_lane(&self, lane: &LaneId) -> bool {
        self.primary_lane().is_some_and(|primary| primary == lane)
    }

    pub fn is_primary_entry(&self, entry: &SessionEntry) -> bool {
        self.primary_lane()
            .map_or_else(|| entry.is_local(), |lane| entry.lane == *lane)
    }

    /// Set the reload strip's status and (re)start its TTL.
    pub fn set_reload_status(&mut self, status: ReloadStatus) {
        self.reload_status = Some(status);
        self.reload_status_at = Some(Instant::now());
    }

    /// Surface a transient warning in the sidebar's reload strip. The TUI owns
    /// the alternate screen, so a bare `eprintln!` is wiped invisibly; route
    /// operational warnings here. Reuses the reload toast's auto-expiry
    /// (`RELOAD_STATUS_ERR_TTL`).
    pub fn show_warning(&mut self, msg: impl Into<String>) {
        self.set_reload_status(ReloadStatus::Err(msg.into()));
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
        let cur = option_row(&FRAME_RATE_LIMIT_OPTIONS, self.prefs.frame_rate_limit);
        self.prefs.frame_rate_limit = cycle_option(&FRAME_RATE_LIMIT_OPTIONS, cur, direction).0;
    }

    pub fn cycle_agents_probe_interval(&mut self, direction: i32) {
        let cur = option_row(
            &AGENTS_PROBE_INTERVAL_OPTIONS,
            self.prefs.agents_probe_interval_secs,
        );
        self.prefs.agents_probe_interval_secs =
            cycle_option(&AGENTS_PROBE_INTERVAL_OPTIONS, cur, direction).0;
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

    /// Columns/rows one border edge takes: 1 when borders are shown, else 0.
    pub fn border_inset(&self) -> u16 {
        u16::from(self.prefs.show_borders)
    }

    pub fn pty_size(&self) -> (u16, u16) {
        let bo = self.border_inset() * 2;
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
        let b = self.border_inset() * 2;
        let content_width = match self.effective_layout_mode() {
            LayoutMode::Horizontal => self.prefs.sidebar_width.saturating_sub(b),
            LayoutMode::Vertical => self.term_width.saturating_sub(b),
        };
        crate::geometry::sidebar_footer_height(crate::geometry::banner_visible(
            self.update_available.is_some(),
            content_width,
        ))
    }

    /// Resolve a screen row in the scrollable session area into
    /// `(layout, viewport_y, scroll, visible_height)`. `None` when the row is in
    /// the header banner, footer, or outside the sidebar. Shared by the row and
    /// divider hit-testers so they agree on geometry and the applied scroll offset.
    fn session_row_hit(&self, row: u16) -> Option<(BuiltLayout, u16, u16, u16)> {
        let b = self.border_inset();
        let sidebar_h = match self.effective_layout_mode() {
            LayoutMode::Horizontal => self.term_height,
            LayoutMode::Vertical => self.effective_sidebar_height(),
        };
        let header_height = crate::geometry::SIDEBAR_HEADER_HEIGHT;
        let footer_height = self.sidebar_footer_height();
        let sessions_top = b + header_height;
        let sessions_bottom = sidebar_h.saturating_sub(b + footer_height);
        // The list viewport sits above the Summary card (pinned to the bottom
        // of the Agents tab, between the list and the footer, not part of the
        // sectioned list; `summary_card_height` is 0 elsewhere). `list_bottom`
        // is never past `sessions_bottom`, so this one check covers both.
        let list_bottom = sessions_bottom.saturating_sub(self.summary_card_height());
        if row < sessions_top || row >= list_bottom {
            return None;
        }
        let visible_height = list_bottom - sessions_top;
        let built = self.current_layout(self.prefs.view_mode);
        let scroll = built
            .layout
            .scroll_offset(self.focus_target().map(|f| f.0), visible_height);
        let viewport_y = row - sessions_top;
        Some((built, viewport_y, scroll, visible_height))
    }

    /// Map a screen row to a sidebar focus target. Walks the unified
    /// layout (local cards + remote groups + headers) so variable-
    /// height rows hit-test correctly.
    pub fn focus_at_row(&self, row: u16) -> Option<FocusTarget> {
        let (built, viewport_y, scroll, _) = self.session_row_hit(row)?;
        built.layout.row_at_y(viewport_y, scroll).map(FocusTarget)
    }

    /// Start direct manipulation on the project row under `row`. The drag is
    /// live immediately (so a fast drag still reorders); only its indicators
    /// wait for `PROJECT_DRAG_INDICATOR_DELAY`.
    pub fn start_project_drag(&mut self, row: u16, now: Instant) -> Option<usize> {
        let Some((built, viewport_y, scroll, _)) = self.session_row_hit(row) else {
            self.project_drag.cancel();
            self.project_drag_pending = None;
            return None;
        };
        let hit = self.project_drag.begin(&built.layout, viewport_y, scroll);
        self.project_drag_pending = hit.is_some().then_some(now);
        hit
    }

    /// Track the last valid project row visited by an active drag. Reaching a
    /// different row reveals the indicators right away: the pointer has left
    /// the pressed row, so this is a reorder and not a click.
    pub fn update_project_drag(&mut self, row: u16) -> Option<usize> {
        let Some((built, viewport_y, scroll, _)) = self.session_row_hit(row) else {
            return self.project_drag.target();
        };
        let target = self.project_drag.update(&built.layout, viewport_y, scroll);
        if target != self.project_drag.source() {
            self.project_drag_pending = None;
        }
        target
    }

    /// Source and target rows for the drag indicators, or `None` while no drag
    /// is active or its reveal delay hasn't elapsed.
    pub fn project_drag_indicators(&self) -> Option<(usize, usize)> {
        if self.project_drag_pending.is_some() {
            return None;
        }
        self.project_drag.source().zip(self.project_drag.target())
    }

    /// Reveal the drag indicators once the press has been held long enough.
    /// Returns whether this tick made them appear, so the caller can redraw —
    /// holding still produces no events of its own.
    pub fn tick_project_drag(&mut self, now: Instant) -> bool {
        let Some(pending) = self.project_drag_pending else {
            return false;
        };
        if now.saturating_duration_since(pending) >= PROJECT_DRAG_INDICATOR_DELAY {
            self.project_drag_pending = None;
            return true;
        }
        false
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
            // Returns the `Option<String>` host the mouse layer's collapse keys
            // still speak; becomes a plain `LaneId` once those move over.
            Some(self.host_for_lane(&meta.lane).map(str::to_string))
        } else {
            None
        }
    }

    /// Map a screen column to a tab index in vertical/tabs mode.
    pub fn session_at_col(&self, col: u16, row: u16) -> Option<usize> {
        let b = self.border_inset();
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
        let content_width = self.term_width.saturating_sub(b.saturating_mul(2));
        let layout = tab_bar_layout(&label_refs, self.focused, content_width);
        let local_col = col.saturating_sub(b);
        layout
            .tabs
            .iter()
            .find(|t| (t.start..t.end).contains(&local_col))
            .map(|t| t.index)
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
        self.entries
            .iter()
            .filter(|entry| self.is_primary_entry(entry))
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
    #[cfg(test)]
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

    /// Agents detected in a sidebar section. `None` means the lane has not
    /// been probed yet.
    pub fn section_agents(&self, lane: &LaneId) -> Option<&[crate::agent::DetectedAgent]> {
        self.agents.get(lane.as_str()).map(Vec::as_slice)
    }

    /// Fold a remote refresh round's agent detection into `agents`.
    /// `covered_hosts` = every host queried; `fresh` = per-host result for
    /// hosts whose probe succeeded. A covered host missing from `fresh` had a
    /// failed probe, so its stale list is dropped (else dead pane ids keep
    /// rendering as clickable footer lines). The local `None` key is untouched;
    /// hosts no longer configured are pruned.
    #[cfg(test)]
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
        &crate::theme::THEMES[self.active_theme_index()]
    }

    /// Index of the theme in force: the fixed choice, or — in "follow
    /// terminal" mode — whichever of the dark/light slots matches the probed
    /// terminal background.
    pub fn active_theme_index(&self) -> usize {
        if !self.prefs.theme_auto {
            self.prefs.theme_index
        } else if self.terminal_is_dark {
            self.prefs.dark_theme_index
        } else {
            self.prefs.light_theme_index
        }
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
#[path = "../../../tests/unit/model/state.rs"]
mod tests;

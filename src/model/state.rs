use std::collections::{HashMap, HashSet};
use std::time::Instant;

use ratatui::layout::{Position, Rect};
use ratatui_sectioned_list::ItemKind;
use ratatui_textarea::TextArea;
use serde::{Deserialize, Serialize};

use crate::config::PluginConfig;
use crate::keybindings::Keybindings;
use crate::layout::{card_height, context_menu_rect, tab_col_ranges, tab_label};
use crate::new_session::{make_textarea, textarea_line, NewSessionState};
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

// One list for local and remote rows. "Switch" is dropped — the focus
// already triggers the switch, so the menu item was redundant. On a remote
// row Rename/Kill map to `ssh <host> tmux <cmd>` and Move up/down reorder
// *within the host group* (hosts can't interleave), persisted to that
// server's `@deck_order` — same labels, different backend.
const SESSION_MENU_ITEMS: &[&str] = &["Rename", "Kill", "Move up", "Move down"];
// Items shown but greyed-out / unselectable when the right-clicked row
// is a synthetic placeholder (a remote host with no sessions, or an
// unreachable one): there's no real session to Rename/Kill/reorder —
// i.e. every session item.
const PLACEHOLDER_DISABLED_ITEMS: &[&str] = SESSION_MENU_ITEMS;
// Only Kill is greyed when the row is the last live session on a remote
// host: killing it would tear down that host's tmux server. Rename is
// still fine.
const LAST_REMOTE_SESSION_DISABLED: &[&str] = &["Kill"];
// Host divider [...] menu acts on the whole remote *group*. "Remove
// from list" is equivalent to `deck remote remove <host>`.
const HOST_DIVIDER_MENU_ITEMS: &[&str] = &["New session", "Port Forward", "Remove from list"];
// The `@local` divider reuses the host divider's item list so the menu
// reads consistently, but Port Forward and Remove from list are remote-
// only concepts: they're shown greyed out, leaving just "New session"
// (which creates a local session).
const LOCAL_DIVIDER_DISABLED: &[&str] = &["Port Forward", "Remove from list"];
// Right-click on blank sidebar space. "New session" is intentionally
// absent — creating a local session lives on the `@local` divider's
// `[…]` menu instead.
const GLOBAL_MENU_ITEMS: &[&str] = &[
    "Add Remote Host",
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

/// State of the "Summary" card at the top of the Agents tab. `Idle` shows
/// a "Generate Summary" button; clicking it kicks an async job and flips
/// to `Generating` (an animated placeholder); when the job finishes the
/// generated text lands and it becomes `Ready`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SummaryState {
    #[default]
    Idle,
    Generating,
    Ready {
        text: String,
        /// Unix seconds when the text landed, for the card's "Xm ago" age and
        /// to drive its "Re-generate" affordance.
        generated_at: u64,
    },
    /// Generation failed (no agents, `claude` missing, non-zero exit); the
    /// card shows the reason and the Generate button stays available to retry.
    Error(String),
}

/// Default body height (text rows) of the Agents-tab Summary card. The
/// live value is `AppState::summary_height`, drag-adjustable and persisted.
pub const DEFAULT_SUMMARY_HEIGHT: u16 = 6;
/// Drag-resize bounds for the summary card's body height.
pub const SUMMARY_MIN_HEIGHT: u16 = 2;
pub const SUMMARY_MAX_HEIGHT: u16 = 40;

pub const SETTINGS_ITEM_COUNT: usize = 10;
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

// --- Context menu ---

#[derive(Debug, Clone)]
pub enum MenuKind {
    /// Right-clicked a session row. Local and remote rows share one item
    /// list (`SESSION_MENU_ITEMS`); only the greyed subset is per-row.
    Session {
        focus: FocusTarget,
        /// Subset of the items shown greyed-out and not selectable (e.g.
        /// Rename/Kill on a synthetic placeholder row). Empty for a real
        /// session, where every item is actionable.
        disabled: &'static [&'static str],
    },
    Global,
    /// Click on the `[…]` button on a remote host divider. The items are
    /// the fixed `HOST_DIVIDER_MENU_ITEMS` list (see `items()`).
    HostDivider {
        host: String,
    },
    /// Click on the `[…]` button on the `@local` divider. Shares the host
    /// divider's items, but the remote-only ones (`LOCAL_DIVIDER_DISABLED`)
    /// are greyed out.
    LocalDivider,
}

impl MenuKind {
    pub fn items(&self) -> &'static [&'static str] {
        match self {
            MenuKind::Session { .. } => SESSION_MENU_ITEMS,
            MenuKind::Global => GLOBAL_MENU_ITEMS,
            MenuKind::HostDivider { .. } | MenuKind::LocalDivider => HOST_DIVIDER_MENU_ITEMS,
        }
    }

    /// Items that are shown but greyed-out / unselectable: session menus
    /// carry a per-row set, and the `@local` divider greys the remote-only
    /// items. Other menus have every item enabled.
    pub fn disabled(&self) -> &'static [&'static str] {
        match self {
            MenuKind::Session { disabled, .. } => disabled,
            MenuKind::LocalDivider => LOCAL_DIVIDER_DISABLED,
            MenuKind::Global | MenuKind::HostDivider { .. } => &[],
        }
    }
}

/// Menu items to grey out / disable for a right-clicked row.
///
/// - A synthetic remote placeholder (no sessions / unreachable) isn't a
///   real session, so both Rename and Kill are disabled.
/// - The last live session on a remote host disables Kill — killing it
///   would tear down that host's tmux server. Rename stays available.
/// - Everything else (a local session, or a remote host with more than
///   one session) has every item enabled.
pub fn session_menu_disabled(
    target: &SessionTargetRef<'_>,
    remote_sessions: &[RemoteSessionRow],
) -> &'static [&'static str] {
    match target {
        SessionTargetRef::Remote(row) if !row.is_attachable_session() => PLACEHOLDER_DISABLED_ITEMS,
        SessionTargetRef::Remote(row)
            if attachable_on_host(remote_sessions, &row.host).nth(1).is_none() =>
        {
            LAST_REMOTE_SESSION_DISABLED
        }
        SessionTargetRef::Local(_) | SessionTargetRef::Remote(_) => &[],
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

    pub fn disabled(&self) -> &'static [&'static str] {
        self.kind.disabled()
    }

    /// Whether the item at `idx` is selectable (exists and not disabled).
    pub fn is_enabled(&self, idx: usize) -> bool {
        self.items()
            .get(idx)
            .is_some_and(|label| !self.disabled().contains(label))
    }

    /// First selectable item, used as the initial highlight so it never
    /// starts on a greyed item. Falls back to 0 when every item is
    /// disabled (a fully-greyed menu, e.g. a placeholder remote row).
    pub fn first_enabled(&self) -> usize {
        (0..self.items().len())
            .find(|&i| self.is_enabled(i))
            .unwrap_or(0)
    }

    /// Next selectable item after the current selection, or the current
    /// selection if there's no enabled item below it.
    pub fn next_enabled(&self) -> usize {
        ((self.selected + 1)..self.items().len())
            .find(|&i| self.is_enabled(i))
            .unwrap_or(self.selected)
    }

    /// Previous selectable item before the current selection, or the
    /// current selection if there's no enabled item above it.
    pub fn prev_enabled(&self) -> usize {
        (0..self.selected)
            .rev()
            .find(|&i| self.is_enabled(i))
            .unwrap_or(self.selected)
    }
}

// --- Session data ---

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub name: String,
    pub dir: String,
    pub is_current: bool,
    pub idle_seconds: u64,
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

/// Synthetic row name for a reachable host whose tmux server isn't up
/// (so it has no sessions to attach to). Distinct from `unreachable`:
/// the host responded, it just has nothing running.
pub const REMOTE_NO_SESSIONS_LABEL: &str = "(no sessions)";
/// Synthetic row name for a host deck couldn't reach over ssh.
pub const REMOTE_UNREACHABLE_LABEL: &str = "(unreachable)";

/// Whether `name` collides with a synthetic placeholder-row label. A real
/// session so named would be mistaken for a placeholder (e.g. treated as
/// non-attachable by `is_attachable_session`), so these are reserved and
/// rejected when creating or renaming.
pub fn is_reserved_session_name(name: &str) -> bool {
    name == REMOTE_NO_SESSIONS_LABEL || name == REMOTE_UNREACHABLE_LABEL
}

impl RemoteSessionRow {
    /// Whether deck can attach a PTY to this row. Synthetic status
    /// placeholders — still loading, unreachable, or the "no sessions"
    /// marker for a reachable but server-less host — are not real tmux
    /// sessions. The attach/respawn machinery must skip them; otherwise
    /// it spins forever trying to `tmux attach` a host with nothing to
    /// attach to, leaving the row stuck on "connecting…".
    pub fn is_attachable_session(&self) -> bool {
        !self.loading && !self.unreachable && self.name != REMOTE_NO_SESSIONS_LABEL
    }
}

/// The attachable (real) sessions on `host`, in display order. The one
/// filter behind the last-session-on-host kill policy and the per-host
/// name/order collectors, so the call sites can't drift.
pub fn attachable_on_host<'a>(
    rows: &'a [RemoteSessionRow],
    host: &'a str,
) -> impl Iterator<Item = &'a RemoteSessionRow> {
    rows.iter()
        .filter(move |r| r.host == host && r.is_attachable_session())
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
fn host_status_of(r: &RemoteSessionRow) -> HostStatus {
    if r.unreachable {
        HostStatus::Unreachable
    } else if r.loading {
        HostStatus::Connecting
    } else {
        HostStatus::Connected
    }
}

// --- Port-forward liveness types ---

/// Liveness of a single configured forward, refreshed each probe tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardHealth {
    /// Not yet probed this session, enumeration was unavailable, or the host
    /// is still connecting.
    Probing,
    /// `-L`/`-D`: a local listener is present on the listen port. `-R`: the
    /// host connection is up (the remote listener can't be confirmed locally,
    /// so it simply tracks reachability).
    Up,
    /// `-L`/`-D`: no local listener. `-R`: the host is unreachable.
    Down,
}

/// Stable identity of a configured forward, used to key liveness across config
/// reloads and reorders. A local listen port is unique per host, but `mode` and
/// `bind_addr` are included so an `-L` and an `-R` sharing a port number (one
/// local, one remote) don't collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForwardKey {
    pub host: String,
    pub mode: crate::config::ForwardMode,
    pub bind_addr: Option<String>,
    pub listen_port: u16,
}

impl ForwardKey {
    pub fn from_spec(host: &str, spec: &crate::config::ForwardSpec) -> Self {
        Self {
            host: host.to_string(),
            mode: spec.mode,
            bind_addr: spec.bind_addr.clone(),
            listen_port: spec.listen_port,
        }
    }
}

/// Per-host port-forward badge shown on the sidebar divider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PfBadge {
    pub count: usize,
    pub color: PfBadgeColor,
}

/// Rolled-up health color for a host's forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PfBadgeColor {
    /// All forwards Up → green.
    Healthy,
    /// At least one Down → pink.
    Degraded,
    /// At least one Probing, none Down → yellow.
    Probing,
}

/// Roll a host's per-forward healths into one badge color. `Down` dominates,
/// then `Probing`, else `Healthy` (all `Up`).
pub fn rollup_color(healths: &[ForwardHealth]) -> PfBadgeColor {
    if healths.contains(&ForwardHealth::Down) {
        PfBadgeColor::Degraded
    } else if healths.contains(&ForwardHealth::Probing) {
        PfBadgeColor::Probing
    } else {
        PfBadgeColor::Healthy
    }
}

/// Per-item data carried in the sidebar's `SectionedList`. Headers and
/// session rows live in the same flat list so the renderer, scroll
/// logic, and mouse hit-test all walk the same items in lockstep.
#[derive(Debug, Clone)]
pub enum SidebarItemData {
    /// The `@local` group divider, shown above the local session rows in
    /// Expanded view. Carries no data — the renderer draws a fixed label
    /// and a single `[…]` menu button.
    LocalHeader,
    /// Remote host name, plus the 0-based index of the host among
    /// distinct remote hosts in render order — used to cycle the
    /// divider accent color. The renderer formats the `@host` label and
    /// reuses the bare host for the divider's click target.
    Header {
        host: String,
        host_idx: usize,
        status: HostStatus,
        pf: Option<PfBadge>,
    },
    /// A session row at the given flat index — matches the
    /// `FocusTarget` numbering: local rows first, then remotes. The
    /// renderer pairs this index with a `&[&dyn SidebarSession]` slice
    /// built in render order; storage routing happens via
    /// `AppState::session_target` in the action layer.
    Session { session_idx: usize },
    /// A focusable agent row in the Agents tab. `row_idx` indexes into
    /// `AppState::agent_rows()` (local agents first, then remote hosts in
    /// section order), matching the `FocusTarget` numbering for that tab
    /// so a click/keyboard move maps straight back to the agent.
    Agent { row_idx: usize },
    /// Non-selectable placeholder under a section divider in the Agents
    /// tab when that section has no agents. `detecting` = the section
    /// hasn't been probed yet (shows "detecting…" vs "no agents").
    AgentsPlaceholder { detecting: bool },
    /// Non-selectable blank row. Used in the Agents tab to set off each
    /// remote `@host` section with a leading gap.
    Spacer,
    /// The "Summary" card pinned at the top of the Agents tab: a title,
    /// then a "Generate Summary" button / animated placeholder / the
    /// generated text depending on `AppState::summary`.
    SummaryCard,
    /// Non-selectable placeholder under `@local` when the local tmux server
    /// has no sessions. Kept out of `FocusTarget` numbering so remote flat
    /// indices still start at `filtered.len()`.
    LocalEmpty,
}

/// Sidebar layout — built on top of `ratatui_sectioned_list::SectionedList`
/// so geometry, focus-driven scroll, and mouse hit-testing are shared
/// across the renderer and the action layer.
pub type SidebarLayout = ratatui_sectioned_list::SectionedList<SidebarItemData>;

/// One focusable agent row in the Agents tab, in display order: local
/// agents first, then each remote host's agents in section order. The
/// renderer and the `Agent { row_idx }` layout items both index into the
/// `Vec` this produces (`AppState::agent_rows`), so they can't disagree
/// about which agent a row points at.
#[derive(Debug, Clone)]
pub struct AgentRow {
    pub host: Option<String>,
    pub agent: crate::agent::DetectedAgent,
}

/// Which button on a divider a `DividerHit` targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DividerButton {
    /// `[⟳]` — force-refresh (reconnect) the host.
    Reconnect,
    /// `[…]` — open the host-divider menu.
    More,
    /// `⇄N` — the port-forward liveness badge; opens the port-forward overlay.
    PfBadge,
    /// `[…]` on the `@local` divider — opens the local-divider menu. Carries
    /// no host (the `DividerHit.host` is unused for this kind).
    LocalMore,
}

/// Click-region for one button (`[⟳]` or `[…]`) on a remote-host
/// divider. The sidebar renderer fills `divider_hits` after each render;
/// mouse hit-testing consults it before `focus_at_row()`.
#[derive(Debug, Clone)]
pub struct DividerHit {
    pub host: String,
    pub rect: Rect,
    pub kind: DividerButton,
}

/// A detected agent's switch target, keyed by host the usual way
/// (`None` = local). `pane_id` is the stable `%N` handle used to focus
/// the exact pane; `session` is the `switch-client` target (which only
/// renames — not renumbers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTarget {
    pub host: Option<String>,
    pub session: String,
    pub pane_id: String,
}

/// Click-region for one agent line in a section footer. The sidebar
/// renderer fills `agent_hits` after each render; a left click in `rect`
/// switches to (and focuses) that agent's pane.
#[derive(Debug, Clone)]
pub struct AgentHit {
    pub rect: Rect,
    pub target: AgentTarget,
}

/// Click-regions for the two buttons in the kill-confirmation prompt.
/// The sidebar renderer fills `kill_confirm_hits` while the prompt is
/// shown; mouse hit-testing maps a click in `yes`/`no` to confirm/cancel.
#[derive(Debug, Clone, Copy)]
pub struct KillConfirmHits {
    pub yes: Rect,
    pub no: Rect,
}

// --- Side effects ---

#[derive(Debug)]
pub enum Effect {
    SwitchSession(String),
    /// Switch the main view to a remote session. Carries (host, name)
    /// — App's dispatch layer routes the `tmux switch-client` over ssh.
    SwitchRemote(RemoteSwitchRequest),
    /// Focus a detected agent's pane (Agents tab Enter / number jump).
    /// App's dispatch layer routes this exactly like an agent-row click.
    SwitchAgentPane(AgentTarget),
    /// Show a remote host placeholder in the main pane. Used for
    /// synthetic rows like "(no sessions)" that are focusable but don't
    /// have a tmux session to attach to.
    ShowRemotePlaceholder(String),
    KillSession(KillRequest),
    RenameSession(RenameRequest),
    /// Create a new tmux session with `req.name` at `req.dir`.
    CreateSession(CreateSessionRequest),
    /// Detach a remote host from deck (equivalent to `deck remote remove <host>`).
    RemoveRemoteHost(String),
    OpenNewSessionPicker,
    OpenRemoteNewSessionPicker(String),
    OpenAddRemotePicker,
    AddRemoteHost(String),
    RereadNewSessionEntries,
    ResizePty {
        /// Clear the host terminal before the next draw after resize.
        full_redraw: bool,
    },
    SaveConfig,
    SaveSessionOrder,
    SaveRemoteSessionOrder(String),
    ApplyTmuxTheme,
    RefreshSessions,
    Quit,
}

#[derive(Debug, Default)]
pub struct SideEffect {
    effects: Vec<Effect>,
}

/// Generate `SideEffect` push-helpers from a `method(args) => Effect`
/// table; each body is `self.push(<effect>)`.
macro_rules! effect_pushers {
    ($(
        $(#[$meta:meta])*
        $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) => $build:expr ;
    )*) => {
        $(
            $(#[$meta])*
            pub fn $name(&mut self, $($arg: $ty),*) {
                self.push($build);
            }
        )*
    };
}

impl SideEffect {
    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }

    pub fn merge(&mut self, other: SideEffect) {
        self.effects.extend(other.effects);
    }

    pub fn push(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    effect_pushers! {
        switch_session(name: String) => Effect::SwitchSession(name);
        switch_remote(req: RemoteSwitchRequest) => Effect::SwitchRemote(req);
        switch_agent_pane(target: AgentTarget) => Effect::SwitchAgentPane(target);
        show_remote_placeholder(host: String) => Effect::ShowRemotePlaceholder(host);
        kill_session(req: KillRequest) => Effect::KillSession(req);
        rename_session(req: RenameRequest) => Effect::RenameSession(req);
        create_session(req: CreateSessionRequest) => Effect::CreateSession(req);
        remove_remote_host(host: String) => Effect::RemoveRemoteHost(host);
        open_new_session_picker() => Effect::OpenNewSessionPicker;
        open_remote_new_session_picker(host: String) => Effect::OpenRemoteNewSessionPicker(host);
        open_add_remote_picker() => Effect::OpenAddRemotePicker;
        add_remote_host(host: String) => Effect::AddRemoteHost(host);
        reread_new_session_entries() => Effect::RereadNewSessionEntries;
        resize_pty(full_redraw: bool) => Effect::ResizePty { full_redraw };
        save_config() => Effect::SaveConfig;
        save_session_order() => Effect::SaveSessionOrder;
        save_remote_session_order(host: String) => Effect::SaveRemoteSessionOrder(host);
        apply_tmux_theme() => Effect::ApplyTmuxTheme;
        refresh_sessions() => Effect::RefreshSessions;
        quit() => Effect::Quit;
    }

    pub fn has_quit(&self) -> bool {
        self.effects
            .iter()
            .any(|effect| matches!(effect, Effect::Quit))
    }
}

/// Test-only accessors returning the first matching variant's payload:
/// `=> &str` as `&str`, `=> &Ty` by reference.
#[cfg(test)]
macro_rules! effect_finders {
    ($( $name:ident : $variant:ident => &str );* $(;)?) => {
        $(
            pub fn $name(&self) -> Option<&str> {
                self.effects.iter().find_map(|effect| match effect {
                    Effect::$variant(v) => Some(v.as_str()),
                    _ => None,
                })
            }
        )*
    };
    ($( $name:ident : $variant:ident => &$ty:ty );* $(;)?) => {
        $(
            pub fn $name(&self) -> Option<&$ty> {
                self.effects.iter().find_map(|effect| match effect {
                    Effect::$variant(v) => Some(v),
                    _ => None,
                })
            }
        )*
    };
}

/// Test-only `bool` predicates: each row maps a method to an `Effect`
/// pattern checked with `matches!`.
#[cfg(test)]
macro_rules! effect_predicates {
    ($( $name:ident => $pat:pat ),* $(,)?) => {
        $(
            pub fn $name(&self) -> bool {
                self.effects.iter().any(|effect| matches!(effect, $pat))
            }
        )*
    };
}

#[cfg(test)]
impl SideEffect {
    effect_finders! {
        first_switch_session: SwitchSession => &str;
        first_remote_placeholder: ShowRemotePlaceholder => &str;
        first_remove_remote_host: RemoveRemoteHost => &str;
        first_save_remote_session_order: SaveRemoteSessionOrder => &str;
        first_open_remote_new_session_picker: OpenRemoteNewSessionPicker => &str;
    }

    effect_finders! {
        first_kill_session: KillSession => &KillRequest;
        first_rename_session: RenameSession => &RenameRequest;
    }

    effect_predicates! {
        has_open_new_session_picker => Effect::OpenNewSessionPicker,
        has_resize_pty => Effect::ResizePty { .. },
        has_full_redraw_after_resize => Effect::ResizePty { full_redraw: true },
        has_save_config => Effect::SaveConfig,
        has_save_session_order => Effect::SaveSessionOrder,
        has_refresh_sessions => Effect::RefreshSessions,
        has_reread_new_session_entries => Effect::RereadNewSessionEntries,
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
    /// `Some(host)` creates the session on that remote host over ssh;
    /// `None` creates it on the local tmux server.
    pub host: Option<String>,
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
        textarea_line(&self.input)
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
            PfFormError::ListenPortRange => "Listen port must be a number from 0 to 65535.",
            PfFormError::TargetPortRange => "Target port must be a number from 0 to 65535.",
            PfFormError::TargetHostRequired => "Target host is required for -L and -R forwards.",
        }
    }
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

#[derive(Debug, Clone)]
pub struct PortForwardOverlay {
    pub host: String,
    pub selected: usize,
    pub add_form: Option<PfAddForm>,
    pub status: Option<String>,
}

// --- Warning overlay state ---

/// Modal warning banner shown over the main pane. Used by the
/// self-update flow to surface "can't self-update from here" /
/// "unsupported platform" messages. Lives on `App` (as
/// `warning_state: Option<WarningState>`) rather than in `OverlayState`
/// because the dispatch loop's "block actions while a warning is up"
/// gate reads it from App directly.
#[derive(Clone)]
pub enum WarningState {
    Proactive { text: &'static str, detail: String },
}

// --- Overlay state ---

/// UI state for transient sidebar overlays — help screen, kill-confirm
/// prompt, in-progress rename, right-click context menu, and the
/// exclude-pattern editor popup. Grouped so the renderer and key
/// dispatcher have a single place to ask "is any overlay active?".
#[derive(Debug, Default)]
pub struct OverlayState {
    pub show_help: bool,
    pub confirm_kill: bool,
    pub renaming: Option<RenameState>,
    pub context_menu: Option<ContextMenu>,
    pub exclude_editor: Option<ExcludeEditorState>,
    pub new_session: Option<NewSessionState>,
    pub add_remote: Option<crate::add_remote::AddRemoteState>,
    /// Port-forward overlay for a single host. See `PortForwardOverlay`.
    pub port_forward: Option<PortForwardOverlay>,
    /// The Agents-tab summary "big view" popup is open.
    pub summary_popup: bool,
    /// Settings input box for the generated-summary language (free text).
    pub summary_lang_input: Option<TextArea<'static>>,
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
    pub frame_rate_limit: u16,
    /// How often the Agents tab probes (seconds). Drives the refresh cadence
    /// while that tab is active; see `App`'s run loop.
    pub agents_probe_interval_secs: u64,
    pub show_borders: bool,
    /// Active sidebar tab. `Projects` lists tmux sessions; `Agents` lists
    /// detected agents as the primary list. Persisted to config. Agent
    /// detection in the refresh worker runs only while this is `Agents`
    /// (see `agents_tab_active`).
    pub sidebar_tab: SidebarTab,
    /// Cursor row for the Agents tab, kept separate from `focused` (the
    /// Projects cursor) so switching tabs preserves each one's position.
    /// Indexes into `agent_rows()`.
    pub agent_focused: usize,
    /// State of the Agents-tab "Summary" card (idle / generating / ready).
    pub summary: SummaryState,
    /// The summary prompt template (from config), `{{SESSIONS}}` filled
    /// with the agent panes at generation time. Seeded in `App::new` and
    /// refreshed on config reload.
    pub summary_prompt: String,
    /// Model passed to `claude --model` for the summary (from config); empty
    /// follows the user's Claude Code default.
    pub summary_model: String,
    /// Language the summary is asked to use (from config); empty = default.
    pub summary_language: String,
    /// Body height (text rows) of the inline summary card, drag-adjustable
    /// from its bottom edge and persisted to config.
    pub summary_height: u16,
    /// Click-region of the card's "Generate Summary" button, captured each
    /// frame for mouse hit-testing. `None` when the button isn't shown
    /// (not on the Agents tab, or while generating).
    pub summary_button_rect: Option<Rect>,
    /// Click-region of the card's "popup" button (open the big view),
    /// captured each frame. `None` unless the summary is `Ready`.
    pub summary_popup_button_rect: Option<Rect>,
    /// True while dragging the card's bottom edge to resize it.
    pub dragging_summary: bool,
    /// Scroll offset (in wrapped text rows) of the Ready summary's content,
    /// when it overflows the card's fixed text area.
    pub summary_scroll: usize,
    /// Max scroll offset for the current Ready text at the current width,
    /// captured by the renderer each frame so scroll input can clamp.
    pub summary_max_scroll: usize,
    /// Scroll offset of the summary popup's text, and its captured max.
    pub summary_popup_scroll: usize,
    pub summary_popup_max_scroll: usize,
    /// The card's full rect, captured each frame so the mouse layer can
    /// route wheel events over it to scrolling the summary.
    pub summary_card_rect: Option<Rect>,
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
    /// Click-region of the footer's "menu" button, captured during render
    /// for mouse hit-testing. Opens the global context menu.
    pub menu_button_bounds: Option<Rect>,

    /// Click-regions of the `Projects` / `Agents` tab labels in the
    /// sidebar header, captured during render for mouse hit-testing.
    /// `None` in layouts without the header (vertical/tabs layout).
    pub projects_tab_rect: Option<Rect>,
    pub agents_tab_rect: Option<Rect>,

    /// Result of the most recent manual config reload. Rendered in the
    /// sidebar footer and auto-cleared by the main loop after a short
    /// TTL — see `RELOAD_STATUS_OK_TTL` / `RELOAD_STATUS_ERR_TTL`.
    pub reload_status: Option<ReloadStatus>,
    pub reload_status_at: Option<Instant>,

    /// Click-regions for divider `[…]` buttons, refilled by the sidebar
    /// renderer each frame. Read by mouse dispatch.
    pub divider_hits: Vec<DividerHit>,

    /// Click-regions for agent rows in the Agents tab, refilled by the
    /// sidebar renderer each frame. A left click switches to that pane.
    pub agent_hits: Vec<AgentHit>,

    /// The agent deck last switched to (via an agent-line click). Its
    /// footer line renders highlighted as "you are here". Identified by
    /// `(host, pane_id)` so the highlight is uniform for local and remote
    /// — never branches on origin. Cleared by any non-agent switch.
    pub active_agent: Option<AgentTarget>,

    /// Click-regions for the kill-confirmation `[No]` / `[Yes]` buttons,
    /// refilled by the sidebar renderer while the prompt is up.
    /// Read by mouse dispatch. `None` when the prompt is not shown.
    pub kill_confirm_hits: Option<KillConfirmHits>,

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
    pub agents: HashMap<Option<String>, Vec<crate::agent::DetectedAgent>>,

    /// Sidebar groups the user has collapsed (Expanded view only). `None`
    /// is the `@local` group; `Some(host)` is a remote `@host` group. A
    /// collapsed group renders as just its divider — its session rows are
    /// hidden by the layout. Persisted to config (`collapsed_sections`)
    /// and restored at startup.
    pub collapsed_sections: HashSet<Option<String>>,
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
            sessions: Vec::new(),
            filtered: Vec::new(),
            focused: 0,
            current_session: String::new(),
            session_order: Vec::new(),
            remote_sessions: Vec::new(),
            main_view: MainView::Terminal,
            focus_mode: FocusMode::Main,
            theme_index: 0,
            settings: SettingsState::default(),
            layout_mode: LayoutMode::default(),
            view_mode: ViewMode::default(),
            sidebar_width: 28,
            sidebar_height: SIDEBAR_HEIGHT,
            frame_rate_limit: 5,
            agents_probe_interval_secs: DEFAULT_AGENTS_PROBE_INTERVAL,
            show_borders: true,
            sidebar_tab: SidebarTab::default(),
            agent_focused: 0,
            summary: SummaryState::Idle,
            summary_prompt: String::new(),
            summary_model: String::new(),
            summary_language: String::new(),
            summary_height: DEFAULT_SUMMARY_HEIGHT,
            summary_button_rect: None,
            summary_popup_button_rect: None,
            dragging_summary: false,
            summary_scroll: 0,
            summary_popup_scroll: 0,
            summary_popup_max_scroll: 0,
            summary_max_scroll: 0,
            summary_card_rect: None,
            dragging_separator: false,
            overlay: OverlayState::default(),
            term_width,
            term_height,
            last_scroll: Instant::now(),
            exclude_patterns: Vec::new(),
            plugins: Vec::new(),
            keybindings: Keybindings::default(),
            update_check_mode: UpdateCheckMode::default(),
            update_available: None,
            update_last_checked_secs: None,
            banner_upgrade_bounds: None,
            menu_button_bounds: None,
            projects_tab_rect: None,
            agents_tab_rect: None,
            reload_status: None,
            reload_status_at: None,
            divider_hits: Vec::new(),
            kill_confirm_hits: None,
            config_remotes: Vec::new(),
            forward_health: HashMap::new(),
            agents: HashMap::new(),
            agent_hits: Vec::new(),
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
        self.theme_index = theme_index;
        self.layout_mode = cfg.layout;
        self.show_borders = cfg.show_borders;
        self.sidebar_tab = cfg.sidebar_tab;
        self.view_mode = cfg.view_mode;
        self.frame_rate_limit = normalize_frame_rate_limit(cfg.frame_rate_limit);
        self.sidebar_width = cfg.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        self.sidebar_height = cfg.sidebar_height;
        self.exclude_patterns = cfg.exclude_patterns.clone();
        self.plugins = cfg.plugins.clone();
        self.keybindings = keybindings;
        self.update_check_mode = cfg.update_check;
        self.summary_prompt = cfg.summary_prompt.clone();
        self.summary_model = cfg.summary_model.clone();
        self.summary_language = cfg.summary_language.clone();
        self.agents_probe_interval_secs =
            normalize_agents_probe_interval(cfg.agents_probe_interval);
        self.set_summary_height(cfg.summary_height);
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
        self.frame_rate_limit = cycle_option(
            &FRAME_RATE_LIMIT_OPTIONS,
            normalize_frame_rate_limit(self.frame_rate_limit),
            direction,
        );
    }

    pub fn cycle_agents_probe_interval(&mut self, direction: i32) {
        self.agents_probe_interval_secs = cycle_option(
            &AGENTS_PROBE_INTERVAL_OPTIONS,
            normalize_agents_probe_interval(self.agents_probe_interval_secs),
            direction,
        );
    }

    pub fn banner_upgrade_at(&self, col: u16, row: u16) -> bool {
        let pos = Position::new(col, row);
        self.banner_upgrade_bounds.is_some_and(|r| r.contains(pos))
    }

    /// Which sidebar tab label `(col, row)` falls on in the header, if
    /// any. Used by mouse dispatch to switch tabs on a click.
    pub fn tab_at(&self, col: u16, row: u16) -> Option<SidebarTab> {
        let pos = Position::new(col, row);
        let hit = |rect: Option<Rect>| rect.is_some_and(|r| r.contains(pos));
        if hit(self.projects_tab_rect) {
            Some(SidebarTab::Projects)
        } else if hit(self.agents_tab_rect) {
            Some(SidebarTab::Agents)
        } else {
            None
        }
    }

    /// Whether the Agents tab is the active sidebar view. The tab selector
    /// only exists in the Horizontal layout (the Vertical layout is a
    /// session tab-bar with no header), so the Agents view is gated to
    /// Horizontal — everything stays the Projects view in Vertical even if
    /// `sidebar_tab` happens to be `Agents`. Gates agent detection in the
    /// refresh worker and selects the agents layout / focus space.
    pub fn agents_tab_active(&self) -> bool {
        self.sidebar_tab == SidebarTab::Agents && self.layout_mode == LayoutMode::Horizontal
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
        if self.layout_mode == LayoutMode::Vertical {
            return if self.show_borders { 3 } else { 1 };
        }
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

    /// Height of the sidebar footer in rows, for mouse hit-testing. The
    /// formula itself lives in `crate::layout::sidebar_footer_height`,
    /// shared with the renderer (`ui::sidebar::draw_sidebar`) so the two
    /// can't drift — when they did, the bottom visible session row was
    /// click-dead.
    pub fn sidebar_footer_height(&self) -> u16 {
        let b = if self.show_borders { 2u16 } else { 0 };
        let content_width = match self.layout_mode {
            LayoutMode::Horizontal => self.sidebar_width.saturating_sub(b),
            LayoutMode::Vertical => self.term_width.saturating_sub(b),
        };
        crate::layout::sidebar_footer_height(
            crate::layout::banner_visible(self.update_available.is_some(), content_width),
            self.plugins.len(),
        )
    }

    /// Resolve a screen row inside the sidebar's scrollable session area
    /// into `(layout, viewport_y, scroll, visible_height)`. `None` when
    /// the row falls in the header banner, the footer, or outside the
    /// sidebar. Shared by the row and divider hit-testers so they agree
    /// on geometry and the scroll offset the renderer applied.
    fn session_row_hit(&self, row: u16) -> Option<(SidebarLayout, u16, u16, u16)> {
        let b = if self.show_borders { 1u16 } else { 0 };
        let sidebar_h = match self.layout_mode {
            LayoutMode::Horizontal => self.term_height,
            LayoutMode::Vertical => self.effective_sidebar_height(),
        };
        let header_height = crate::layout::SIDEBAR_HEADER_HEIGHT;
        let footer_height = self.sidebar_footer_height();
        let sessions_top = b + header_height;
        let sessions_bottom = sidebar_h.saturating_sub(b + footer_height);
        if row < sessions_top || row >= sessions_bottom {
            return None;
        }
        let visible_height = sessions_bottom - sessions_top;
        let layout = self.current_layout(self.view_mode);
        let scroll = layout.scroll_offset(self.focus_target().map(|f| f.0), visible_height);
        let viewport_y = row - sessions_top;
        Some((layout, viewport_y, scroll, visible_height))
    }

    /// Map a screen row to a sidebar focus target. Walks the unified
    /// layout (local cards + remote groups + headers) so variable-
    /// height rows hit-test correctly.
    pub fn focus_at_row(&self, row: u16) -> Option<FocusTarget> {
        let (layout, viewport_y, scroll, _) = self.session_row_hit(row)?;
        layout.row_at_y(viewport_y, scroll).map(FocusTarget)
    }

    /// Whether `row` falls on a group divider header (`@local` / `@host`).
    /// Used to swallow right-clicks on dividers — their actions live on
    /// the divider's own `[…]` button, not a context menu.
    pub fn is_divider_at_row(&self, row: u16) -> bool {
        let Some((layout, viewport_y, scroll, visible_height)) = self.session_row_hit(row) else {
            return false;
        };
        // Bind the result before the block ends so the `VisibleIter`
        // (which borrows `layout`) drops before `layout` does — its Drop
        // impl in ratatui-sectioned-list 0.1.1 otherwise outlives the borrow.
        let hit = layout.visible_items(scroll, visible_height).any(|v| {
            v.item.kind == ItemKind::Header
                && viewport_y >= v.viewport_y
                && viewport_y < v.viewport_y + v.visible_height
        });
        hit
    }

    /// Map a screen row on a group divider to that group's section key
    /// (`None` = `@local`, `Some(host)` = a remote `@host`). Returns
    /// `None` when the row isn't on a group divider, or lands on an
    /// agent-count footer (which isn't a collapse target). Used by the
    /// mouse layer to toggle a group when its divider is clicked.
    pub fn divider_section_key_at(&self, row: u16) -> Option<Option<String>> {
        let (layout, viewport_y, scroll, _) = self.session_row_hit(row)?;
        // header_at_y returns the 0-based header section index; resolve it
        // back to its item to read the SidebarItemData. AgentCount footers
        // are headers too, so we explicitly reject them.
        let section_idx = layout.header_at_y(viewport_y, scroll)?;
        let item = layout
            .items()
            .iter()
            .filter(|it| it.kind == ItemKind::Header)
            .nth(section_idx)?;
        match &item.data {
            SidebarItemData::LocalHeader => Some(None),
            SidebarItemData::Header { host, .. } => Some(Some(host.clone())),
            // AgentCount (or anything else) isn't a collapse target.
            _ => None,
        }
    }

    /// Map a screen column to a tab index in vertical/tabs mode.
    pub fn session_at_col(&self, col: u16, row: u16) -> Option<usize> {
        let b = if self.show_borders { 1u16 } else { 0 };
        if row != b {
            return None;
        }
        // Build labels in the same flat order the tab renderer walks —
        // local rows first, then remotes as `host:session` — so a hit
        // maps straight to a `FocusTarget` flat index.
        let mut labels: Vec<String> =
            Vec::with_capacity(self.filtered.len() + self.remote_sessions.len());
        for &i in &self.filtered {
            labels.push(tab_label(None, &self.sessions[i].name));
        }
        for r in &self.remote_sessions {
            labels.push(tab_label(Some(&r.host), &r.name));
        }
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

    /// Total number of focusable rows in the active sidebar tab. Projects:
    /// local sessions (after filtering) followed by remote sessions.
    /// Agents: the flattened agent list.
    pub fn focusable_count(&self) -> usize {
        if self.agents_tab_active() {
            self.agent_rows().len()
        } else {
            self.filtered.len() + self.remote_sessions.len()
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
        match host {
            None => self
                .filtered
                .iter()
                .position(|&i| self.sessions[i].name == session),
            Some(h) => self
                .remote_sessions
                .iter()
                .position(|r| r.host == h && r.name == session)
                .map(|p| self.filtered.len() + p),
        }
    }

    /// Optimistically mark a host's rows as reconnecting so the sidebar
    /// shows "(connecting...)" the instant the user hits the divider's
    /// reconnect button, before the refresh round returns.
    pub fn mark_host_reconnecting(&mut self, host: &str) {
        for row in &mut self.remote_sessions {
            if row.host == host {
                row.loading = true;
                row.unreachable = false;
            }
        }
    }

    /// Agents detected in a sidebar section, addressed uniformly by host
    /// (`None` = local). `None` result = not probed yet. The layout uses
    /// this without caring whether the section is local or remote.
    pub fn section_agents(&self, host: Option<&str>) -> Option<&[crate::agent::DetectedAgent]> {
        self.agents
            .get(&host.map(str::to_string))
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
                self.agents.remove(&Some(host));
            }
        }
        for (host, list) in fresh {
            self.agents.insert(Some(host), list);
        }
        let configured: std::collections::HashSet<&str> = self
            .config_remotes
            .iter()
            .map(|r| r.host.as_str())
            .collect();
        self.agents
            .retain(|k, _| k.as_deref().is_none_or(|h| configured.contains(h)));
    }

    /// Build the unified sidebar layout: a flat list of header /
    /// session items in render order. Renderers and the mouse
    /// hit-tester share this so they can't disagree about which row
    /// lives where.
    pub fn sidebar_layout(&self, view_mode: ViewMode) -> SidebarLayout {
        let card_h = card_height(view_mode) as u16;
        let mut layout = SidebarLayout::new();
        // Group dividers (`@local`, `@host`) are an Expanded-view
        // adornment; Compact rows already carry an origin prefix.
        let show_headers = matches!(view_mode, ViewMode::Expanded);

        // Collapse is an Expanded-view feature. `section_idx` counts every
        // pushed header (group dividers *and* agent footers) in push order,
        // matching the crate's section numbering. We track only the GROUP
        // headers' (section_idx, key) so we can flip their collapsed flag
        // after the list is built. A collapsed group's footer is skipped
        // below so the divider sits alone; its rows are hidden by the widget.
        layout.set_collapsible(true);
        let mut header_count: usize = 0;
        let mut group_headers: Vec<(usize, Option<String>)> = Vec::new();
        let is_collapsed = |key: &Option<String>| self.collapsed_sections.contains(key);

        // Local group: an `@local` divider over the local rows, matching
        // the remote `@host` dividers. Flat index for a local row equals
        // its filtered_pos regardless of the header (headers aren't rows).
        // Keep the divider even when local has no sessions so the user can
        // still open the local section menu and see the empty-state row.
        if show_headers {
            group_headers.push((header_count, None));
            header_count += 1;
            layout.push_header(SidebarItemData::LocalHeader, 1);
        }
        for pos in 0..self.filtered.len() {
            layout.push_row(SidebarItemData::Session { session_idx: pos }, card_h);
        }
        if show_headers && self.filtered.is_empty() && !is_collapsed(&None) {
            layout.push_header(SidebarItemData::LocalEmpty, card_h);
            header_count += 1;
        }

        // Remote groups: detect host transitions in render order
        // (which matches focus order — `remote_sessions` is already
        // grouped by host because the refresh worker emits hosts in
        // config order, one block at a time). Flat index for a remote
        // row is filtered.len() + remote_idx. Each group gets an
        // `@host` divider above and an agent-count footer below.
        let local_count = self.filtered.len();
        let mut host_idx: usize = 0;
        let mut prev_host: Option<&str> = None;
        for (remote_idx, r) in self.remote_sessions.iter().enumerate() {
            let new_host = Some(r.host.as_str()) != prev_host;
            if new_host {
                if prev_host.is_some() {
                    host_idx += 1;
                }
                if show_headers {
                    // A host's rows are contiguous and share a status; this
                    // row is the first of the group, so it represents it.
                    let status = host_status_of(r);
                    group_headers.push((header_count, Some(r.host.clone())));
                    header_count += 1;
                    layout.push_header(
                        SidebarItemData::Header {
                            host: r.host.clone(),
                            host_idx,
                            status,
                            pf: self.host_pf_badge(&r.host),
                        },
                        1,
                    );
                }
                prev_host = Some(r.host.as_str());
            }
            // Match the local card height so groups visually align
            // (3 rows in Expanded, 1 in Compact). The renderer pads
            // the bottom of each row with blank lines to fill the card.
            layout.push_row(
                SidebarItemData::Session {
                    session_idx: local_count + remote_idx,
                },
                card_h,
            );
        }

        // Flip each group header's collapsed flag so the widget hides its
        // rows and the geometry/scroll/hit-test all honor the collapse.
        for (section_idx, key) in group_headers {
            layout.set_collapsed(section_idx, self.collapsed_sections.contains(&key));
        }

        layout
    }

    /// Distinct remote hosts in the order their rows first appear in
    /// `remote_sessions` (the refresh worker emits hosts in config order,
    /// one contiguous block each). Shared by `agent_rows` and
    /// `agents_layout` so both walk sections identically.
    fn remote_hosts_in_order(&self) -> Vec<String> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut hosts = Vec::new();
        for r in &self.remote_sessions {
            if seen.insert(r.host.as_str()) {
                hosts.push(r.host.clone());
            }
        }
        hosts
    }

    /// The flat list of detected agents for the Agents tab, in display
    /// order: local agents first, then each remote host's agents in
    /// section order. `Agent { row_idx }` items and the renderer index
    /// into this, so its order is the Agents-tab `FocusTarget` numbering.
    pub fn agent_rows(&self) -> Vec<AgentRow> {
        let mut rows = Vec::new();
        if let Some(list) = self.section_agents(None) {
            for agent in list {
                rows.push(AgentRow {
                    host: None,
                    agent: agent.clone(),
                });
            }
        }
        for host in self.remote_hosts_in_order() {
            if let Some(list) = self.section_agents(Some(&host)) {
                for agent in list {
                    rows.push(AgentRow {
                        host: Some(host.clone()),
                        agent: agent.clone(),
                    });
                }
            }
        }
        rows
    }

    /// Flat focusable index of the agent row matching `target`, or `None`
    /// if it isn't currently listed. Lets the Agents-tab cursor track the
    /// pane switched to via a click, the way `focusable_index_for` does
    /// for the Projects tab.
    pub fn agent_row_index_for(&self, target: &AgentTarget) -> Option<usize> {
        self.agent_rows()
            .iter()
            .position(|row| row.host == target.host && row.agent.pane_id == target.pane_id)
    }

    /// Row height the Summary card reserves: title + blank + a
    /// `summary_height` body area + a drag-handle row. A fixed-size window
    /// for every state, so overflowing Ready text scrolls inside it rather
    /// than growing the card; the user resizes it by dragging the handle.
    pub fn summary_card_height(&self) -> u16 {
        3 + self.summary_height
    }

    /// Set the card body height (rows), clamped to the drag-resize bounds.
    /// Returns whether it changed.
    pub fn set_summary_height(&mut self, rows: u16) -> bool {
        let clamped = rows.clamp(SUMMARY_MIN_HEIGHT, SUMMARY_MAX_HEIGHT);
        if clamped != self.summary_height {
            self.summary_height = clamped;
            true
        } else {
            false
        }
    }

    /// Whether `(col, row)` falls on the Summary card's "Generate" button.
    pub fn summary_button_at(&self, col: u16, row: u16) -> bool {
        let pos = Position::new(col, row);
        self.summary_button_rect.is_some_and(|r| r.contains(pos))
    }

    /// Whether `(col, row)` falls on the Summary card's "popup" button.
    pub fn summary_popup_button_at(&self, col: u16, row: u16) -> bool {
        let pos = Position::new(col, row);
        self.summary_popup_button_rect
            .is_some_and(|r| r.contains(pos))
    }

    /// Whether `(col, row)` falls anywhere on the Summary card — used to
    /// route wheel events to scrolling its text.
    pub fn summary_card_at(&self, col: u16, row: u16) -> bool {
        let pos = Position::new(col, row);
        self.summary_card_rect.is_some_and(|r| r.contains(pos))
    }

    /// Whether `(col, row)` is on the card's bottom drag-handle row.
    pub fn summary_resize_at(&self, col: u16, row: u16) -> bool {
        self.summary_card_rect.is_some_and(|r| {
            let handle_y = r.y + r.height.saturating_sub(1);
            row == handle_y && col >= r.x && col < r.x + r.width
        })
    }

    /// New body height implied by dragging the handle to `row` — the rows
    /// between the card top and the pointer, minus the title/blank/handle
    /// chrome. Clamped by `set_summary_height`.
    pub fn summary_height_for_drag(&self, row: u16) -> u16 {
        let top = self.summary_card_rect.map_or(0, |r| r.y);
        // total = row - top + 1; body rows = total - 3 (title, blank, handle).
        row.saturating_sub(top).saturating_sub(2)
    }

    /// Apply a wheel/keyboard scroll delta to the Summary text, clamped to
    /// the captured max offset.
    pub fn scroll_summary(&mut self, delta: i32) {
        let max = self.summary_max_scroll as i32;
        self.summary_scroll = (self.summary_scroll as i32 + delta).clamp(0, max) as usize;
    }

    /// Apply a scroll delta to the summary popup, clamped to its max.
    pub fn scroll_summary_popup(&mut self, delta: i32) {
        let max = self.summary_popup_max_scroll as i32;
        self.summary_popup_scroll =
            (self.summary_popup_scroll as i32 + delta).clamp(0, max) as usize;
    }

    /// Build the Agents-tab layout: an `@local` / `@host` divider per
    /// section (in `agent_rows` order) with that section's agents as
    /// focusable rows beneath it, or a non-focusable placeholder when a
    /// section has no agents. `row_idx` on each `Agent` item matches the
    /// `agent_rows()` position so focus/scroll/hit-test stay in sync.
    pub fn agents_layout(&self) -> SidebarLayout {
        let mut layout = SidebarLayout::new();
        // No collapse on the Agents tab — sections are informational and
        // always expanded, so the focus index maps straight to a row.
        layout.set_collapsible(false);
        let mut row_idx = 0usize;

        let mut push_section = |layout: &mut SidebarLayout, host: Option<&str>| {
            match self.section_agents(host) {
                Some(list) if !list.is_empty() => {
                    for _ in list {
                        layout.push_row(SidebarItemData::Agent { row_idx }, 1);
                        row_idx += 1;
                    }
                }
                Some(_) => {
                    layout.push_header(
                        SidebarItemData::AgentsPlaceholder { detecting: false },
                        1,
                    );
                }
                None => {
                    layout.push_header(SidebarItemData::AgentsPlaceholder { detecting: true }, 1);
                }
            }
        };

        // The Summary card is pinned at the very top, above `@local`.
        layout.push_header(SidebarItemData::SummaryCard, self.summary_card_height());

        layout.push_header(SidebarItemData::LocalHeader, 1);
        push_section(&mut layout, None);

        for (host_idx, host) in self.remote_hosts_in_order().into_iter().enumerate() {
            // A blank row sets each remote section off from what's above.
            // Local stays flush at the top (no leading gap).
            layout.push_header(SidebarItemData::Spacer, 1);
            let status = self.host_conn_status(&host).unwrap_or(HostStatus::Connecting);
            layout.push_header(
                SidebarItemData::Header {
                    host: host.clone(),
                    host_idx,
                    status,
                    pf: self.host_pf_badge(&host),
                },
                1,
            );
            push_section(&mut layout, Some(&host));
        }

        layout
    }

    /// The layout for the active sidebar tab. Projects → the session
    /// list; Agents → the agent list. Callers (renderer, hit-testers,
    /// scroll) use this so they all see the same rows for the active tab.
    pub fn current_layout(&self, view_mode: ViewMode) -> SidebarLayout {
        if self.agents_tab_active() {
            self.agents_layout()
        } else {
            self.sidebar_layout(view_mode)
        }
    }

    /// The port-forward badge for a host's divider, or `None` when the host has
    /// no forwards. Color rolls up the per-forward health; count is the number
    /// of configured forwards.
    pub fn host_pf_badge(&self, host: &str) -> Option<PfBadge> {
        let forwards = self
            .config_remotes
            .iter()
            .find(|r| r.host == host)
            .map(|r| r.forwards.as_slice())?;
        if forwards.is_empty() {
            return None;
        }
        let healths: Vec<ForwardHealth> = forwards
            .iter()
            .map(|f| {
                self.forward_health
                    .get(&ForwardKey::from_spec(host, f))
                    .copied()
                    .unwrap_or(ForwardHealth::Probing)
            })
            .collect();
        Some(PfBadge {
            count: forwards.len(),
            color: rollup_color(&healths),
        })
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
        self.remote_sessions
            .iter()
            .find(|r| r.host == host)
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

    /// Focused remote placeholder row, if any. These rows occupy normal
    /// focus slots so users can land on a host with no attachable session,
    /// but the main pane must render an explicit status instead of a stale
    /// terminal screen.
    pub fn focused_remote_placeholder(&self) -> Option<&RemoteSessionRow> {
        if self.agents_tab_active() {
            return None;
        }
        match self.session_target(self.focus_target()?)? {
            SessionTargetRef::Remote(row) if !row.is_attachable_session() => Some(row),
            SessionTargetRef::Local(_) | SessionTargetRef::Remote(_) => None,
        }
    }

    /// Section key of the group the flat focus index `idx` lives in:
    /// `None` for a local row (`idx < filtered.len()`), `Some(host)` for a
    /// remote one. Used by the section-toggle keybinding and focus-skip
    /// logic. For an out-of-range index this falls back to `None`.
    pub fn section_key_of_focus(&self, idx: usize) -> Option<String> {
        let local_count = self.filtered.len();
        if idx < local_count {
            None
        } else {
            self.remote_sessions
                .get(idx - local_count)
                .map(|r| r.host.clone())
        }
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
            && self
                .collapsed_sections
                .contains(&self.section_key_of_focus(idx))
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
            host: row.host,
            session: row.agent.session,
            pane_id: row.agent.pane_id,
        })
    }

    /// Name to show in the kill-confirmation overlay: the focused row's
    /// session name, or `None` when no kill is pending or focus has no
    /// valid target. The renderer gates the overlay on this being `Some`.
    ///
    /// Resolves through `session_target` so a focused *remote* row reports
    /// its name too — a raw `filtered[focused]` lookup only covers local
    /// rows, leaving remote kills with no name so the overlay never drew
    /// (issue #41).
    pub fn confirm_kill_name(&self) -> Option<String> {
        if !self.overlay.confirm_kill {
            return None;
        }
        match self.session_target(self.focus_target()?)? {
            SessionTargetRef::Local(row) => Some(row.name.clone()),
            SessionTargetRef::Remote(row) => Some(row.name.clone()),
        }
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
    pub fn kill_blocked_reason(&self, target: &SessionTargetRef<'_>) -> Option<&'static str> {
        match target {
            SessionTargetRef::Remote(row) if !row.is_attachable_session() => {
                Some("no session to kill")
            }
            SessionTargetRef::Remote(row)
                if attachable_on_host(&self.remote_sessions, &row.host)
                    .nth(1)
                    .is_none() =>
            {
                Some("last session on host")
            }
            SessionTargetRef::Local(_) if self.sessions.len() <= 1 => Some("last local session"),
            SessionTargetRef::Local(_) | SessionTargetRef::Remote(_) => None,
        }
    }

    /// Whether the focused kill `target` may be killed. See
    /// [`AppState::kill_blocked_reason`].
    pub fn can_kill(&self, target: &SessionTargetRef<'_>) -> bool {
        self.kill_blocked_reason(target).is_none()
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

    // --- Filtering and ordering ---

    pub fn recompute_filter(&mut self) {
        self.filtered = (0..self.sessions.len()).collect();
        // Clamp focused to the unified focusable range (local + remote).
        // Without remotes this collapses to the original local-only
        // behavior; with remotes it keeps focus inside the remote
        // section after the local list shrinks.
        // Clamp against the Projects row space specifically (not the
        // tab-aware `focusable_count`, which would use the agent count
        // when the Agents tab is active and corrupt the Projects cursor).
        let total = self.filtered.len() + self.remote_sessions.len();
        if total > 0 && self.focused >= total {
            self.focused = total - 1;
        }
    }

    /// Keep the Agents-tab cursor inside the current agent list after the
    /// detected agents change (agents come and go between refresh rounds).
    pub fn clamp_agent_focus(&mut self) {
        let total = self.agent_rows().len();
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
            .map(|row| (row.host, row.agent.pane_id))
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
                .position(|row| row.host == host && row.agent.pane_id == pane_id)
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

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui_sectioned_list::ItemKind;
use ratatui_textarea::TextArea;
use serde::{Deserialize, Serialize};

use crate::config::PluginConfig;
use crate::keybindings::Keybindings;
use crate::layout::{
    card_height, context_menu_width, plugin_block_rows, tab_col_ranges, tab_label,
    BANNER_MIN_WIDTH,
};
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

// "Switch" is dropped — the focus already triggers the switch, so the
// menu item was redundant.
const SESSION_MENU_ITEMS: &[&str] = &["Rename", "Kill", "Move up", "Move down"];
// Remote sessions live on a different tmux server. Rename/Kill map to
// `ssh <host> tmux <cmd>`; Move up/down reorder *within the host group*
// (hosts can't interleave), persisted to that server's `@deck_order`.
const REMOTE_SESSION_MENU_ITEMS: &[&str] = &["Rename", "Kill", "Move up", "Move down"];
// Items shown but greyed-out / unselectable when the right-clicked row
// is a synthetic placeholder (a remote host with no sessions, or an
// unreachable one): there's no real session to Rename/Kill/reorder.
const PLACEHOLDER_DISABLED_ITEMS: &[&str] = &["Rename", "Kill", "Move up", "Move down"];
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

pub const SETTINGS_ITEM_COUNT: usize = 7;

/// Blank rows appended under each section's agent-count footer, as a gap
/// before the next section. The footer item is `1 + this` rows tall;
/// `sidebar_layout` (height) and the renderer (blank lines) both use it.
pub const AGENT_FOOTER_GAP_ROWS: u16 = 2;

// --- Context menu ---

#[derive(Debug, Clone)]
pub enum MenuKind {
    /// Right-clicked a session row. `items` is decided at construction
    /// (e.g. local rows include `Move up/down`, remotes don't) so the
    /// reducer doesn't have to redo that lookup on every keypress.
    Session {
        focus: FocusTarget,
        items: &'static [&'static str],
        /// Subset of `items` shown greyed-out and not selectable (e.g.
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
            MenuKind::Session { items, .. } => items,
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

/// Menu items shown after right-clicking a session row. The action
/// layer reads `session_target` to decide which list applies; the
/// renderer never needs to know.
pub fn session_menu_items(target: &SessionTargetRef<'_>) -> &'static [&'static str] {
    match target {
        SessionTargetRef::Local(_) => SESSION_MENU_ITEMS,
        SessionTargetRef::Remote(_) => REMOTE_SESSION_MENU_ITEMS,
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
            if remote_sessions
                .iter()
                .filter(|r| r.host == row.host && r.is_attachable_session())
                .count()
                <= 1 =>
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
    /// Non-selectable footer at the bottom of a section: a `claude X,
    /// codex Y` count line, then one line per agent's session/window/pane
    /// (each clickable → switch). `host` keys the section (`None` = local)
    /// so a clicked line knows where to switch. `agents` is `None` while
    /// the section hasn't been probed yet ("claude …, codex …").
    AgentCount {
        host: Option<String>,
        agents: Option<Vec<crate::agent::DetectedAgent>>,
    },
}

/// Sidebar layout — built on top of `ratatui_sectioned_list::SectionedList`
/// so geometry, focus-driven scroll, and mouse hit-testing are shared
/// across the renderer and the action layer.
pub type SidebarLayout = ratatui_sectioned_list::SectionedList<SidebarItemData>;

/// Push a section's non-selectable agent footer: a count line, one line
/// per located agent, then a `AGENT_FOOTER_GAP_ROWS` gap. The layout
/// height must match what the renderer draws, so it's computed from the
/// agent count here. `None` agents = not probed yet (just the "…" line).
fn push_agent_footer(
    layout: &mut SidebarLayout,
    host: Option<String>,
    agents: Option<Vec<crate::agent::DetectedAgent>>,
) {
    let n = agents.as_ref().map_or(0, Vec::len) as u16;
    layout.push_header(
        SidebarItemData::AgentCount { host, agents },
        1 + n + AGENT_FOOTER_GAP_ROWS,
    );
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
    /// the `@local` divider's "New session" item; uses the focused
    /// session's dir as the picker's starting point.
    pub open_new_session_picker: bool,
    /// Dispatch should open the new-session picker targeting this remote
    /// host: the dir browser lists remote directories over ssh and the
    /// session is created on that host. Fired by the host divider menu's
    /// "New session" item.
    pub open_remote_new_session_picker: Option<String>,
    /// Dispatch should open the Add Remote Host picker (build candidates from
    /// ~/.ssh/config minus already-added hosts).
    pub open_add_remote_picker: bool,
    /// A host was just added; dispatch should onboard it (spawn connection),
    /// the same way `reload_config` does for a newly-configured host.
    pub add_remote_host: Option<String>,
    /// Dispatch should re-run `read_dir` for the picker's current
    /// parent and refresh `entries`. Fired by any reducer arm that
    /// changes the effective parent.
    pub reread_new_session_entries: bool,
    pub resize_pty: bool,
    pub save_config: bool,
    /// Dispatch should persist the current local `session_order` onto the
    /// tmux sessions (`@deck_order`) so the manual arrangement survives a
    /// deck restart. Fired by `ReorderSession`.
    pub save_session_order: bool,
    /// Dispatch should persist this host's remote session order onto its
    /// tmux server (`@deck_order` over ssh). Fired by `ReorderSession`
    /// when the moved row is remote; carries the host whose group changed.
    pub save_remote_session_order: Option<String>,
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
        if other.open_remote_new_session_picker.is_some() {
            self.open_remote_new_session_picker = other.open_remote_new_session_picker;
        }
        self.open_add_remote_picker |= other.open_add_remote_picker;
        if other.add_remote_host.is_some() {
            self.add_remote_host = other.add_remote_host;
        }
        self.reread_new_session_entries |= other.reread_new_session_entries;
        self.resize_pty |= other.resize_pty;
        self.save_config |= other.save_config;
        self.save_session_order |= other.save_session_order;
        if other.save_remote_session_order.is_some() {
            self.save_remote_session_order = other.save_remote_session_order;
        }
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
    /// Whether the per-section agent footers render and agent detection
    /// runs. Toggled by the "Show Agents" checkbox in the sidebar header;
    /// persisted to config. When false, `sidebar_layout` omits the
    /// footers and the refresh worker skips agent detection entirely.
    pub show_agents: bool,
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

    /// Click-region of the "Show Agents" checkbox in the sidebar header,
    /// captured during render for mouse hit-testing. `None` in layouts
    /// without the header (tabs mode).
    pub show_agents_checkbox: Option<Rect>,

    /// Result of the most recent manual config reload. Rendered in the
    /// sidebar footer and auto-cleared by the main loop after a short
    /// TTL — see `RELOAD_STATUS_OK_TTL` / `RELOAD_STATUS_ERR_TTL`.
    pub reload_status: Option<ReloadStatus>,
    pub reload_status_at: Option<Instant>,

    /// Click-regions for divider `[…]` buttons, refilled by the sidebar
    /// renderer each frame. Read by mouse dispatch.
    pub divider_hits: Vec<DividerHit>,

    /// Click-regions for agent footer lines, refilled by the sidebar
    /// renderer each frame. A left click switches to that agent's pane.
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
    /// hidden by the layout and its agent footer is omitted. Persisted to
    /// config (`collapsed_sections`) and restored at startup.
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        theme_index: usize,
        layout_mode: LayoutMode,
        view_mode: ViewMode,
        show_borders: bool,
        show_agents: bool,
        sidebar_width: u16,
        sidebar_height: u16,
        term_width: u16,
        term_height: u16,
        exclude_patterns: Vec<String>,
        plugins: Vec<PluginConfig>,
        keybindings: Keybindings,
        update_check_mode: UpdateCheckMode,
        collapsed_sections: HashSet<Option<String>>,
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
            show_agents,
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
            show_agents_checkbox: None,
            reload_status: None,
            reload_status_at: None,
            divider_hits: Vec::new(),
            kill_confirm_hits: None,
            config_remotes: Vec::new(),
            forward_health: HashMap::new(),
            agents: HashMap::new(),
            agent_hits: Vec::new(),
            active_agent: None,
            collapsed_sections,
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

    /// Whether `(col, row)` falls on the header's "Show Agents" checkbox.
    pub fn show_agents_checkbox_at(&self, col: u16, row: u16) -> bool {
        match self.show_agents_checkbox {
            Some(r) => col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height,
            None => false,
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
        let header_height = 2u16;
        let footer_height = self.sidebar_footer_height();
        let sessions_top = b + header_height;
        let sessions_bottom = sidebar_h.saturating_sub(b + footer_height);
        if row < sessions_top || row >= sessions_bottom {
            return None;
        }
        let visible_height = sessions_bottom - sessions_top;
        let layout = self.sidebar_layout(self.view_mode);
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

    /// Total number of focusable rows in the sidebar: local sessions
    /// (after filtering) followed by remote sessions.
    pub fn focusable_count(&self) -> usize {
        self.filtered.len() + self.remote_sessions.len()
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
    pub fn section_agents(&self, host: Option<&str>) -> Option<Vec<crate::agent::DetectedAgent>> {
        self.agents.get(&host.map(str::to_string)).cloned()
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
        let configured: std::collections::HashSet<&str> =
            self.config_remotes.iter().map(|r| r.host.as_str()).collect();
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
        if show_headers && !self.filtered.is_empty() {
            group_headers.push((header_count, None));
            header_count += 1;
            layout.push_header(SidebarItemData::LocalHeader, 1);
        }
        for pos in 0..self.filtered.len() {
            layout.push_row(SidebarItemData::Session { session_idx: pos }, card_h);
        }
        // Footer line under the local section: detected agent counts.
        // Non-focusable (a header), so it can't be selected. Skipped
        // entirely when the user turned agents off, or when the group is
        // collapsed (a collapsed group shows just its divider).
        if show_headers && self.show_agents && !self.filtered.is_empty() && !is_collapsed(&None) {
            push_agent_footer(&mut layout, None, self.section_agents(None));
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
                if let Some(ph) = prev_host {
                    // Close the previous host's section with its footer,
                    // unless that group is collapsed (divider stands alone).
                    if show_headers && self.show_agents && !is_collapsed(&Some(ph.to_string())) {
                        push_agent_footer(
                            &mut layout,
                            Some(ph.to_string()),
                            self.section_agents(Some(ph)),
                        );
                        header_count += 1;
                    }
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
        // Footer for the last remote host group (skipped if collapsed).
        if show_headers && self.show_agents {
            if let Some(ph) = prev_host {
                if !is_collapsed(&Some(ph.to_string())) {
                    push_agent_footer(
                        &mut layout,
                        Some(ph.to_string()),
                        self.section_agents(Some(ph)),
                    );
                }
            }
        }

        // Flip each group header's collapsed flag so the widget hides its
        // rows and the geometry/scroll/hit-test all honor the collapse.
        for (section_idx, key) in group_headers {
            layout.set_collapsed(section_idx, self.collapsed_sections.contains(&key));
        }

        layout
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
        idx < self.focusable_count()
            && self
                .collapsed_sections
                .contains(&self.section_key_of_focus(idx))
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

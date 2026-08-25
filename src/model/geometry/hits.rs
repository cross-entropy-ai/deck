//! Every clickable region a frame publishes, and the one resolver that turns
//! a `(col, row)` into what it landed on.
//!
//! The sidebar provides the base set and the active modal adds its own; the
//! render loop captures the pair whole and hands it to mouse dispatch. Match
//! order in `HitRegions::hit` is the hit-test priority, so it lives in one
//! place and cannot drift between callers.

use ratatui::layout::{Position, Rect};

use crate::lane::LaneId;
use crate::state::SidebarTab;

/// One focusable entry in the Agents-tab list, the twin of `SessionEntry`,
/// in display order (local section first, then each remote host). Renderer
/// and layout both index into the `Vec` it produces
/// (`AppState::agent_entries`), so they agree on which entry points where.
/// Its [`kind`](AgentEntry::kind) is a detected `Agent` or a synthetic
/// `Placeholder` for an empty section (mirroring `SessionEntryKind`'s `Live`
/// vs `NoSessions`/`Unreachable`); both are focusable and occupy a flat-index
/// slot, so entries, count, layout, and focus walk the same sequence and
/// activating a placeholder is a guarded no-op. Owns its data (no lifetime)
/// at the cost of a per-entry `DetectedAgent` clone per `agent_entries()`
/// run — the lists are small, so the symmetry is worth the copies.
#[derive(Debug, Clone)]
pub struct AgentEntry {
    pub lane: LaneId,
    pub kind: AgentEntryKind,
}

impl AgentEntry {
    /// The detected agent this entry points at, or `None` for a placeholder.
    /// Lets the renderer / focus paths treat real agents and placeholders
    /// uniformly while only switching to (and counting) the real ones.
    pub fn agent(&self) -> Option<&crate::agent::DetectedAgent> {
        match &self.kind {
            AgentEntryKind::Agent(agent) => Some(agent),
            AgentEntryKind::Placeholder { .. } => None,
        }
    }
}

/// What an [`AgentEntry`] is — the twin of `SessionEntryKind`: a real detected
/// agent, or the inert placeholder shown for a section with no agents.
#[derive(Debug, Clone)]
pub enum AgentEntryKind {
    /// A detected agent — the switch target.
    Agent(crate::agent::DetectedAgent),
    /// An empty section's placeholder. `probed` = `true` once detection ran
    /// and came back empty (`no agents`), `false` while the first probe is
    /// pending (`detecting…`). Not switchable.
    Placeholder { probed: bool },
}

/// Click-region for one divider button. The sidebar renderer fills
/// `HitRegions.dividers` after each render; mouse hit-testing resolves it
/// before `focus_at_row()`, then dispatches the button's system `command` on
/// its `lane`.
#[derive(Debug, Clone)]
pub struct DividerHit {
    pub lane: LaneId,
    pub rect: Rect,
    /// The backend-defined action id (see [`SectionButton::action`]).
    pub action: crate::system::LaneActionId,
}

/// A detected agent's switch target, keyed by its mounted backend lane.
/// `pane_id` is the stable `%N` handle that focuses the exact pane;
/// `session` is the `switch-client` target (renames, doesn't renumber).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTarget {
    pub lane: LaneId,
    pub session: String,
    pub pane_id: String,
}

/// Click-region for one agent line in a section footer. The sidebar
/// renderer fills `HitRegions.agents` after each render; a left click in
/// `rect` switches to (and focuses) that agent's pane.
#[derive(Debug, Clone)]
pub struct AgentHit {
    pub rect: Rect,
    pub target: AgentTarget,
}

/// Click-regions for the two buttons in the kill-confirmation prompt.
/// The sidebar renderer fills `HitRegions.kill` while the prompt is shown;
/// mouse hit-testing maps a click in `yes`/`no` to confirm/cancel.
#[derive(Debug, Clone, Copy)]
pub struct KillConfirmHits {
    pub yes: Rect,
    pub no: Rect,
}

/// Click rects for the two sidebar tab labels (`Projects` / `Agents`),
/// published by the header renderer so mouse dispatch can switch tabs.
/// Clamped to the header area so a narrow sidebar can't leak a click
/// target into the PTY pane (bug #16).
#[derive(Debug, Clone, Copy)]
pub struct TabRects {
    pub projects: Rect,
    pub agents: Rect,
}

/// Click/scroll regions the Agents-tab Summary card publishes each frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct SummaryHits {
    /// The "Generate" button, for click hit-testing.
    pub button: Option<Rect>,
    /// The "popup" (big view) button; `None` unless the summary is Ready.
    pub popup: Option<Rect>,
    /// The card's full rect, for routing wheel events to text scrolling.
    pub card: Option<Rect>,
    /// Overflow the renderer measured for the Ready text at this width
    /// (0 = it fits). Carried out with the rects, then copied onto the card by
    /// the render loop, the same way the popup's bound is.
    pub max_scroll: usize,
}

impl SummaryHits {
    /// Whether `pos` falls anywhere on the Summary card. Used by the wheel path
    /// to route scroll to the card text. Checked directly, not via
    /// `HitRegions::hit` priority: the card rect spans the whole Agents-tab
    /// viewport, and the rows/dividers over it outrank it for *clicks* but not
    /// the wheel.
    pub fn card_at(&self, col: u16, row: u16) -> bool {
        let pos = Position::new(col, row);
        self.card.is_some_and(|r| r.contains(pos))
    }

    /// Whether `(col, row)` is on the card's top drag-handle row. The card
    /// is pinned to the bottom, so its top edge is the resize boundary.
    pub fn resize_at(&self, col: u16, row: u16) -> bool {
        self.card
            .is_some_and(|r| row == r.y && col >= r.x && col < r.x + r.width)
    }

    /// New body height implied by dragging the top handle to `row`. The card
    /// bottom is anchored to the footer, so dragging the top up grows the card:
    /// `body = (card_bottom - row) - chrome` (chrome = handle, title, blank).
    /// Clamped by `set_summary_height`.
    pub fn height_for_drag(&self, row: u16) -> u16 {
        let bottom = self.card.map_or(0, |r| r.y + r.height);
        bottom.saturating_sub(row).saturating_sub(3)
    }
}

/// The add-remote picker's click targets: one per visible host, plus the
/// footer hints that double as its buttons.
#[derive(Debug, Clone, Default)]
pub struct AddRemoteHits {
    /// Visible host rows, each carrying its index in the filtered list.
    pub hosts: Vec<ListItemHit>,
    pub add: Option<Rect>,
    pub cancel: Option<Rect>,
}

/// The hidden-session restore picker's click targets.
#[derive(Debug, Clone, Default)]
pub struct HiddenHits {
    /// Visible name rows, each carrying its index in the filtered list.
    pub rows: Vec<ListItemHit>,
    pub restore_all: Option<Rect>,
    pub cancel: Option<Rect>,
}

/// The mount picker's click targets: one per visible candidate, plus the
/// footer hints that double as its buttons.
#[derive(Debug, Clone, Default)]
pub struct MountHits {
    /// Visible candidate rows, each carrying its index in the filtered list.
    pub rows: Vec<ListItemHit>,
    pub mount: Option<Rect>,
    pub sort: Option<Rect>,
    pub cancel: Option<Rect>,
}

/// The port-forward list's click targets. `None` for every button while the
/// add form is open, since the list is not on screen then.
#[derive(Debug, Clone, Default)]
pub struct PfHits {
    /// Visible forward rows, each carrying its index in the forward list.
    pub rows: Vec<ListItemHit>,
    pub add: Option<Rect>,
    pub delete: Option<Rect>,
    pub close: Option<Rect>,
}

/// Find the row `pos` lands on, as an index into the list that produced it.
fn row_at(rows: &[ListItemHit], pos: Position) -> Option<usize> {
    rows.iter()
        .find(|row| row.rect.contains(pos))
        .map(|row| row.index)
}

/// Every picker-shaped overlay publishes the same thing: a list of rows, then
/// a few footer hints that double as buttons. Their `hit` walks differ only in
/// which [`HitKind`] each part lands on, so declare that mapping once instead
/// of writing the same walk per picker. Row hits win over buttons, matching the
/// order the renderers paint in.
macro_rules! picker_hits {
    ($(
        $ty:ident { $rows:ident => $row_kind:ident $(, $button:ident => $kind:ident)* $(,)? }
    )*) => {
        $(impl $ty {
            fn hit(&self, pos: Position) -> Option<HitKind> {
                if let Some(index) = row_at(&self.$rows, pos) {
                    return Some(HitKind::$row_kind(index));
                }
                $(
                    if self.$button.is_some_and(|rect| rect.contains(pos)) {
                        return Some(HitKind::$kind);
                    }
                )*
                None
            }
        })*
    };
}

picker_hits! {
    AddRemoteHits { hosts => AddRemoteHost, add => AddRemoteAdd, cancel => AddRemoteCancel }
    HiddenHits { rows => HiddenRow, restore_all => HiddenRestoreAll, cancel => HiddenCancel }
    MountHits { rows => MountRow, mount => MountConfirm, sort => MountSort, cancel => MountCancel }
    PfHits { rows => PfRow, add => PfAdd, delete => PfDelete, close => PfClose }
}

/// Every clickable region published for one frame. The sidebar provides the
/// base set and the active modal can add its own rows; `AppState` stores the
/// combined registry. `HitRegions::hit` is the single resolver mouse dispatch
/// consults, so hit-test priority lives in one place. Sidebar rects are
/// clamped to its content area before modal regions are added.
#[derive(Debug, Clone, Default)]
pub struct HitRegions {
    /// The footer banner's clickable "upgrade" span.
    pub banner: Option<Rect>,
    /// Divider `[⟳]` / `[…]` / pf-badge buttons.
    pub dividers: Vec<DividerHit>,
    /// The kill-confirmation `[No]` / `[Yes]` buttons, while shown.
    pub kill: Option<KillConfirmHits>,
    /// Agent rows in the Agents tab.
    pub agents: Vec<AgentHit>,
    /// The `Projects` / `Agents` header tab labels (`None` in tabs mode,
    /// which has no header).
    pub tabs: Option<TabRects>,
    /// Visible directory rows in the active new-session picker. Each carries
    /// its absolute selection index in the filtered list.
    pub new_session_dirs: Vec<ListItemHit>,
    /// The new-session picker's footer `⏎ create` hint, which doubles as the
    /// modal's confirm button for the mouse.
    pub new_session_create: Option<Rect>,
    /// Rows and buttons of the active add-remote picker.
    pub add_remote: AddRemoteHits,
    /// Rows and buttons of the active mount picker.
    pub mounts: MountHits,
    /// Rows and buttons of the active hidden-session restore picker.
    pub hidden: HiddenHits,
    /// Rows and buttons of the active port-forward list.
    pub port_forward: PfHits,
    /// The expanded header's collapse button, or the collapsed rail's expand
    /// button.
    pub sidebar_toggle: Option<Rect>,
    /// The Summary card's buttons/card/scroll bound.
    pub summary: SummaryHits,
    /// The footer's "menu" button.
    pub menu: Option<Rect>,
}

/// What a `(col, row)` click resolves to among the frame's rect-based regions.
/// Vec hits are carried by index so the caller reads the matched data from the
/// registry, keeping hit data in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    /// The kill-confirmation `[Yes]` button.
    KillYes,
    /// The kill-confirmation `[No]` button.
    KillNo,
    /// The footer banner's "upgrade" span.
    Banner,
    /// A header tab label; carries which tab.
    Tab(SidebarTab),
    /// A visible directory row in the new-session picker.
    NewSessionDir(usize),
    /// The new-session picker's footer confirm hint.
    NewSessionCreate,
    /// A visible host row in the add-remote picker.
    AddRemoteHost(usize),
    /// The add-remote picker's footer `[Enter] Add` hint.
    AddRemoteAdd,
    /// The add-remote picker's footer `[Esc] Cancel` hint.
    AddRemoteCancel,
    /// A visible name row in the hidden-session restore picker.
    HiddenRow(usize),
    /// The restore picker's footer `^A all` hint.
    HiddenRestoreAll,
    /// The restore picker's footer `⎋ cancel` hint.
    HiddenCancel,
    /// A visible candidate row in the mount picker.
    MountRow(usize),
    /// The mount picker's footer `⏎ mount` hint.
    MountConfirm,
    /// The mount picker's footer `⇥ sort` hint.
    MountSort,
    /// The mount picker's footer `⎋ cancel` hint.
    MountCancel,
    /// A visible forward row in the port-forward list.
    PfRow(usize),
    /// The port-forward list's `[A] Add` hint.
    PfAdd,
    /// The port-forward list's `[D] Delete` hint.
    PfDelete,
    /// The port-forward list's `[Esc] Close` hint.
    PfClose,
    /// Collapse or expand the whole horizontal sidebar.
    SidebarToggle,
    /// The Summary card's "Generate" button.
    SummaryButton,
    /// The Summary card's "popup" (big view) button.
    SummaryPopup,
    /// The footer "menu" button; carries its rect (mouse anchors the menu
    /// at its x/y).
    Menu(Rect),
    /// A divider button; carries an index into `HitRegions.dividers`.
    Divider(usize),
    /// An agent row; carries an index into `HitRegions.agents`.
    Agent(usize),
}

impl HitRegions {
    /// Resolve a click at `(col, row)` to the region it lands on. Match
    /// order encodes priority: active-modal regions first, then banner, tabs,
    /// summary buttons, menu, dividers, and agent rows.
    pub fn hit(&self, col: u16, row: u16) -> Option<HitKind> {
        let pos = Position::new(col, row);
        if let Some(kill) = self.kill {
            if kill.yes.contains(pos) {
                return Some(HitKind::KillYes);
            }
            if kill.no.contains(pos) {
                return Some(HitKind::KillNo);
            }
        }
        if let Some(item) = self
            .new_session_dirs
            .iter()
            .find(|item| item.rect.contains(pos))
        {
            return Some(HitKind::NewSessionDir(item.index));
        }
        if self.new_session_create.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::NewSessionCreate);
        }
        if let Some(hit) = self.add_remote.hit(pos) {
            return Some(hit);
        }
        if let Some(hit) = self.mounts.hit(pos) {
            return Some(hit);
        }
        if let Some(hit) = self.hidden.hit(pos) {
            return Some(hit);
        }
        if let Some(hit) = self.port_forward.hit(pos) {
            return Some(hit);
        }
        if self.banner.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::Banner);
        }
        if let Some(tabs) = self.tabs {
            if tabs.projects.contains(pos) {
                return Some(HitKind::Tab(SidebarTab::Projects));
            }
            if tabs.agents.contains(pos) {
                return Some(HitKind::Tab(SidebarTab::Agents));
            }
        }
        if self.sidebar_toggle.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::SidebarToggle);
        }
        if self.summary.button.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::SummaryButton);
        }
        if self.summary.popup.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::SummaryPopup);
        }
        if let Some(r) = self.menu {
            if r.contains(pos) {
                return Some(HitKind::Menu(r));
            }
        }
        if let Some(i) = self.dividers.iter().position(|h| h.rect.contains(pos)) {
            return Some(HitKind::Divider(i));
        }
        if let Some(i) = self.agents.iter().position(|h| h.rect.contains(pos)) {
            return Some(HitKind::Agent(i));
        }
        None
    }
}

/// One visible row in a windowed modal list, mapped back to its absolute
/// selection index so mouse input follows the same filtered list as keyboard
/// navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListItemHit {
    pub rect: Rect,
    pub index: usize,
}

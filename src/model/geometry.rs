//! Sidebar / hit-region geometry plus pure layout helpers shared by the UI
//! (drawing) and model/action (hit-testing) layers. They live in `model`
//! (not `ui`), so `ui` and `app` depend on the model for geometry, never the
//! reverse. The tab bar's geometry (leading/inner pad, separator) is the
//! single source of truth here: renderer and hit-tester both read it, so
//! changing tab width keeps click-target math in sync.

use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;

use crate::lane::LaneId;

use unicode_width::UnicodeWidthChar;

use crate::menu::MenuItem;
use crate::state::SidebarTab;

/// One divider button. Open-ended: `glyph` is drawn, `command` is an id only
/// the registering backend understands and the shell echoes back to its
/// lane-action provider.
#[derive(Debug, Clone)]
pub struct SectionButton {
    pub glyph: String,
    pub action: crate::system::LaneActionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneActionAnchor {
    pub x: u16,
    pub y: u16,
}

/// Truncate `s` to at most `max_width` display columns, appending an
/// ellipsis on overflow. Kept in the leaf geometry module so `model`
/// needn't reach up into `ui`; `ui::text` re-exports it.
pub fn truncate(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        // Only room for the ellipsis itself; no content fits beside it.
        return "…".to_string();
    }
    // One column short of `max_width`, leaving room for the ellipsis.
    let (head, _) = split_at_width(s, max_width - 1);
    format!("{head}…")
}

/// Split `s` into the longest prefix whose display width fits `max` columns
/// and the remainder. The prefix is empty when even the first char is wider
/// than `max` — callers decide whether to overflow or skip.
pub fn split_at_width(s: &str, max: usize) -> (&str, &str) {
    let mut width = 0usize;
    for (i, ch) in s.char_indices() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > max {
            return s.split_at(i);
        }
        width += ch_width;
    }
    (s, "")
}

// --- Tab / banner / header / footer geometry ---

/// Max display width of each side of a remote tab label. A remote tab
/// reads `host:session`; capping both sides keeps the whole label within
/// 13 columns (6 + ":" + 6), an ellipsis taking over past that.
pub const TAB_REMOTE_SIDE_MAX: usize = 6;

/// Visible label for a session tab in the vertical/tabs layout. Local
/// (`host == None`) shows the bare name; remote shows `host:session`, each
/// side truncated to `TAB_REMOTE_SIDE_MAX`; a loading placeholder (empty
/// name) shows just the host. Shared by tab renderer and hit-tester so
/// widths and click targets can't drift apart.
pub fn tab_label(host: Option<&str>, name: &str) -> String {
    match host {
        None => name.to_string(),
        Some(host) if name.is_empty() => truncate(host, TAB_REMOTE_SIDE_MAX),
        Some(host) => format!(
            "{}:{}",
            truncate(host, TAB_REMOTE_SIDE_MAX),
            truncate(name, TAB_REMOTE_SIDE_MAX),
        ),
    }
}

/// Leading padding (in columns) before the first tab in the tab bar.
pub const TAB_LEADING_PAD: u16 = 1;
/// Padding (in columns) between idx and name, and after name, inside a tab.
pub const TAB_INNER_PAD: u16 = 1;
/// Separator glyph rendered between tabs (width 1).
pub const TAB_SEPARATOR: &str = "│";
/// Shared footer/tab-bar menu label. The vertical tab layout reserves its
/// right edge before windowing sessions, so the menu never disappears merely
/// because there are too many tabs.
pub const MENU_LABEL: &str = "≡ menu";
/// Overflow marker shown at either edge of a windowed vertical tab run.
pub const TAB_OVERFLOW_MARKER: &str = "…";

/// Connector prefixed to a nested section's divider label: `TREE_BRANCH` while
/// siblings follow, `TREE_BRANCH_LAST` on the one that closes the run.
///
/// Both are exactly two cells, which is what lets the renderer trade them with
/// the collapse chevron so the connector leads the divider (see
/// `ui::sidebar::sessions::lead_with_branch`). The line is what says this
/// section hangs off the one above; the chevron is a control on it, and reads
/// as one only after the relationship is established.
pub const TREE_BRANCH: &str = "├ ";
/// See [`TREE_BRANCH`].
pub const TREE_BRANCH_LAST: &str = "└ ";
const TAB_OVERFLOW_RUN_WIDTH: u16 = 2; // marker + one separating space

/// Minimum sidebar content width before the update banner renders at all.
pub const BANNER_MIN_WIDTH: u16 = 8;

/// Rows of the sidebar header (the `Projects / Agents` tab selector).
pub const SIDEBAR_HEADER_HEIGHT: u16 = 2;

/// Whether the update banner renders: an update is known and the sidebar
/// content is wide enough for the label.
pub fn banner_visible(has_update: bool, content_width: u16) -> bool {
    has_update && content_width >= BANNER_MIN_WIDTH
}

/// Sidebar footer height in rows: `2` fixed (top separator + menu/version
/// line) plus the update banner (when shown). Shared by renderer and
/// hit-testing so they can't drift (when they did, the bottom session row
/// went click-dead).
pub fn sidebar_footer_height(banner_visible: bool) -> u16 {
    2 + banner_visible as u16
}

/// On-screen rect of the context menu anchored at `(menu_x, menu_y)`,
/// clamped inside the terminal. Shared by the renderer and mouse
/// hit-testing so they can't disagree about where the menu actually is.
pub fn context_menu_rect(
    items: &[MenuItem],
    menu_x: u16,
    menu_y: u16,
    term_w: u16,
    term_h: u16,
) -> Rect {
    let horizontal_margin = u16::from(term_w > 2);
    let available_w = term_w.saturating_sub(horizontal_margin * 2);
    let w = context_menu_width(items).min(available_w);
    let vertical_margin = u16::from(term_h > 2);
    let available_h = term_h.saturating_sub(vertical_margin * 2);
    let h = (items.len() as u16 + 2).min(available_h);
    let max_x = term_w.saturating_sub(horizontal_margin).saturating_sub(w);
    let x = menu_x.clamp(horizontal_margin, max_x);
    let max_y = term_h.saturating_sub(vertical_margin).saturating_sub(h);
    let y = menu_y.clamp(vertical_margin, max_y);
    Rect::new(x, y, w, h)
}

/// Collapse `$HOME` to `~` in a directory path. Pure; lives in the leaf
/// geometry module so the sidebar layout builder (in `model`) can format
/// session rows without reaching up into `ui`. `ui::text` re-exports it.
pub fn shorten_dir(dir: &str) -> String {
    // Resolved once: this runs per visible session row, and `env::var` takes
    // the process-global env lock + allocates each call.
    static HOME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let home = HOME.get_or_init(|| std::env::var("HOME").unwrap_or_default());
    match dir.strip_prefix(home.as_str()) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => dir.to_string(),
    }
}

fn tab_width(index: usize, name: &str) -> u16 {
    let idx_width = format!("{}", index + 1).len() as u16;
    let name_width = UnicodeWidthStr::width(name) as u16;
    idx_width
        .saturating_add(TAB_INNER_PAD)
        .saturating_add(name_width)
        .saturating_add(TAB_INNER_PAD)
}

/// One visible tab and its half-open content-column range. `index` is the
/// original flat session index, not its position inside the visible window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleTab {
    pub index: usize,
    pub start: u16,
    pub end: u16,
}

/// Geometry for the single-row vertical tab bar. The renderer and click
/// decoder both build this value, keeping overflow/windowing hit targets exact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabBarLayout {
    pub tabs: Vec<VisibleTab>,
    pub left_clipped: bool,
    pub right_clipped: bool,
    /// Content-relative column where the pinned menu label begins.
    pub menu_x: Option<u16>,
}

fn tab_run_width(labels: &[&str], start: usize, end: usize) -> u32 {
    let tabs = (start..end)
        .map(|i| u32::from(tab_width(i, labels[i])))
        .sum::<u32>();
    let separators = end.saturating_sub(start + 1) as u32;
    u32::from(TAB_LEADING_PAD)
        + if start > 0 {
            u32::from(TAB_OVERFLOW_RUN_WIDTH)
        } else {
            0
        }
        + tabs
        + separators
        + if end < labels.len() {
            u32::from(TAB_OVERFLOW_RUN_WIDTH)
        } else {
            0
        }
}

/// Window vertical session tabs around `focused`, reserving a pinned menu at
/// the right. Whole neighboring tabs are added symmetrically while they fit;
/// an unusually long focused label is truncated by the renderer to its range.
pub fn tab_bar_layout(labels: &[&str], focused: usize, width: u16) -> TabBarLayout {
    let menu_width = MENU_LABEL.width() as u16;
    let menu_x =
        (width >= menu_width.saturating_add(1)).then(|| width.saturating_sub(menu_width + 1));
    // Keep one quiet cell between tabs/overflow and the pinned menu.
    let tabs_limit = menu_x.map_or(width, |x| x.saturating_sub(1));

    if labels.is_empty() || tabs_limit <= TAB_LEADING_PAD {
        return TabBarLayout {
            tabs: Vec::new(),
            left_clipped: false,
            right_clipped: false,
            menu_x,
        };
    }

    let focused = focused.min(labels.len() - 1);
    let mut start = focused;
    let mut end = focused + 1;
    loop {
        let left_width = (start > 0).then(|| tab_run_width(labels, start - 1, end));
        let right_width = (end < labels.len()).then(|| tab_run_width(labels, start, end + 1));
        let left_fits = left_width.is_some_and(|w| w <= u32::from(tabs_limit));
        let right_fits = right_width.is_some_and(|w| w <= u32::from(tabs_limit));

        match (left_fits, right_fits) {
            (false, false) => break,
            (true, false) => start -= 1,
            (false, true) => end += 1,
            (true, true) => {
                let left_count = focused - start;
                let right_count = end - focused - 1;
                if left_count < right_count
                    || (left_count == right_count && left_width <= right_width)
                {
                    start -= 1;
                } else {
                    end += 1;
                }
            }
        }
    }

    let left_clipped = start > 0;
    let right_clipped = end < labels.len();
    let mut cursor = TAB_LEADING_PAD
        + if left_clipped {
            TAB_OVERFLOW_RUN_WIDTH
        } else {
            0
        };
    let right_reserve = if right_clipped {
        TAB_OVERFLOW_RUN_WIDTH
    } else {
        0
    };
    let mut tabs = Vec::with_capacity(end - start);
    for (i, label) in labels.iter().enumerate().take(end).skip(start) {
        let room = tabs_limit
            .saturating_sub(cursor)
            .saturating_sub(right_reserve);
        if room == 0 {
            break;
        }
        let width = tab_width(i, label).min(room);
        tabs.push(VisibleTab {
            index: i,
            start: cursor,
            end: cursor + width,
        });
        cursor += width;
        if i + 1 < end {
            cursor = cursor.saturating_add(TAB_SEPARATOR.width() as u16);
        }
    }

    TabBarLayout {
        tabs,
        left_clipped,
        right_clipped,
        menu_x,
    }
}

pub fn context_menu_width(items: &[MenuItem]) -> u16 {
    // Display width, not byte length: menu labels could carry wide chars,
    // and a byte count would over-size the popup for CJK.
    let max_w = items
        .iter()
        .map(|i| UnicodeWidthStr::width(i.label()))
        .max()
        .unwrap_or(0);
    (max_w as u16) + 4 // 1 border + 1 padding each side + 1 border
}

// --- Sidebar item / hit-region types ---

/// Sidebar layout — a `SectionedList` of the crate's `BasicItem` preset.
/// Headers carry a local / host divider (separator fill, accent
/// color, `[⟳]`/`[…]` buttons); rows carry a session/agent title plus dim
/// secondary lines. Geometry, focus-driven scroll, and hit-testing are
/// shared across renderer and action layer via this one type.
pub type SidebarLayout =
    ratatui_sectioned_list::SectionedList<ratatui_sectioned_list::widget::BasicItem>;

/// Metadata for one header in a [`SidebarLayout`], in push order parallel
/// to the crate's section numbering, so a `header_at_y` index resolves back
/// to the host it divides and its buttons. `BasicItem` headers carry only
/// text/buttons, not identity, so this side-table lets the hit-tester map a
/// divider click to a host (collapse / reconnect / menu).
#[derive(Debug, Clone)]
pub struct SectionMeta {
    /// Lane this divider heads. The hit-tester resolves clicks against it and
    /// the owning [`System`](crate::system::System) routes button actions by
    /// it. For a non-divider placeholder header it's still set; read `divider`
    /// to tell them apart.
    pub lane: LaneId,
    /// Buttons on this divider, left→right, matching the `BasicItem`
    /// `.button()` order. Empty for placeholder headers (empty-local /
    /// no-agents / detecting).
    pub buttons: Vec<SectionButton>,
    /// Whether the bar is a real, clickable group divider (toggles collapse,
    /// carries buttons). `false` for placeholder rows that occupy a header
    /// slot but aren't interactive.
    pub divider: bool,
}

/// Switches distinguishing the two sidebar tabs built through the shared
/// `build_sections` skeleton. Only these toggles and per-row content differ.
#[derive(Debug, Clone, Copy)]
pub struct SectionLayoutOpts {
    /// Push local / host divider headers. Projects omits them in Compact
    /// view (rows carry an origin prefix instead); the Agents tab always shows
    /// them.
    pub show_headers: bool,
    /// Track and apply per-section collapse (Projects/Expanded only). The
    /// Agents tab leaves this off so a host collapsed on Projects can't hide
    /// its agent rows.
    pub collapsible: bool,
    /// Give remote section headers a 1-row top margin (Agents tab) instead of
    /// sitting flush (Projects).
    pub remote_header_margin: bool,
}

/// A built sidebar layout plus the per-header metadata the hit-tester needs
/// to resolve divider clicks back to a host. Returned together so the two
/// can never drift: they're produced in the same pass.
#[derive(Debug, Clone)]
pub struct BuiltLayout {
    pub layout: SidebarLayout,
    pub sections: Vec<SectionMeta>,
}

impl Default for BuiltLayout {
    fn default() -> Self {
        Self {
            layout: SidebarLayout::new(),
            sections: Vec::new(),
        }
    }
}

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
    /// Max scroll offset for the Ready text at this width (0 = no overflow).
    pub max_scroll: usize,
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

impl AddRemoteHits {
    fn hit(&self, pos: Position) -> Option<HitKind> {
        if let Some(index) = row_at(&self.hosts, pos) {
            return Some(HitKind::AddRemoteHost(index));
        }
        if self.add.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::AddRemoteAdd);
        }
        if self.cancel.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::AddRemoteCancel);
        }
        None
    }
}

impl HiddenHits {
    fn hit(&self, pos: Position) -> Option<HitKind> {
        if let Some(index) = row_at(&self.rows, pos) {
            return Some(HitKind::HiddenRow(index));
        }
        if self.restore_all.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::HiddenRestoreAll);
        }
        if self.cancel.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::HiddenCancel);
        }
        None
    }
}

impl MountHits {
    fn hit(&self, pos: Position) -> Option<HitKind> {
        if let Some(index) = row_at(&self.rows, pos) {
            return Some(HitKind::MountRow(index));
        }
        if self.mount.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::MountConfirm);
        }
        if self.sort.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::MountSort);
        }
        if self.cancel.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::MountCancel);
        }
        None
    }
}

impl PfHits {
    fn hit(&self, pos: Position) -> Option<HitKind> {
        if let Some(index) = row_at(&self.rows, pos) {
            return Some(HitKind::PfRow(index));
        }
        if self.add.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::PfAdd);
        }
        if self.delete.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::PfDelete);
        }
        if self.close.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::PfClose);
        }
        None
    }
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
    /// The add-remote picker's footer `⏎ add` hint.
    AddRemoteAdd,
    /// The add-remote picker's footer `⎋ cancel` hint.
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
    /// The port-forward list's `[a] add` hint.
    PfAdd,
    /// The port-forward list's `[d] delete` hint.
    PfDelete,
    /// The port-forward list's `[esc] close` hint.
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

#[cfg(test)]
#[path = "../../tests/unit/ui/layout.rs"]
mod tests;

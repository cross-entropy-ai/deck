//! Sidebar / hit-region geometry, plus the pure layout helpers shared by
//! the UI (drawing) and the model/action (hit-testing) layers.
//!
//! These constants and pure functions used to live in `ui/layout.rs`, which
//! forced `model::state` to import *upward* into `ui` for its hit-testing.
//! They now live here in `model`, so `ui` and `app` depend on the model for
//! geometry, never the reverse.
//!
//! The tab bar's geometry (leading pad, inner pad, separator) is defined
//! here as the single source of truth. Renderer and hit-tester both read
//! from these constants/helpers, so tweaking the tab visual width
//! automatically keeps click-target math in sync.

use ratatui::layout::{Position, Rect};
use ratatui::style::Color;
use unicode_width::UnicodeWidthStr;

use unicode_width::UnicodeWidthChar;

use crate::menu::MenuItem;
use crate::state::SidebarTab;
use crate::theme::Theme;

/// Truncate `s` to at most `max_width` display columns, appending an
/// ellipsis when it overflows. A pure string helper kept here (in the
/// leaf geometry module) so `model` doesn't have to reach up into `ui`
/// for it; `ui::text` re-exports this name for its own call sites.
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
    let mut out = String::new();
    let mut width = 0usize;

    for ch in s.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width + 1 > max_width {
            break;
        }
        out.push(ch);
        width += ch_width;
    }

    format!("{out}…")
}

// --- Tab / banner / header / footer geometry ---

/// Max display width of each side of a remote tab label. A remote tab
/// reads `host:session`; capping both sides keeps the whole label within
/// 13 columns (6 + ":" + 6), an ellipsis taking over past that.
pub const TAB_REMOTE_SIDE_MAX: usize = 6;

/// Visible label for a session tab in the vertical/tabs layout.
///
/// Local sessions (`host == None`) show their bare name. Remote sessions
/// show `host:session`, each side truncated to `TAB_REMOTE_SIDE_MAX`; a
/// loading placeholder (empty name) shows just the host. Shared by the
/// tab renderer and the click hit-tester so tab widths and click targets
/// can't drift apart.
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

/// Minimum sidebar content width before the update banner renders at all.
pub const BANNER_MIN_WIDTH: u16 = 8;

/// Rows of the sidebar header (the `Projects / Agents` tab selector).
pub const SIDEBAR_HEADER_HEIGHT: u16 = 2;

/// Whether the update banner renders: an update is known and the sidebar
/// content is wide enough for the label.
pub fn banner_visible(has_update: bool, content_width: u16) -> bool {
    has_update && content_width >= BANNER_MIN_WIDTH
}

/// Height of the sidebar footer in rows: `2` fixed rows (the top
/// separator and the menu/version line) plus the update banner (when
/// shown) plus the plugin block. Shared by the renderer and mouse
/// hit-testing so the two can't drift (when they did, the bottom visible
/// session row went click-dead).
pub fn sidebar_footer_height(banner_visible: bool, plugin_count: usize) -> u16 {
    2 + banner_visible as u16 + plugin_block_rows(plugin_count)
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
    let w = context_menu_width(items);
    let h = items.len() as u16 + 2;
    let x = menu_x.min(term_w.saturating_sub(w));
    let y = menu_y.min(term_h.saturating_sub(h));
    Rect::new(x, y, w, h)
}

/// Rows the plugin status block takes in the sidebar footer: title +
/// one row per plugin + trailing separator. Zero when no plugins are
/// configured so the sidebar keeps its original layout for users
/// without any extensions. Shared so mouse hit-testing in
/// `AppState::focus_at_row` stays in sync with the sidebar renderer.
pub const fn plugin_block_rows(count: usize) -> u16 {
    if count == 0 {
        0
    } else {
        count as u16 + 2
    }
}

/// Accent color for a remote host's divider, cycled by the host's order
/// among distinct remote hosts. Shared by the layout builder (which bakes it
/// into the `BasicItem` header) so dividers keep their per-host tint.
pub fn host_accent(theme: &Theme, host_idx: usize) -> Color {
    let tints = [theme.teal, theme.pink, theme.yellow, theme.accent];
    tints[host_idx % tints.len()]
}

/// Collapse `$HOME` to `~` in a directory path. Pure; lives in the leaf
/// geometry module so the sidebar layout builder (in `model`) can format
/// session rows without reaching up into `ui`. `ui::text` re-exports it.
pub fn shorten_dir(dir: &str) -> String {
    // Resolved once: this runs per visible session row, and `env::var` takes
    // the process-global env lock + allocates each call.
    static HOME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let home = HOME.get_or_init(|| std::env::var("HOME").unwrap_or_default());
    if !home.is_empty() && dir.starts_with(home) {
        format!("~{}", &dir[home.len()..])
    } else {
        dir.to_string()
    }
}

/// Short idle-age badge (`3m` / `2h` / `1d`), or `None` under a minute.
/// Pure; co-located with [`shorten_dir`] for the same reason. `ui::text`
/// re-exports it.
pub fn format_idle_badge(seconds: u64) -> Option<String> {
    if seconds < 60 {
        return None;
    }
    if seconds < 3600 {
        return Some(format!("{}m", seconds / 60));
    }
    if seconds < 86_400 {
        return Some(format!("{}h", seconds / 3600));
    }
    Some(format!("{}d", seconds / 86_400))
}

fn tab_width(index: usize, name: &str) -> u16 {
    let idx_width = format!("{}", index + 1).len() as u16;
    let name_width = UnicodeWidthStr::width(name) as u16;
    idx_width + TAB_INNER_PAD + name_width + TAB_INNER_PAD
}

/// Column ranges (start, end) for each tab in the vertical/tabs layout,
/// computed from session names alone. Used by the renderer to place
/// tabs and by state to map a click column back to a tab index.
pub fn tab_col_ranges(names: &[&str]) -> Vec<(u16, u16)> {
    let mut ranges = Vec::with_capacity(names.len());
    let mut x: u16 = TAB_LEADING_PAD;
    for (i, name) in names.iter().enumerate() {
        let width = tab_width(i, name);
        ranges.push((x, x + width));
        x += width;
        if i + 1 < names.len() {
            x += TAB_SEPARATOR.chars().count() as u16;
        }
    }
    ranges
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
/// Headers carry an `@local` / `@host` divider (separator fill, accent
/// color, `[⟳]`/`[…]` buttons); rows carry a session/agent title plus dim
/// secondary lines. Geometry, focus-driven scroll, and mouse hit-testing
/// are shared across the renderer (`SectionedListWidget::basic`) and the
/// action layer via this one type.
pub type SidebarLayout =
    ratatui_sectioned_list::SectionedList<ratatui_sectioned_list::widget::BasicItem>;

/// Metadata for one header in a [`SidebarLayout`], in push order — parallel
/// to the crate's section numbering, so a `header_at_y` index resolves
/// straight back to the host it divides and the buttons drawn on it.
///
/// `BasicItem` headers carry only text/buttons, not identity, so this
/// side-table is what lets the hit-tester map a divider click to a host
/// (collapse / reconnect / menu) the way the old `SidebarItemData::Header`
/// did.
#[derive(Debug, Clone)]
pub struct SectionMeta {
    /// Host this divider heads (`None` = `@local`). `None` for a
    /// non-divider placeholder header too — read `divider` to tell them apart.
    pub host: Option<String>,
    /// Buttons on this divider, left→right, matching the `BasicItem`
    /// `.button()` order. Empty for placeholder headers (empty-local /
    /// no-agents / detecting).
    pub buttons: Vec<DividerButton>,
    /// Whether the bar is a real, clickable group divider (toggles collapse,
    /// carries buttons). `false` for placeholder rows that occupy a header
    /// slot but aren't interactive.
    pub divider: bool,
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

/// One focusable agent row in the Agents tab, in display order: local
/// agents first, then each remote host's agents in section order. The
/// renderer and the `Agent { row_idx }` layout items both index into the
/// `Vec` this produces (`AppState::agent_rows`), so they can't disagree
/// about which agent a row points at.
///
/// Borrows its `host`/`agent` straight out of `AppState.agents` rather
/// than cloning: `agent_rows()` runs per frame *and* per keystroke, so a
/// per-row `DetectedAgent` clone there was pure waste (D17). The `Vec`
/// the produced rows live in is short-lived (one frame / one call), so a
/// borrow is always available.
#[derive(Debug, Clone, Copy)]
pub struct AgentRow<'a> {
    pub host: Option<&'a str>,
    pub agent: &'a crate::agent::DetectedAgent,
}

/// Which button on a divider a `DividerHit` targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DividerButton {
    /// `[⟳]` — force-refresh (reconnect) the host.
    Reconnect,
    /// `[…]` — open the host-divider menu.
    More,
    /// `[…]` on the `@local` divider — opens the local-divider menu. Carries
    /// no host (the `DividerHit.host` is unused for this kind).
    LocalMore,
}

/// Click-region for one button (`[⟳]` or `[…]`) on a remote-host
/// divider. The sidebar renderer fills `HitRegions.dividers` after each
/// render; mouse hit-testing resolves it before `focus_at_row()`.
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
/// published by the header renderer so mouse dispatch can switch tabs on
/// a click. Each rect is clamped to the header area so a narrow sidebar
/// can't leak a tab's click target into the PTY pane (bug #16).
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

/// Every clickable region the sidebar publishes for one frame: the
/// renderer captures it whole and `AppState` stores it as a single field.
/// `HitRegions::hit` is the one resolver mouse dispatch consults for every
/// rect-based button/region test, so hit-test priority lives in one place
/// and geometry can't drift across the layers (it dissolves the
/// point-in-rect copies of D1).
///
/// Rects are clamped to the sidebar content area at capture time, so a
/// narrow sidebar can never publish a button that overlaps the PTY pane.
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
    /// The Summary card's buttons/card/scroll bound.
    pub summary: SummaryHits,
    /// The footer's "menu" button.
    pub menu: Option<Rect>,
}

/// What a `(col, row)` click resolves to among the sidebar's rect-based
/// regions. Vecs are carried by index so the caller reads the matched
/// `DividerHit` / `AgentHit` straight out of the registry — keeping the
/// hit data (host, kind, target, rect) in one place.
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
    /// The Summary card's "Generate" button.
    SummaryButton,
    /// The Summary card's "popup" (big view) button.
    SummaryPopup,
    /// Anywhere on the Summary card (for routing wheel events).
    SummaryCard,
    /// The footer "menu" button; carries its rect (mouse anchors the menu
    /// at its x/y).
    Menu(Rect),
    /// A divider button; carries an index into `HitRegions.dividers`.
    Divider(usize),
    /// An agent row; carries an index into `HitRegions.agents`.
    Agent(usize),
}

impl HitRegions {
    /// Resolve a click at `(col, row)` to the region it lands on, if any.
    /// The match order encodes hit-test priority: the kill buttons,
    /// banner, tabs, and summary buttons take precedence over the menu
    /// button, then dividers (whose buttons sit on a group header row),
    /// then agent rows. Uses `Rect::contains` throughout.
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
        if self.summary.card.is_some_and(|r| r.contains(pos)) {
            return Some(HitKind::SummaryCard);
        }
        None
    }
}

#[cfg(test)]
#[path = "../../tests/unit/ui/layout.rs"]
mod tests;

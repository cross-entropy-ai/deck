//! Carving the frame into rects, and the layout pass's output.
//!
//! Everything that answers "where does this go" — pane split, sidebar bands,
//! context-menu placement — plus the `SidebarLayout` the section builder fills
//! and the `BuiltLayout` the renderer and hit-testers both read.

use std::ops::Range;

use ratatui::layout::{Constraint, Layout, Margin, Rect};
use unicode_width::UnicodeWidthStr;

use crate::lane::LaneId;
use crate::menu::MenuItem;
use crate::state::LayoutMode;

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

/// Right-aligned button cell ranges within a section header, in button order
/// with a one-cell gap. This is the single Deck-owned copy of the geometry
/// used to publish click targets for the sectioned-list preset.
pub fn header_button_ranges(width: u16, buttons: &[String]) -> Vec<Range<u16>> {
    if buttons.is_empty() {
        return Vec::new();
    }
    let widths: Vec<u16> = buttons.iter().map(|button| button.width() as u16).collect();
    let total = widths.iter().sum::<u16>() + buttons.len() as u16 - 1;
    if total > width {
        return Vec::new();
    }

    let mut x = width - total;
    widths
        .into_iter()
        .enumerate()
        .map(|(index, button_width)| {
            if index > 0 {
                x += 1;
            }
            let range = x..x + button_width;
            x = range.end;
            range
        })
        .collect()
}

/// All screen regions owned by the top-level Deck layout. Render, PTY sizing,
/// and mouse routing consume this same value so borders and divider columns
/// cannot drift between three independent formulas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneAreas {
    /// Rect passed to the sidebar renderer. In the shared horizontal frame it
    /// is already border-free; in vertical layout it still owns its frame.
    pub sidebar: Rect,
    /// Actual sidebar content, used by sidebar sub-layout and hit-testing.
    pub sidebar_content: Rect,
    /// Screen region routed to sidebar input, including its outer edge.
    pub sidebar_hit: Rect,
    /// The visible horizontal divider, absent in vertical layout.
    pub divider: Option<Rect>,
    /// Divider drag target. Borderless mode intentionally includes one extra
    /// column as a larger affordance.
    pub divider_hit: Option<Rect>,
    /// Rect passed to the main-pane renderer.
    pub main: Rect,
    /// Exact terminal surface inside any border.
    pub main_content: Rect,
    /// One shared outer frame for bordered horizontal layout.
    pub shared_frame: Option<Rect>,
}

/// Resolve the complete Deck pane geometry for one terminal frame.
pub fn pane_areas(
    full: Rect,
    mode: LayoutMode,
    sidebar_width: u16,
    sidebar_height: u16,
    show_borders: bool,
) -> PaneAreas {
    match mode {
        LayoutMode::Horizontal if show_borders => {
            let inner = full.inner(Margin::new(1, 1));
            let [sidebar, divider, main] = Layout::horizontal([
                Constraint::Length(sidebar_width.saturating_sub(1)),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .areas(inner);
            PaneAreas {
                sidebar,
                sidebar_content: sidebar,
                sidebar_hit: Rect {
                    width: divider.x.saturating_sub(full.x),
                    ..full
                },
                divider: Some(divider),
                divider_hit: Some(divider),
                main,
                main_content: main,
                shared_frame: Some(full),
            }
        }
        LayoutMode::Horizontal => {
            let [sidebar, divider, main] = Layout::horizontal([
                Constraint::Length(sidebar_width),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .areas(full);
            let divider_hit = Rect {
                width: 2.min(full.right().saturating_sub(divider.x)),
                ..divider
            };
            PaneAreas {
                sidebar,
                sidebar_content: sidebar,
                sidebar_hit: sidebar,
                divider: Some(divider),
                divider_hit: Some(divider_hit),
                main,
                main_content: main,
                shared_frame: None,
            }
        }
        LayoutMode::Vertical => {
            let [sidebar, main] =
                Layout::vertical([Constraint::Length(sidebar_height), Constraint::Min(1)])
                    .areas(full);
            let sidebar_content = if show_borders {
                sidebar.inner(Margin::new(1, 1))
            } else {
                sidebar
            };
            let main_content = if show_borders {
                main.inner(Margin::new(1, 1))
            } else {
                main
            };
            PaneAreas {
                sidebar,
                sidebar_content,
                sidebar_hit: sidebar,
                divider: None,
                divider_hit: None,
                main,
                main_content,
                shared_frame: None,
            }
        }
    }
}

/// Stable sub-regions inside an expanded horizontal sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarAreas {
    pub header: Rect,
    /// Entire area between header and footer, used by inline overlays.
    pub body: Rect,
    pub list: Rect,
    pub summary: Rect,
    pub footer: Rect,
    pub banner_visible: bool,
}

/// Split sidebar content once for rendering and mouse row resolution.
pub fn sidebar_areas(content: Rect, has_update: bool, summary_height: u16) -> SidebarAreas {
    let show_banner = banner_visible(has_update, content.width);
    let footer_height = sidebar_footer_height(show_banner);
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(SIDEBAR_HEADER_HEIGHT),
        Constraint::Min(1),
        Constraint::Length(footer_height),
    ])
    .areas(content);
    let summary_height = summary_height.min(body.height);
    let [list, summary] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(summary_height)]).areas(body);
    SidebarAreas {
        header,
        body,
        list,
        summary,
        footer,
        banner_visible: show_banner,
    }
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
    /// Per row (by row index): whether the tree line running down the gutter
    /// continues past it, because a section below still hangs off the same
    /// parent — or off this row's own lane. Without it the connector on a
    /// nested divider dangles: the elbow is drawn, but nothing joins it to the
    /// group it came from across the rows in between.
    pub tree_rows: Vec<bool>,
}

impl Default for BuiltLayout {
    fn default() -> Self {
        Self {
            layout: SidebarLayout::new(),
            sections: Vec::new(),
            tree_rows: Vec::new(),
        }
    }
}

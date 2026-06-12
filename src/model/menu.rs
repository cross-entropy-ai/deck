//! Sidebar context menus: the `MenuItem` enum, the per-context item lists,
//! and the `ContextMenu` overlay state with its enabled/disabled logic.

use crate::state::{attachable_on_host, FocusTarget, RemoteSessionRow, SessionTargetRef};

// One list for local and remote rows. "Switch" is dropped — the focus
// already triggers the switch, so the menu item was redundant. On a remote
// row Rename/Kill map to `ssh <host> tmux <cmd>` and MoveUp/MoveDown reorder
// *within the host group* (hosts can't interleave), persisted to that
// server's `@deck_order` — same labels, different backend.
const SESSION_MENU_ITEMS: &[MenuItem] = &[
    MenuItem::Rename,
    MenuItem::Kill,
    MenuItem::MoveUp,
    MenuItem::MoveDown,
];
// Items shown but greyed-out / unselectable when the right-clicked row
// is a synthetic placeholder (a remote host with no sessions, or an
// unreachable one): there's no real session to Rename/Kill/reorder —
// i.e. every session item.
const PLACEHOLDER_DISABLED_ITEMS: &[MenuItem] = SESSION_MENU_ITEMS;
// Only Kill is greyed when the row is the last live session on a remote
// host: killing it would tear down that host's tmux server. Rename is
// still fine.
const LAST_REMOTE_SESSION_DISABLED: &[MenuItem] = &[MenuItem::Kill];
// Host divider [...] menu acts on the whole remote *group*. RemoveFromList
// is equivalent to `deck remote remove <host>`.
const HOST_DIVIDER_MENU_ITEMS: &[MenuItem] = &[
    MenuItem::NewSession,
    MenuItem::PortForward,
    MenuItem::RemoveFromList,
];
// The `@local` divider reuses the host divider's item list so the menu
// reads consistently, but PortForward and RemoveFromList are remote-
// only concepts: they're shown greyed out, leaving just NewSession
// (which creates a local session).
const LOCAL_DIVIDER_DISABLED: &[MenuItem] = &[MenuItem::PortForward, MenuItem::RemoveFromList];
// Right-click on blank sidebar space. NewSession is intentionally
// absent — creating a local session lives on the `@local` divider's
// `[…]` menu instead.
const GLOBAL_MENU_ITEMS: &[MenuItem] = &[
    MenuItem::AddRemoteHost,
    MenuItem::ToggleLayout,
    MenuItem::ToggleBorders,
    MenuItem::Settings,
    MenuItem::Quit,
];

/// One entry in a sidebar context menu. Carries its own display text via
/// [`MenuItem::label`], so the renderer, the enabled/disabled subsets, and
/// the dispatch in `reduce.rs` all key off the variant instead of the label
/// string — renaming an item's text can't silently detach its action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Rename,
    Kill,
    MoveUp,
    MoveDown,
    AddRemoteHost,
    ToggleLayout,
    ToggleBorders,
    Settings,
    Quit,
    NewSession,
    PortForward,
    RemoveFromList,
}

impl MenuItem {
    /// The text rendered for this item. Kept byte-identical to the old
    /// `&'static str` menu literals so the popup looks unchanged.
    pub fn label(&self) -> &'static str {
        match self {
            MenuItem::Rename => "Rename",
            MenuItem::Kill => "Kill",
            MenuItem::MoveUp => "Move up",
            MenuItem::MoveDown => "Move down",
            MenuItem::AddRemoteHost => "Add Remote Host",
            MenuItem::ToggleLayout => "Toggle layout",
            MenuItem::ToggleBorders => "Toggle borders",
            MenuItem::Settings => "Settings",
            MenuItem::Quit => "Quit",
            MenuItem::NewSession => "New session",
            MenuItem::PortForward => "Port Forward",
            MenuItem::RemoveFromList => "Remove from list",
        }
    }
}

#[derive(Debug, Clone)]
pub enum MenuKind {
    /// Right-clicked a session row. Local and remote rows share one item
    /// list (`SESSION_MENU_ITEMS`); only the greyed subset is per-row.
    Session {
        focus: FocusTarget,
        /// Subset of the items shown greyed-out and not selectable (e.g.
        /// Rename/Kill on a synthetic placeholder row). Empty for a real
        /// session, where every item is actionable.
        disabled: &'static [MenuItem],
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
    pub fn items(&self) -> &'static [MenuItem] {
        match self {
            MenuKind::Session { .. } => SESSION_MENU_ITEMS,
            MenuKind::Global => GLOBAL_MENU_ITEMS,
            MenuKind::HostDivider { .. } | MenuKind::LocalDivider => HOST_DIVIDER_MENU_ITEMS,
        }
    }

    /// Items that are shown but greyed-out / unselectable: session menus
    /// carry a per-row set, and the `@local` divider greys the remote-only
    /// items. Other menus have every item enabled.
    pub fn disabled(&self) -> &'static [MenuItem] {
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
) -> &'static [MenuItem] {
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
    pub fn items(&self) -> &'static [MenuItem] {
        self.kind.items()
    }

    pub fn disabled(&self) -> &'static [MenuItem] {
        self.kind.disabled()
    }

    /// Whether the item at `idx` is selectable (exists and not disabled).
    pub fn is_enabled(&self, idx: usize) -> bool {
        self.items()
            .get(idx)
            .is_some_and(|item| !self.disabled().contains(item))
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

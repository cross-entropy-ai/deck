//! Sidebar context menus: the `MenuItem` enum, the per-context item lists,
//! and the `ContextMenu` overlay state with its enabled/disabled logic.

use crate::state::{attachable_on_lane, FocusTarget, SessionEntry};
use crate::system::SessionCapabilities;

// One list for local and remote rows. No "Switch" item — focus already
// switches. On a remote row Rename/Close map to `ssh <host> tmux <cmd>`.
// Reordering is direct manipulation (left-button drag), not a menu command.
const SESSION_MENU_ITEMS: &[MenuItem] = &[MenuItem::Rename, MenuItem::Close];
// Greyed-out when the right-clicked row is a synthetic placeholder (remote
// host with no sessions, or unreachable): no real session to
// Rename/Close, i.e. every session item.
const PLACEHOLDER_DISABLED_ITEMS: &[MenuItem] = SESSION_MENU_ITEMS;
// Only Close is greyed when the row is the last live session on a remote
// host: killing it would tear down that host's tmux server. Rename is
// still fine.
const LAST_REMOTE_SESSION_DISABLED: &[MenuItem] = &[MenuItem::Close];
const RENAME_DISABLED: &[MenuItem] = &[MenuItem::Rename];
// Host divider [...] menu acts on the whole remote *group*. RemoveFromList
// is equivalent to `deck remote remove <host>`.
const HOST_DIVIDER_MENU_ITEMS: &[MenuItem] = &[
    MenuItem::NewSession,
    MenuItem::Containers,
    MenuItem::PortForward,
    MenuItem::RemoveFromList,
];
// The `@local` divider reuses the host divider's item list for consistency,
// but PortForward and RemoveFromList are remote-only: they're greyed out,
// leaving just NewSession (creates a local session).
const LOCAL_DIVIDER_DISABLED: &[MenuItem] = &[
    MenuItem::Containers,
    MenuItem::PortForward,
    MenuItem::RemoveFromList,
];
const PORT_FORWARD_DISABLED: &[MenuItem] = &[MenuItem::PortForward];
const MOUNTS_DISABLED: &[MenuItem] = &[MenuItem::Containers];
const PORT_FORWARD_AND_MOUNTS_DISABLED: &[MenuItem] =
    &[MenuItem::Containers, MenuItem::PortForward];
// Right-click on blank sidebar space / the persistent footer button. Put the
// primary creation action first; the explicit "local" label distinguishes it
// from the per-host divider's NewSession action.
const GLOBAL_MENU_ITEMS: &[MenuItem] = &[
    MenuItem::NewLocalSession,
    MenuItem::AddRemoteHost,
    MenuItem::Settings,
    MenuItem::Quit,
];

/// One entry in a sidebar context menu. Carries its display text via
/// [`MenuItem::label`]; renderer, enabled/disabled subsets, and dispatch
/// key off the variant, not the label, so renaming an item's text can't
/// silently detach its action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Rename,
    Close,
    AddRemoteHost,
    Settings,
    Quit,
    NewLocalSession,
    NewSession,
    Containers,
    PortForward,
    RemoveFromList,
}

impl MenuItem {
    /// The text rendered for this item.
    pub fn label(&self) -> &'static str {
        match self {
            MenuItem::Rename => "Rename",
            MenuItem::Close => "Close",
            MenuItem::AddRemoteHost => "Add Remote Host",
            MenuItem::Settings => "Settings",
            MenuItem::Quit => "Quit",
            MenuItem::NewLocalSession => "New local session",
            MenuItem::NewSession => "New session",
            MenuItem::Containers => "Containers…",
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
        /// Items shown greyed-out and unselectable (e.g. Rename/Close on a
        /// placeholder row). Empty for a real session.
        disabled: &'static [MenuItem],
    },
    Global,
    LaneDivider {
        lane: crate::lane::LaneId,
        primary: bool,
        port_forward_enabled: bool,
        /// Whether this lane's system offers lanes to mount under it.
        mounts_enabled: bool,
    },
}

impl MenuKind {
    pub fn items(&self) -> &'static [MenuItem] {
        match self {
            MenuKind::Session { .. } => SESSION_MENU_ITEMS,
            MenuKind::Global => GLOBAL_MENU_ITEMS,
            MenuKind::LaneDivider { .. } => HOST_DIVIDER_MENU_ITEMS,
        }
    }

    /// Items that are shown but greyed-out / unselectable: session menus
    /// carry a per-row set, and the `@local` divider greys the remote-only
    /// items. Other menus have every item enabled.
    pub fn disabled(&self) -> &'static [MenuItem] {
        match self {
            MenuKind::Session { disabled, .. } => disabled,
            MenuKind::LaneDivider { primary: true, .. } => LOCAL_DIVIDER_DISABLED,
            MenuKind::LaneDivider {
                port_forward_enabled: false,
                mounts_enabled: false,
                ..
            } => PORT_FORWARD_AND_MOUNTS_DISABLED,
            MenuKind::LaneDivider {
                port_forward_enabled: false,
                ..
            } => PORT_FORWARD_DISABLED,
            MenuKind::LaneDivider {
                mounts_enabled: false,
                ..
            } => MOUNTS_DISABLED,
            MenuKind::Global | MenuKind::LaneDivider { .. } => &[],
        }
    }
}

/// Menu items to grey out for a right-clicked row: a placeholder (no
/// sessions / unreachable) disables Rename and Close; the last live session
/// on a remote host disables Close (closing it tears down the host's tmux
/// server), Rename stays; everything else has every item enabled.
pub fn session_menu_disabled(
    entry: &SessionEntry,
    entries: &[SessionEntry],
    capabilities: SessionCapabilities,
) -> &'static [MenuItem] {
    if !entry.is_attachable() || (!capabilities.rename && !capabilities.kill) {
        return PLACEHOLDER_DISABLED_ITEMS;
    }
    let last_remote = attachable_on_lane(entries, &entry.lane).nth(1).is_none();
    match (capabilities.rename, capabilities.kill && !last_remote) {
        (false, false) => PLACEHOLDER_DISABLED_ITEMS,
        (false, true) => RENAME_DISABLED,
        (true, false) => LAST_REMOTE_SESSION_DISABLED,
        (true, true) => &[],
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

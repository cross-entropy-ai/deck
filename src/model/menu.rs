//! Sidebar context menus: the `MenuItem` enum, the per-context item lists,
//! and the `ContextMenu` overlay state with its enabled/disabled logic.

use crate::state::{attachable_on_lane, FocusTarget, SessionEntry};
use crate::system::SessionCapabilities;

// One list for local and remote rows. No "Switch" item — focus already
// switches. On a remote row Rename/Close map to `ssh <host> tmux <cmd>`.
// Reordering is direct manipulation (left-button drag), not a menu command.
const SESSION_MENU_ITEMS: &[MenuItem] = &[MenuItem::Rename, MenuItem::Close, MenuItem::Hide];
// Greyed-out when the right-clicked row is a synthetic placeholder (remote
// host with no sessions, or unreachable): no real session to
// Rename/Close, i.e. every session item.
pub(crate) const PLACEHOLDER_DISABLED_ITEMS: &[MenuItem] = SESSION_MENU_ITEMS;
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
    MenuItem::ShowHidden,
    MenuItem::RemoveFromList,
];
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
    /// Stop capturing this one session (see `config::HiddenSession`).
    Hide,
    /// Undo every `Hide` on this lane. It lives on the divider because that is
    /// where the sessions went missing, which is where someone will look for
    /// them — a Settings list would be correct and undiscoverable.
    ShowHidden,
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
            MenuItem::Hide => "Hide",
            MenuItem::ShowHidden => "Show hidden",
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
        /// Whether this lane has any hidden session to restore.
        has_hidden: bool,
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
    /// carry a per-row set, and a divider greys whatever its lane cannot do.
    ///
    /// Built rather than picked from a static table: the divider's greyed set
    /// is four independent flags, and enumerating their combinations was
    /// already at four constants before `Show hidden` would have made it eight.
    pub fn disabled(&self) -> Vec<MenuItem> {
        match self {
            MenuKind::Session { disabled, .. } => disabled.to_vec(),
            MenuKind::Global => Vec::new(),
            MenuKind::LaneDivider {
                primary,
                port_forward_enabled,
                mounts_enabled,
                has_hidden,
                ..
            } => {
                let mut out = Vec::new();
                // Both key off the lane's own capability, never off "is this
                // the local lane": deck mounts containers on this machine too,
                // and the local lane already answers `false` to port forwards.
                if !mounts_enabled {
                    out.push(MenuItem::Containers);
                }
                if !port_forward_enabled {
                    out.push(MenuItem::PortForward);
                }
                if !has_hidden {
                    out.push(MenuItem::ShowHidden);
                }
                // The local lane is not in the list it would be removed from.
                if *primary {
                    out.push(MenuItem::RemoveFromList);
                }
                out
            }
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

    pub fn disabled(&self) -> Vec<MenuItem> {
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

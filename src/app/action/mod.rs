mod keyboard;
mod mouse;
mod reduce;

pub use keyboard::key_to_action;
pub use mouse::mouse_to_action;
pub use reduce::apply_action;

#[derive(Debug)]
pub enum Action {
    FocusNext,
    FocusPrev,
    FocusIndex(usize),
    ScrollUp,
    ScrollDown,

    SwitchProject,
    KillSession,
    ConfirmKill,
    CancelKill,
    ReorderSession(i32),
    /// Detach a remote host from deck's config — equivalent to
    /// `deck remote remove <host>`. Triggered from the remote-session
    /// right-click menu's "Remove from list".
    RemoveRemoteFromList(String),
    StartRename,
    RenameInputKey(crossterm::event::KeyEvent),
    RenameConfirm,
    RenameCancel,

    ToggleLayout,
    ToggleBorders,
    /// Flip the "Show Agents" checkbox in the sidebar header (clicked).
    ToggleShowAgents,
    ToggleViewMode,
    /// Collapse/expand a sidebar group (Expanded view only). `None` is the
    /// `@local` group; `Some(host)` is a remote `@host` group. Fired by a
    /// divider click or the section-toggle keybinding.
    ToggleSection(Option<String>),
    OpenSettings,
    CloseSettings,
    SettingsNext,
    SettingsPrev,
    SettingsAdjust,
    OpenThemePicker,
    CloseThemePicker,
    ThemePickerNext,
    ThemePickerPrev,
    ConfirmThemePicker,

    OpenKeybindingsView,
    CloseKeybindingsView,
    KeybindingsViewScrollUp,
    KeybindingsViewScrollDown,

    ToggleUpdateCheck,
    TriggerUpgrade,
    AbortUpgrade,
    ReloadConfig,

    OpenExcludeEditor,
    CloseExcludeEditor,
    ExcludeEditorNext,
    ExcludeEditorPrev,
    ExcludeEditorStartAdd,
    ExcludeEditorDelete,
    ExcludeEditorInputKey(crossterm::event::KeyEvent),
    ExcludeEditorConfirm,
    ExcludeEditorCancelAdd,

    CloseNewSessionPicker,
    NewSessionInputKey(crossterm::event::KeyEvent),
    NewSessionConfirm,
    NewSessionPrev,
    NewSessionNext,
    NewSessionClear,
    NewSessionDeleteSegment,
    NewSessionSwitchFocus,
    NewSessionDirUp,
    NewSessionDirEnter,

    ToggleHelp,
    DismissHelp,

    SetFocusMain,
    SetFocusSidebar,
    ToggleFocus,

    OpenSessionMenu {
        target: crate::state::FocusTarget,
        x: u16,
        y: u16,
    },
    OpenGlobalMenu {
        x: u16,
        y: u16,
    },
    /// Open the `@local` divider's `[…]` menu (local "New session";
    /// Port Forward / Remove from list greyed out).
    OpenLocalDividerMenu {
        x: u16,
        y: u16,
    },
    MenuNext,
    MenuPrev,
    MenuConfirm,
    MenuDismiss,
    MenuHover(usize),
    MenuClickItem(usize),

    SidebarClickSession(usize),
    NumberKeyJump(usize),
    /// Switch to (and focus) the pane a detected agent runs in. Fired by
    /// clicking an agent line in a section footer.
    SwitchToAgentPane(crate::state::AgentTarget),

    ResizeSidebar(u16),
    ResizeSidebarHeight(u16),
    StartDrag,
    StopDrag,

    Resize(u16, u16),

    ForwardKey(Vec<u8>),
    ForwardMouse(Vec<u8>),

    ActivatePlugin(usize),
    DeactivatePlugin,

    Quit,

    // Port-forward overlay (per-host)
    OpenHostDividerMenu {
        host: String,
        x: u16,
        y: u16,
    },
    ReconnectHost {
        host: String,
    },
    OpenPortForward(String),
    PfClose,
    PfFocusUp,
    PfFocusDown,
    PfDelete,
    PfAddOpen,
    PfAddCancel,
    PfAddSubmit,
    PfAddFieldNext,
    PfAddFieldPrev,
    PfAddModeLeft,
    PfAddModeRight,
    /// Forward a raw key event to the focused textarea (insert/delete/
    /// arrow within a field). Modal keys (Tab/Enter/Esc/Up/Down/etc.)
    /// are mapped to their own actions before reaching this variant.
    PfAddInputKey(crossterm::event::KeyEvent),
    PfTaskResult {
        host: String,
        op: crate::app::port_forward_task::OpKind,
        ok: bool,
        message: String,
    },

    AddRemoteInputKey(crossterm::event::KeyEvent),
    AddRemoteNext,
    AddRemotePrev,
    AddRemoteConfirm,
    AddRemoteClose,

    PfProbeResult {
        key: crate::state::ForwardKey,
        health: crate::state::ForwardHealth,
    },

    None,
}

mod keyboard;
mod mouse;
mod reduce;

pub use keyboard::key_to_action;
pub use mouse::mouse_to_action;
pub use reduce::apply_action;

#[cfg(test)]
#[path = "../../../tests/unit/app/action/modality.rs"]
mod modality_tests;

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
    ToggleTransparentBg,
    /// Switch the sidebar to a specific tab (clicked tab label).
    SelectTab(crate::state::SidebarTab),
    /// Toggle between the Projects and Agents sidebar tabs (keybinding).
    ToggleSidebarTab,
    ToggleViewMode,
    /// Collapse/expand a sidebar group (Expanded view only). `None` is the
    /// `@local` group; `Some(host)` is a remote `@host` group. Fired by a
    /// divider click or the section-toggle keybinding.
    ToggleSection(Option<String>),

    TriggerUpgrade,
    AbortUpgrade,
    ReloadConfig,

    ToggleHelp,
    DismissHelp,

    SetFocusMain,
    ToggleFocus,

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

    ReconnectHost {
        host: String,
    },

    Quit,

    /// Settings page and its sub-overlays (theme picker, keybindings view,
    /// exclude editor).
    Settings(SettingsAction),
    /// Agents-tab summary card, popup, and language editor.
    Summary(SummaryAction),
    /// New-session picker (local and remote).
    NewSession(NewSessionAction),
    /// Sidebar context menus (session / global / divider) and their navigation.
    Menu(MenuAction),
    /// Per-host port-forward overlay and its add form.
    Pf(PfAction),
    /// Add-remote-host picker.
    AddRemote(AddRemoteAction),

    None,
}

#[derive(Debug)]
pub enum SettingsAction {
    Open,
    Close,
    Next,
    Prev,
    Adjust,
    AdjustPrev,
    OpenThemePicker,
    CloseThemePicker,
    ThemePickerNext,
    ThemePickerPrev,
    ConfirmThemePicker,

    OpenKeybindingsView,
    CloseKeybindingsView,
    KeybindingsScrollUp,
    KeybindingsScrollDown,

    ToggleUpdateCheck,
    CycleFrameRateLimit(i32),
    /// Cycle the Agents-tab probe interval (settings, left/right).
    CycleAgentsProbeInterval(i32),
    /// Toggle the inline Summary card on/off (settings, left/right/Enter).
    ToggleSummary,

    ExcludeOpen,
    ExcludeClose,
    ExcludeNext,
    ExcludePrev,
    ExcludeStartAdd,
    ExcludeDelete,
    ExcludeInputKey(crossterm::event::KeyEvent),
    ExcludeConfirm,
    ExcludeCancelAdd,
}

#[derive(Debug)]
pub enum SummaryAction {
    /// Kick the Agents-tab summary generation (Generate button click).
    Generate,
    /// Cancel an in-flight generation (Esc on the Agents tab while
    /// `Generating`, or a cancel click): kills the `claude` child and
    /// restores the prior card state. No-op when not generating.
    Cancel,
    /// Scroll the Agents-tab summary text by a row delta (wheel over card).
    Scroll(i32),
    /// Open the summary "big view" popup (popup button click).
    OpenPopup,
    /// Close the summary popup (Esc / click outside).
    ClosePopup,
    /// Scroll the summary popup text by a row delta.
    ScrollPopup(i32),
    /// Begin dragging the summary card's bottom edge to resize it.
    StartDrag,
    /// Set the summary card body height (rows) mid-drag.
    Resize(u16),
    /// Finish the summary resize drag and persist the new height.
    StopDrag,
    /// Open the generated-summary language editor (settings input box).
    OpenLanguageEditor,
    /// Forward a key to the language editor's input field.
    LanguageInputKey(crossterm::event::KeyEvent),
    /// Confirm the typed language and persist it.
    LanguageConfirm,
    /// Discard the language edit.
    LanguageCancel,
}

#[derive(Debug)]
pub enum NewSessionAction {
    Close,
    InputKey(crossterm::event::KeyEvent),
    Confirm,
    Prev,
    Next,
    Clear,
    DeleteSegment,
    SwitchFocus,
    DirUp,
    DirEnter,
}

#[derive(Debug)]
pub enum MenuAction {
    OpenSession {
        target: crate::state::FocusTarget,
        x: u16,
        y: u16,
    },
    OpenGlobal {
        x: u16,
        y: u16,
    },
    /// Open the `@local` divider's `[…]` menu (local "New session";
    /// Port Forward / Remove from list greyed out).
    OpenLocalDivider {
        x: u16,
        y: u16,
    },
    OpenHostDivider {
        host: String,
        x: u16,
        y: u16,
    },
    Next,
    Prev,
    Confirm,
    Dismiss,
    Hover(usize),
    ClickItem(usize),
}

#[derive(Debug)]
pub enum PfAction {
    Open(String),
    Close,
    FocusUp,
    FocusDown,
    Delete,
    AddOpen,
    AddCancel,
    AddSubmit,
    AddFieldNext,
    AddFieldPrev,
    AddModeLeft,
    AddModeRight,
    /// Forward a raw key event to the focused textarea (insert/delete/
    /// arrow within a field). Modal keys (Tab/Enter/Esc/Up/Down/etc.)
    /// are mapped to their own actions before reaching this variant.
    AddInputKey(crossterm::event::KeyEvent),
    TaskResult {
        host: String,
        op: crate::app::port_forward_task::OpKind,
        ok: bool,
        message: String,
    },
    ProbeResult {
        key: crate::state::ForwardKey,
        health: crate::state::ForwardHealth,
    },
}

#[derive(Debug)]
pub enum AddRemoteAction {
    InputKey(crossterm::event::KeyEvent),
    Next,
    Prev,
    Confirm,
    Close,
}

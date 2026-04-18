mod keyboard;
mod mouse;
mod reduce;

use crate::state::FilterMode;

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
    StartRename,
    RenameInput(char),
    RenameBackspace,
    RenameConfirm,
    RenameCancel,

    ToggleLayout,
    ToggleBorders,
    ToggleViewMode,
    OpenSettings,
    CloseSettings,
    SettingsNext,
    SettingsPrev,
    SettingsAdjust(i32),
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
    ExcludeEditorInput(char),
    ExcludeEditorBackspace,
    ExcludeEditorConfirm,
    ExcludeEditorCancelAdd,

    ToggleHelp,
    DismissHelp,

    CycleFilter,
    SetFilter(FilterMode),

    SetFocusMain,
    SetFocusSidebar,
    ToggleFocus,

    OpenSessionMenu { filtered_idx: usize, x: u16, y: u16 },
    OpenGlobalMenu { x: u16, y: u16 },
    MenuNext,
    MenuPrev,
    MenuConfirm,
    MenuDismiss,
    MenuHover(usize),
    MenuClickItem(usize),

    SidebarClickSession(usize),
    NumberKeyJump(usize),

    ResizeSidebar(u16),
    ResizeSidebarHeight(u16),
    StartDrag,
    StopDrag,
    SetHoverSeparator(bool),

    Resize(u16, u16),

    ForwardKey(Vec<u8>),
    ForwardMouse(Vec<u8>),

    ActivatePlugin(usize),
    DeactivatePlugin,

    Quit,

    None,
}

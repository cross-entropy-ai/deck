//! Transient overlay state — the `Modal` priority enum, the grouped
//! `OverlayState`, and the small per-overlay states (rename, exclude
//! editor, update warning).

use ratatui_textarea::TextArea;

use crate::forwards::PortForwardOverlay;
use crate::menu::ContextMenu;
use crate::new_session::{make_textarea, textarea_line, NewSessionState};

/// The full-input modal overlays, in the one priority order rendering and both
/// input mappers consult. `AppState::active_modal` resolves the highest-
/// priority active overlay; the renderer paints only that modal, while keyboard
/// and mouse mappers route to it before any global keybinding or button-rect
/// test. One visible modal therefore owns all input behind it (bug #7).
/// NOTE: not the update-warning popup, which is a selective gate
/// (`App::warning_state` + `warning_blocks_action`) and stays out of this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    SummaryPopup,
    NewSession,
    AddRemote,
    Rename,
    ContextMenu,
    PortForward,
    ThemePicker,
    KeybindingsView,
    ExcludeEditor,
    SummaryLang,
    Help,
    ConfirmKill,
}

impl Modal {
    /// Every formal modal, in the same priority order as
    /// [`AppState::active_modal`](crate::state::AppState::active_modal).
    ///
    /// Keeping the inventory on the type lets exhaustive tests and shared
    /// modal infrastructure iterate it without maintaining a second hand-
    /// written list that can drift when a variant is added.
    #[cfg(test)]
    pub const ALL: [Self; 12] = [
        Self::SummaryPopup,
        Self::NewSession,
        Self::AddRemote,
        Self::Rename,
        Self::ContextMenu,
        Self::PortForward,
        Self::ThemePicker,
        Self::KeybindingsView,
        Self::ExcludeEditor,
        Self::SummaryLang,
        Self::Help,
        Self::ConfirmKill,
    ];
}

/// UI state for an in-progress rename.
#[derive(Debug, Clone)]
pub struct RenameState {
    pub original_name: String,
    pub input: TextArea<'static>,
    /// Stable routing identity retained while the overlay is open.
    pub lane: crate::lane::LaneId,
}

impl RenameState {
    pub fn new_with_lane(
        original_name: String,
        initial: String,
        lane: crate::lane::LaneId,
    ) -> Self {
        Self {
            original_name,
            input: crate::new_session::make_textarea(&initial),
            lane,
        }
    }
}

/// UI state for the exclude pattern editor popup.
#[derive(Debug, Clone)]
pub struct ExcludeEditorState {
    pub selected: usize,
    pub adding: bool,
    pub input: TextArea<'static>,
    pub error: Option<String>,
}

impl ExcludeEditorState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            adding: false,
            input: make_textarea(""),
            error: None,
        }
    }

    /// Read current add-input text.
    pub fn input_str(&self) -> &str {
        textarea_line(&self.input)
    }

    /// Reset the add input to empty (called on StartAdd / CancelAdd / Confirm).
    pub fn reset_input(&mut self) {
        self.input = make_textarea("");
    }
}

/// Modal warning banner over the main pane, used by the self-update flow
/// ("can't self-update from here" / "unsupported platform"). Lives on `App`
/// (`warning_state: Option<WarningState>`), not `OverlayState`, because the
/// dispatch loop's "block actions while a warning is up" gate reads it from
/// App directly.
#[derive(Clone)]
pub struct WarningState {
    pub text: &'static str,
    pub detail: String,
}

/// UI state for transient sidebar overlays — help, kill-confirm, rename,
/// context menu, exclude-pattern editor. Grouped so renderer and key
/// dispatcher have one place to ask "is any overlay active?".
#[derive(Debug, Default)]
pub struct OverlayState {
    pub show_help: bool,
    pub confirm_kill: bool,
    pub renaming: Option<RenameState>,
    pub context_menu: Option<ContextMenu>,
    pub exclude_editor: Option<ExcludeEditorState>,
    pub new_session: Option<NewSessionState>,
    pub add_remote: Option<crate::add_remote::AddRemoteState>,
    /// Port-forward overlay for a single host. See `PortForwardOverlay`.
    pub port_forward: Option<PortForwardOverlay>,
    /// The Agents-tab summary "big view" popup is open.
    pub summary_popup: bool,
    /// Settings input box for the generated-summary language (free text).
    pub summary_lang_input: Option<TextArea<'static>>,
}

//! Transient overlay state — the `Modal` priority enum, the grouped
//! `OverlayState`, and the small per-overlay states (rename, exclude
//! editor, update warning).

use ratatui_textarea::TextArea;

use crate::forwards::PortForwardOverlay;
use crate::menu::ContextMenu;
use crate::new_session::{make_textarea, textarea_line, NewSessionState};

/// The full-input modal overlays, in the one order both input mappers
/// consult them. `AppState::active_modal` resolves the highest-priority
/// active overlay; the keyboard and mouse mappers route to it *before* any
/// global keybinding or button-rect hit test, so a modal swallows every
/// input behind it (bug #7). The single ordering here is the source of
/// truth — do not re-derive it in either mapper.
///
/// NOTE: this is *not* the update-warning popup. That is a selective gate
/// (`App::warning_state` + `warning_blocks_action`) that blocks only a few
/// actions, not a swallow-everything modal, and stays out of this enum.
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

/// UI state for an in-progress rename.
#[derive(Debug, Clone)]
pub struct RenameState {
    pub original_name: String,
    pub input: TextArea<'static>,
    /// `Some(host)` when the rename targets a remote session.
    pub host: Option<String>,
}

impl RenameState {
    pub fn new(original_name: String, initial: String, host: Option<String>) -> Self {
        let mut ta = TextArea::new(vec![initial]);
        ta.move_cursor(ratatui_textarea::CursorMove::End);
        Self {
            original_name,
            input: ta,
            host,
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

/// Modal warning banner shown over the main pane. Used by the
/// self-update flow to surface "can't self-update from here" /
/// "unsupported platform" messages. Lives on `App` (as
/// `warning_state: Option<WarningState>`) rather than in `OverlayState`
/// because the dispatch loop's "block actions while a warning is up"
/// gate reads it from App directly.
#[derive(Clone)]
pub enum WarningState {
    Proactive { text: &'static str, detail: String },
}

/// UI state for transient sidebar overlays — help screen, kill-confirm
/// prompt, in-progress rename, right-click context menu, and the
/// exclude-pattern editor popup. Grouped so the renderer and key
/// dispatcher have a single place to ask "is any overlay active?".
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

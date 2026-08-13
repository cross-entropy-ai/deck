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
    MountPicker,
    SshSetting,
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
    pub const ALL: [Self; 14] = [
        Self::SummaryPopup,
        Self::NewSession,
        Self::AddRemote,
        Self::Rename,
        Self::ContextMenu,
        Self::PortForward,
        Self::ThemePicker,
        Self::KeybindingsView,
        Self::ExcludeEditor,
        Self::MountPicker,
        Self::SshSetting,
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

/// The "mount another lane under this one" picker: a filter over the candidates
/// a system discovered, plus the async states that discovery and activation put
/// it in. Rendering lives in `ui/overlays/mounts.rs`.
///
/// `generation` is stamped on every request so a late worker answer for a picker
/// the user has already closed or re-pointed is dropped instead of repopulating
/// a stale list.
#[derive(Debug, Clone)]
pub struct MountPickerState {
    /// The lane whose mounts these are.
    pub lane: crate::lane::LaneId,
    pub generation: u64,
    /// Labels are the picker items; `candidates` stays index-aligned with
    /// `picker.items` so a selection resolves back to a backend id.
    pub picker: crate::picker::FilterPicker,
    pub candidates: Vec<crate::system::MountCandidate>,
    /// Set while a worker is out; the list shows a placeholder rather than
    /// "nothing found".
    pub busy: Option<MountBusy>,
    /// A candidate that needs a side effect before it can be mounted, awaiting
    /// the user's confirmation. Deck will change something outside itself here
    /// (start someone's container on a shared host), so it never happens on a
    /// single keypress.
    pub confirming: Option<crate::system::MountCandidate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountBusy {
    Discovering,
    Activating,
}

impl MountPickerState {
    pub fn new(lane: crate::lane::LaneId, generation: u64) -> Self {
        Self {
            lane,
            generation,
            picker: crate::picker::FilterPicker::new(Vec::new()),
            candidates: Vec::new(),
            busy: Some(MountBusy::Discovering),
            confirming: None,
        }
    }

    /// Replace the candidate list, keeping labels and candidates aligned.
    pub fn set_candidates(&mut self, candidates: Vec<crate::system::MountCandidate>) {
        self.picker =
            crate::picker::FilterPicker::new(candidates.iter().map(|c| c.label.clone()).collect());
        self.candidates = candidates;
        self.busy = None;
    }

    /// The highlighted candidate. Resolved through `filtered` so it survives
    /// filtering, which reorders nothing but hides entries.
    pub fn selected(&self) -> Option<&crate::system::MountCandidate> {
        let index = *self.picker.filtered.get(self.picker.selected)?;
        self.candidates.get(index)
    }

    pub fn refilter(&mut self) {
        let needle = self.picker.input_str().to_lowercase();
        self.picker.refilter(move |items, _| {
            items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.to_lowercase().contains(&needle))
                .map(|(index, _)| index)
                .collect()
        });
    }
}

/// Which Deck-owned OpenSSH value a Settings text editor is changing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshSettingField {
    ControlPath,
    ControlPersist,
}

#[derive(Debug, Clone)]
pub struct SshSettingEditorState {
    pub field: SshSettingField,
    pub input: TextArea<'static>,
    pub error: Option<String>,
}

impl SshSettingEditorState {
    pub fn new(field: SshSettingField, value: &str) -> Self {
        Self {
            field,
            input: make_textarea(value),
            error: None,
        }
    }

    pub fn input_str(&self) -> &str {
        textarea_line(&self.input)
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
    /// Settings input box for Deck's ControlPath or ControlPersist value.
    pub ssh_setting_editor: Option<SshSettingEditorState>,
    /// Picker over the lanes a system says the focused lane could mount.
    pub mount_picker: Option<MountPickerState>,
}

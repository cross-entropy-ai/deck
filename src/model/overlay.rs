//! Transient overlay state — the `Modal` priority enum, the grouped
//! `OverlayState`, and the small per-overlay states (rename, exclude
//! editor, update warning).

use ratatui_textarea::TextArea;

use crate::forwards::PortForwardOverlay;
use crate::menu::ContextMenu;
use crate::new_session::{make_textarea, textarea_line, NewSessionState};

/// Declares the formal modals once, **in priority order**, and derives both the
/// [`Modal`] enum and [`Modal::PRIORITY`] from that one list. A new modal is one
/// line here; there is no second inventory to forget.
macro_rules! modals {
    ($($variant:ident),* $(,)?) => {
        /// The full-input modal overlays, in the one priority order rendering and
        /// both input mappers consult. [`AppState::active_modal`] resolves the
        /// highest-priority open overlay; the renderer paints only that modal,
        /// while keyboard and mouse mappers route to it before any global
        /// keybinding or button-rect test. One visible modal therefore owns all
        /// input behind it (bug #7).
        ///
        /// NOTE: not the update-warning popup, which is a selective gate
        /// (`App::warning_state` + `warning_blocks_action`) and stays out of this
        /// enum.
        ///
        /// [`AppState::active_modal`]: crate::state::AppState::active_modal
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Modal {
            $($variant),*
        }

        impl Modal {
            /// Every formal modal, highest priority first — the single
            /// inventory. `active_modal` walks it; exhaustive tests iterate it.
            /// Generated with the enum, so the two cannot drift.
            pub const PRIORITY: &'static [Self] = &[$(Self::$variant),*];
        }
    };
}

modals! {
    // First, so it outranks everything: the prompt arrives unbidden from the
    // network, and a modal that is open but neither painted nor routed leaves
    // the device waiting on an answer nobody can give.
    BuddyApprove,
    SummaryPopup,
    NewSession,
    AddRemote,
    MountPicker,
    HiddenSessions,
    Rename,
    ContextMenu,
    PortForward,
    ThemePicker,
    KeybindingsView,
    ExcludeEditor,
    SshSetting,
    SummaryLang,
    Help,
    ConfirmKill,
}

/// The peer the Buddy connection prompt is asking about.
#[derive(Debug, Clone)]
pub struct BuddyApproveState {
    pub peer: std::net::IpAddr,
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

/// The "restore a hidden session" picker for one lane: a filter over the names
/// that lane is currently not capturing. Rendering reuses the shared filter
/// picker, so it reads as the same idiom as Add Remote.
///
/// The names are a snapshot taken when the picker opened, not a live view of
/// `AppState::hidden_sessions`: restoring one shortens the list, and a list that
/// re-sorted itself under the cursor between two clicks would restore the wrong
/// session. The reducer removes from both.
#[derive(Debug, Clone)]
pub struct HiddenSessionsState {
    pub lane: crate::lane::LaneId,
    pub picker: crate::picker::FilterPicker,
}

impl HiddenSessionsState {
    /// Open over `names`, sorted so the list has a stable order across opens.
    pub fn new(lane: crate::lane::LaneId, names: &std::collections::HashSet<String>) -> Self {
        let mut names: Vec<String> = names.iter().cloned().collect();
        names.sort();
        Self {
            lane,
            picker: crate::picker::FilterPicker::new(names),
        }
    }

    /// The highlighted name, or `None` when the filter matched nothing.
    pub fn selected_name(&self) -> Option<&str> {
        self.picker.selected_item()
    }

    /// Drop `name` from the open list, keeping the cursor in range.
    pub fn forget(&mut self, name: &str) {
        self.picker.items.retain(|item| item != name);
        self.refilter();
    }

    fn refilter(&mut self) {
        self.picker.refilter_substring();
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
    /// The order the candidate list is in. Carried in from `AppState` so the
    /// choice outlives one picker, and applied to `candidates` rather than to a
    /// view of it, keeping `picker.items` index-aligned by construction.
    pub sort: MountSort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountBusy {
    Discovering,
    Activating,
}

/// How the mount picker orders what a system discovered.
///
/// Backends report discovery order (for containers, whatever `docker ps` felt
/// like), which is not an order anyone can predict, so the picker imposes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MountSort {
    /// Mountable candidates first, then by name. The default, because a
    /// candidate needing activation costs a side effect outside Deck and a
    /// confirmation, so it is the one you less often mean.
    #[default]
    ReadyFirst,
    /// Name order, ignoring readiness — better when you know what you want and
    /// the list is long enough that grouping just moves it around.
    Name,
}

impl MountSort {
    /// The next order in the cycle. Two orders today, so this is a toggle; a
    /// cycle keeps the call sites honest if a third is added.
    pub fn next(self) -> Self {
        match self {
            Self::ReadyFirst => Self::Name,
            Self::Name => Self::ReadyFirst,
        }
    }

    /// What the picker says the current order is.
    ///
    /// Worded for containers ("running"), though the rule it describes is the
    /// generic `needs_activation`. Same known compromise as the `Containers…`
    /// menu label: when a second mount provider exists, both move into the
    /// provider so each names its own candidates.
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadyFirst => "running first, then name",
            Self::Name => "name",
        }
    }

    /// Order `candidates` by this rule.
    ///
    /// Names are compared case-insensitively so `Web` and `api` don't split
    /// into separate alphabets, and ties fall back to the id, which is unique
    /// per candidate — without that, two containers with the same name on
    /// different engines would swap places between re-sorts.
    pub fn apply(self, candidates: &mut [crate::system::MountCandidate]) {
        candidates.sort_by(|a, b| {
            let name = |c: &crate::system::MountCandidate| c.label.to_lowercase();
            match self {
                Self::ReadyFirst => (a.needs_activation, name(a), a.id.clone()).cmp(&(
                    b.needs_activation,
                    name(b),
                    b.id.clone(),
                )),
                Self::Name => (name(a), a.id.clone()).cmp(&(name(b), b.id.clone())),
            }
        });
    }
}

impl MountPickerState {
    pub fn new(lane: crate::lane::LaneId, generation: u64, sort: MountSort) -> Self {
        Self {
            lane,
            generation,
            picker: crate::picker::FilterPicker::new(Vec::new()),
            candidates: Vec::new(),
            busy: Some(MountBusy::Discovering),
            confirming: None,
            sort,
        }
    }

    /// Replace the candidate list, sorted, keeping labels and candidates
    /// aligned.
    ///
    /// The filter text survives: discovery is an ssh round trip, so the user
    /// can well have started typing before the answer lands, and throwing that
    /// away would look like dropped keystrokes.
    pub fn set_candidates(&mut self, mut candidates: Vec<crate::system::MountCandidate>) {
        self.sort.apply(&mut candidates);
        self.rebuild(candidates, None);
        self.busy = None;
    }

    /// Re-order the current candidates. The highlight follows its candidate
    /// rather than its row: the set didn't change, only the order, so keeping
    /// the row would silently point at something else.
    pub fn resort(&mut self, sort: MountSort) {
        self.sort = sort;
        let anchor = self.selected().map(|candidate| candidate.id.clone());
        let mut candidates = std::mem::take(&mut self.candidates);
        self.sort.apply(&mut candidates);
        self.rebuild(candidates, anchor);
    }

    /// Install `candidates` as the list, rebuilding the index-aligned labels
    /// and re-deriving the filter, then put the highlight back on `anchor`.
    fn rebuild(&mut self, candidates: Vec<crate::system::MountCandidate>, anchor: Option<String>) {
        let input = std::mem::replace(&mut self.picker.input, make_textarea(""));
        self.picker =
            crate::picker::FilterPicker::new(candidates.iter().map(|c| c.label.clone()).collect());
        self.picker.input = input;
        self.candidates = candidates;
        self.refilter();
        if let Some(anchor) = anchor {
            let row = self
                .picker
                .filtered
                .iter()
                .position(|&index| self.candidates[index].id == anchor);
            if let Some(row) = row {
                self.picker.selected = row;
            }
        }
    }

    /// The highlighted candidate. Resolved through `filtered` so it survives
    /// filtering, which reorders nothing but hides entries.
    pub fn selected(&self) -> Option<&crate::system::MountCandidate> {
        let index = *self.picker.filtered.get(self.picker.selected)?;
        self.candidates.get(index)
    }

    fn refilter(&mut self) {
        self.picker.refilter_substring();
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

/// Declares the modal payloads once and derives the [`ModalState`] enum, its
/// [`kind`](ModalState::kind) discriminant, and the typed accessors on
/// [`OverlayState`] from that one list.
///
/// A variant with `(Type) => getter / getter_mut` carries state, plus an
/// optional `/ taker` for the ones a caller has to move out of. A bare variant
/// is a modal that is either up or not.
macro_rules! modal_states {
    ($(
        $(#[$attr:meta])*
        $variant:ident $(($ty:ty) => $get:ident / $get_mut:ident $(/ $take:ident)?)?
    ),* $(,)?) => {
        /// The one modal Deck is showing, and its state.
        ///
        /// One slot, not thirteen flags: two modals open at once was
        /// representable before this and had to be resolved at read time by a
        /// priority chain. Here it cannot be built.
        ///
        /// [`Modal`] is this type's discriminant. The two variants `Modal` has
        /// and this does not — `ThemePicker` and `KeybindingsView` — are
        /// settings-page sub-popovers whose cursor and scroll live in
        /// `SettingsState` alongside the page they belong to.
        // The variants differ in size by design: a picker carries a filter and
        // its items, a confirmation carries nothing. Boxing the big ones would
        // trade one pointer chase per access for memory this type no longer
        // spends — the thirteen `Option` fields this replaced held every payload
        // side by side, 8,216 bytes against 2,360 here.
        #[allow(clippy::large_enum_variant)]
        #[derive(Debug)]
        pub enum ModalState {
            $($(#[$attr])* $variant $(($ty))?),*
        }

        impl ModalState {
            /// Which modal this is, dropping the state.
            pub fn kind(&self) -> Modal {
                match self {
                    $(Self::$variant { .. } => Modal::$variant),*
                }
            }
        }

        impl OverlayState {
            $($(
                #[doc = concat!(
                    "The open [`ModalState::", stringify!($variant), "`]'s state, or \
                     `None` when something else (or nothing) is open."
                )]
                pub fn $get(&self) -> Option<&$ty> {
                    match &self.modal {
                        Some(ModalState::$variant(state)) => Some(state),
                        _ => None,
                    }
                }

                #[doc = concat!(
                    "Mutable [`ModalState::", stringify!($variant), "`] state, or \
                     `None` when something else (or nothing) is open."
                )]
                pub fn $get_mut(&mut self) -> Option<&mut $ty> {
                    match &mut self.modal {
                        Some(ModalState::$variant(state)) => Some(state),
                        _ => None,
                    }
                }

                $(
                    #[doc = concat!(
                        "Close [`ModalState::", stringify!($variant), "`] and hand \
                         back its state. `None`, and nothing closed, if it was not \
                         the open one."
                    )]
                    pub fn $take(&mut self) -> Option<$ty> {
                        match self.modal.take() {
                            Some(ModalState::$variant(state)) => Some(state),
                            // Not ours: put back what we took.
                            other => {
                                self.modal = other;
                                None
                            }
                        }
                    }
                )?
            )?)*
        }
    };
}

modal_states! {
    /// "May this device drive your Mac?", raised by the Buddy server. Carries
    /// only what the prompt shows: the reply channel stays on `App`, since
    /// nothing in `model` owns an IO handle.
    BuddyApprove(BuddyApproveState) => buddy_approve / _buddy_approve_mut,
    /// The Agents-tab summary "big view" popup.
    SummaryPopup,
    NewSession(NewSessionState) => new_session / new_session_mut,
    AddRemote(crate::add_remote::AddRemoteState) => add_remote / add_remote_mut,
    /// Picker over the lanes a system says the focused lane could mount.
    MountPicker(MountPickerState) => mount_picker / mount_picker_mut,
    HiddenSessions(HiddenSessionsState) => hidden_sessions / hidden_sessions_mut / take_hidden_sessions,
    Rename(RenameState) => renaming / renaming_mut / take_renaming,
    ContextMenu(ContextMenu) => context_menu / context_menu_mut / take_context_menu,
    /// Port-forward overlay for a single lane. See `PortForwardOverlay`.
    PortForward(PortForwardOverlay) => port_forward / port_forward_mut,
    ExcludeEditor(ExcludeEditorState) => exclude_editor / exclude_editor_mut,
    /// Settings input box for Deck's ControlPath or ControlPersist value.
    SshSetting(SshSettingEditorState) => ssh_setting_editor / ssh_setting_editor_mut / take_ssh_setting_editor,
    /// Settings input box for the generated-summary language (free text).
    SummaryLang(TextArea<'static>) => summary_lang_input / summary_lang_input_mut / take_summary_lang_input,
    Help,
    ConfirmKill,
}

/// The transient overlay Deck is showing, if any.
///
/// Exactly one modal is up at a time — the field is private so that stays true
/// — and every reader reaches its state through the generated accessors, which
/// answer `None` unless *that* modal is the open one.
#[derive(Debug, Default)]
pub struct OverlayState {
    modal: Option<ModalState>,
}

impl OverlayState {
    /// Which modal is up, if any. `AppState::active_modal` is the routing
    /// answer — it also weighs the settings-page sub-popovers and their focus
    /// gate — so prefer that outside this module.
    pub fn kind(&self) -> Option<Modal> {
        self.modal.as_ref().map(ModalState::kind)
    }

    /// Whether `kind` is the modal currently up.
    pub fn is(&self, kind: Modal) -> bool {
        self.kind() == Some(kind)
    }

    /// Whether `kind` is *not* up — a different modal, or none at all.
    pub fn is_not(&self, kind: Modal) -> bool {
        !self.is(kind)
    }

    /// Show `modal`, replacing whatever was up.
    pub fn open(&mut self, modal: ModalState) {
        self.modal = Some(modal);
    }

    /// Close `kind` if it is what is up. A no-op otherwise, so a path tidying
    /// up after one overlay cannot dismiss an unrelated one that opened since.
    pub fn close(&mut self, kind: Modal) {
        if self.is(kind) {
            self.modal = None;
        }
    }
}

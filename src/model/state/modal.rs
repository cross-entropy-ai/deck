//! Which overlay is up, and where a click lands inside it.
//!
//! [`AppState::active_modal`] is the one answer the renderer and both input
//! mappers consult, so priority and the settings-page gate live here instead
//! of being re-derived per caller. `menu_item_at` keeps it company: the same
//! question one level down, for the open context menu.

use super::*;

impl AppState {
    /// The highest-priority full-input modal currently open, or `None` when the
    /// sidebar/PTY takes input directly. [`Modal::PRIORITY`] is the source of
    /// truth for the order; the modal renderer plus both input mappers consult
    /// this first, so priority there decides which single overlay is visible and
    /// swallows a key/click when several backing flags are set.
    pub fn active_modal(&self) -> Option<Modal> {
        Modal::PRIORITY
            .iter()
            .copied()
            .find(|modal| modal.is_open(self))
    }

    /// Map a screen position to a context menu item index.
    pub fn menu_item_at(&self, col: u16, row: u16) -> Option<usize> {
        let menu = self.overlay.context_menu()?;
        let items = menu.items();
        // Same rect the renderer draws into (`ui::menu::draw_context_menu`).
        let r = context_menu_rect(&items, menu.x, menu.y, self.term_width, self.term_height);
        if r.width < 2 || r.height < 2 {
            return None;
        }
        // Interior only: clicks on the border select nothing.
        if col > r.x && col < r.x + r.width - 1 && row > r.y && row < r.y + r.height - 1 {
            let idx = (row - r.y - 1) as usize;
            if idx < items.len() {
                return Some(idx);
            }
        }
        None
    }

    // --- Focus clamping and ordering ---
}

impl Modal {
    /// Whether this modal's backing state is currently open.
    ///
    /// Exhaustive by construction: a new [`Modal`] variant does not compile
    /// until it says how to tell whether it is showing. That is the check the
    /// old hand-written if-chain could not give — a forgotten branch there just
    /// made the modal invisible to input routing.
    ///
    /// The settings sub-modals (KeybindingsView / ExcludeEditor / SshSetting /
    /// SummaryLang) count only while the settings page owns focus
    /// (`MainView::Settings` + `FocusMode::Main`); elsewhere their backing
    /// fields are stale and must not gate input. The theme picker is not gated:
    /// it is reachable as a standalone overlay.
    fn is_open(self, state: &AppState) -> bool {
        let on_settings_page =
            state.main_view == MainView::Settings && state.focus_mode == FocusMode::Main;
        match self {
            // The settings page's own sub-popovers keep their cursor and scroll
            // in `SettingsState`, so they answer from there.
            Self::ThemePicker => state.settings.theme_picker_open,
            Self::KeybindingsView => on_settings_page && state.settings.keybindings_view_open,
            // In the modal slot, but reachable only from the settings page: the
            // state outlives a focus change, so the gate is what makes it inert.
            Self::ExcludeEditor | Self::SshSetting | Self::SummaryLang => {
                on_settings_page && state.overlay.is(self)
            }
            // In the modal slot, openable from anywhere. Spelled out rather
            // than caught by `_`, so adding a modal still has to answer here.
            Self::SummaryPopup
            | Self::NewSession
            | Self::AddRemote
            | Self::MountPicker
            | Self::HiddenSessions
            | Self::Rename
            | Self::ContextMenu
            | Self::PortForward
            | Self::Help
            | Self::ConfirmKill => state.overlay.is(self),
        }
    }
}

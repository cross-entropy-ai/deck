//! Reducers for the settings page and the four overlays it opens: the theme
//! picker, the keybindings viewer, the SSH-value editor, and the
//! exclude-pattern editor.
//!
//! Each overlay is a [`Modal`] in its own right, so each gets its own action
//! enum and its own reducer here rather than another block inside
//! `reduce_settings`. They share this file because they share the settings
//! page's state and vocabulary, not because they are the same thing.

use crate::app::settings::setting_rows;
use crate::bounds::step_clamped;
use crate::effects::{Effect, SideEffect};
use crate::new_session::textarea_input;
use crate::overlay::{Modal, ModalState};
use crate::state::{AppState, FocusMode, MainView};
use crate::theme::indices_for_slot;

use super::{
    apply_action, ExcludeAction, KeybindingsAction, SettingsAction, SshSettingAction,
    ThemePickerAction,
};

pub(super) fn reduce_settings(state: &mut AppState, action: SettingsAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        SettingsAction::Open => {
            state.main_view = MainView::Settings;
            state.focus_mode = FocusMode::Main;
            state.settings.reset_pages();
            state.settings.theme_picker_open = false;
            state.settings.theme_picker_selected = state.prefs.theme_index;
            state.overlay.close(Modal::SshSetting);
        }
        SettingsAction::Close => {
            state.main_view = MainView::Terminal;
            state.focus_mode = FocusMode::Main;
            state.settings.reset_pages();
            state.settings.theme_picker_open = false;
            state.overlay.close(Modal::SshSetting);
        }
        SettingsAction::OpenPage(page) => {
            state.settings.push_page(page);
            // Granting Accessibility takes effect without a restart, so the
            // cached answer is stale until re-asked.
            if page == crate::state::SettingsPage::Buddy {
                fx.push(crate::effects::Effect::RefreshBuddyTrust);
            }
        }
        SettingsAction::Back => {
            state.settings.pop_page();
        }
        SettingsAction::Next => {
            let total = setting_rows(state).len();
            let selected = step_clamped(state.settings.selected(), total, 1);
            state.settings.set_selected(selected);
        }
        SettingsAction::Prev => {
            let total = setting_rows(state).len();
            let selected = step_clamped(state.settings.selected(), total, -1);
            state.settings.set_selected(selected);
        }
        SettingsAction::Adjust => {
            // Look up the selected row and fire its adjust — the row, not a
            // positional match, is the source of truth.
            let selected = state.settings.selected();
            let inner_action = setting_rows(state).get(selected).map(|row| (row.adjust)());
            if let Some(inner_action) = inner_action {
                let inner = apply_action(state, inner_action);
                fx.merge(inner);
            }
        }
        SettingsAction::CycleFrameRateLimit(direction) => {
            state.prefs.cycle_frame_rate_limit(direction);
            fx.save_config();
        }
        SettingsAction::CycleAgentsProbeInterval(direction) => {
            state.prefs.cycle_agents_probe_interval(direction);
            fx.save_config();
        }
        SettingsAction::CycleSessionHighlight(direction) => {
            state.prefs.cycle_session_highlight(direction);
            fx.save_config();
        }
        SettingsAction::CycleSummaryAgent(direction) => {
            state.prefs.cycle_summary_agent(direction);
            fx.save_config();
        }
        SettingsAction::ToggleBuddy => {
            state.prefs.buddy_enabled = !state.prefs.buddy_enabled;
            // The listener follows the switch immediately; the save is what
            // makes it survive a restart.
            fx.push(crate::effects::Effect::ReconfigureBuddy);
            fx.save_config();
        }
        SettingsAction::ToggleSummary => {
            state.prefs.summary_enabled = !state.prefs.summary_enabled;
            fx.save_config();
        }
        SettingsAction::ToggleSshConnectionReuse => {
            state.prefs.ssh_connection_reuse = !state.prefs.ssh_connection_reuse;
            if !state.prefs.ssh_connection_reuse {
                state.overlay.close(Modal::PortForward);
            }
            fx.save_config();
        }
        SettingsAction::OpenAddRemotePicker => fx.push(Effect::OpenAddRemotePicker),
        // One aggregate row for every host — it opens the first host
        // that has forwards (else the first host); per-host editing stays on
        // each `@host` divider's `[⇄N]` badge button.
        SettingsAction::OpenPortForwards => fx.push(Effect::OpenConfiguredPortForwards),
        SettingsAction::ToggleUpdateCheck => {
            use crate::update::UpdateCheckMode::{Disabled, Enabled};
            let was_enabled = state.prefs.update_check_mode == Enabled;
            state.prefs.update_check_mode = if was_enabled { Disabled } else { Enabled };
            if was_enabled {
                state.update_available = None;
            }
            fx.save_config();
        }
    }
    fx
}

/// The theme picker. Opening it does not enter the settings page and closing
/// it does not leave wherever it was opened from, so `main_view` and
/// `focus_mode` are deliberately untouched throughout.
pub(super) fn reduce_theme_picker(state: &mut AppState, action: ThemePickerAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        ThemePickerAction::Open(slot) => {
            // Opens as a standalone overlay over the current view: from the
            // sidebar (`t`) it doesn't enter the settings page, from settings
            // it layers on top. Leaving `main_view`/`focus_mode` untouched lets
            // closing the picker return to wherever it was opened from.
            state.settings.theme_picker_open = true;
            state.settings.theme_picker_slot = slot;
            let current = state.prefs.theme_slot(slot);
            state.settings.theme_picker_selected = indices_for_slot(slot)
                .position(|index| index == current)
                .unwrap_or(0);
        }
        ThemePickerAction::ToggleAuto => {
            state.prefs.theme_auto = !state.prefs.theme_auto;
            fx.save_config();
            // Re-ask on the way in: the terminal may have flipped appearance
            // since startup, and if auto was off we ignored any report so far.
            if state.prefs.theme_auto {
                fx.push(Effect::QueryColorScheme);
            }
            fx.push(Effect::ApplyTmuxTheme);
        }
        ThemePickerAction::Close => {
            state.settings.theme_picker_open = false;
        }
        ThemePickerAction::Next => {
            let slot = state.settings.theme_picker_slot;
            let available: Vec<usize> = indices_for_slot(slot).collect();
            state.settings.theme_picker_selected =
                step_clamped(state.settings.theme_picker_selected, available.len(), 1);
            if let Some(&theme_index) = available.get(state.settings.theme_picker_selected) {
                state.prefs.set_theme_slot(slot, theme_index);
            }
            fx.save_config();
            fx.push(Effect::ApplyTmuxTheme);
        }
        ThemePickerAction::Prev => {
            // Side effects only fire when the cursor actually moves (unlike
            // Next, which always re-applies) — preserve that asymmetry.
            if state.settings.theme_picker_selected > 0 {
                let slot = state.settings.theme_picker_slot;
                let available: Vec<usize> = indices_for_slot(slot).collect();
                state.settings.theme_picker_selected =
                    step_clamped(state.settings.theme_picker_selected, available.len(), -1);
                if let Some(&theme_index) = available.get(state.settings.theme_picker_selected) {
                    state.prefs.set_theme_slot(slot, theme_index);
                }
                fx.save_config();
                fx.push(Effect::ApplyTmuxTheme);
            }
        }
        ThemePickerAction::Confirm => {
            state.settings.theme_picker_open = false;
        }
    }
    fx
}

/// The read-only keybindings viewer.
pub(super) fn reduce_keybindings(state: &mut AppState, action: KeybindingsAction) -> SideEffect {
    let fx = SideEffect::default();
    match action {
        KeybindingsAction::Open => {
            state.settings.keybindings_view_open = true;
            state.settings.keybindings_view_scroll = 0;
        }
        KeybindingsAction::Close => {
            state.settings.keybindings_view_open = false;
        }
        KeybindingsAction::ScrollUp => {
            state.settings.keybindings_view_scroll =
                state.settings.keybindings_view_scroll.saturating_sub(1);
        }
        KeybindingsAction::ScrollDown => {
            state.settings.keybindings_view_scroll =
                state.settings.keybindings_view_scroll.saturating_add(1);
        }
    }
    fx
}

/// The editor for one of Deck's OpenSSH connection-reuse values.
pub(super) fn reduce_ssh_setting(state: &mut AppState, action: SshSettingAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        SshSettingAction::Open(field) => {
            let value = match field {
                crate::overlay::SshSettingField::ControlPath => &state.prefs.ssh_control_path,
                crate::overlay::SshSettingField::ControlPersist => &state.prefs.ssh_control_persist,
            };
            state.overlay.open(ModalState::SshSetting(
                crate::overlay::SshSettingEditorState::new(field, value),
            ));
        }
        SshSettingAction::InputKey(key) => {
            if let Some(editor) = state.overlay.ssh_setting_editor_mut() {
                textarea_input(&mut editor.input, key);
                editor.error = None;
            }
        }
        SshSettingAction::Confirm => {
            let Some(mut editor) = state.overlay.take_ssh_setting_editor() else {
                return fx;
            };
            let value = editor.input_str().trim().to_string();
            let validation = match editor.field {
                crate::overlay::SshSettingField::ControlPath => {
                    crate::config::validate_ssh_control_path(&value)
                }
                crate::overlay::SshSettingField::ControlPersist => {
                    crate::config::validate_ssh_control_persist(&value)
                }
            };
            if let Err(error) = validation {
                editor.error = Some(error);
                state.overlay.open(ModalState::SshSetting(editor));
            } else {
                match editor.field {
                    crate::overlay::SshSettingField::ControlPath => {
                        state.prefs.ssh_control_path = value
                    }
                    crate::overlay::SshSettingField::ControlPersist => {
                        state.prefs.ssh_control_persist = value
                    }
                }
                fx.save_config();
            }
        }
        SshSettingAction::Cancel => {
            state.overlay.close(Modal::SshSetting);
        }
    }
    fx
}

/// The exclude-pattern editor.
pub(super) fn reduce_exclude(state: &mut AppState, action: ExcludeAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        ExcludeAction::Open => {
            state.overlay.open(ModalState::ExcludeEditor(
                crate::overlay::ExcludeEditorState::new(),
            ));
        }
        ExcludeAction::Close => {
            state.overlay.close(Modal::ExcludeEditor);
        }
        // Every remaining action edits the open editor; one guard for all.
        other => {
            let Some(editor) = state.overlay.exclude_editor_mut() else {
                return fx;
            };
            match other {
                ExcludeAction::Next => {
                    if !editor.adding && !state.prefs.exclude_patterns.is_empty() {
                        editor.selected =
                            step_clamped(editor.selected, state.prefs.exclude_patterns.len(), 1);
                    }
                }
                ExcludeAction::Prev => {
                    if !editor.adding {
                        editor.selected =
                            step_clamped(editor.selected, state.prefs.exclude_patterns.len(), -1);
                    }
                }
                ExcludeAction::StartAdd | ExcludeAction::CancelAdd => {
                    editor.adding = matches!(other, ExcludeAction::StartAdd);
                    editor.reset_input();
                    editor.error = None;
                }
                ExcludeAction::Delete => {
                    if !editor.adding && !state.prefs.exclude_patterns.is_empty() {
                        state.prefs.exclude_patterns.remove(editor.selected);
                        if editor.selected >= state.prefs.exclude_patterns.len() {
                            editor.selected = state.prefs.exclude_patterns.len().saturating_sub(1);
                        }
                        fx.save_config();
                        fx.refresh_sessions();
                    }
                }
                ExcludeAction::InputKey(key) => {
                    if editor.adding {
                        textarea_input(&mut editor.input, key);
                        editor.error = None;
                    }
                }
                ExcludeAction::Confirm if editor.adding => {
                    let pattern = editor.input_str().trim().to_string();
                    if pattern.is_empty() {
                        editor.adding = false;
                    } else if let Some(e) = pattern
                        .strip_prefix('/')
                        .and_then(|s| s.strip_suffix('/'))
                        .and_then(|inner| regex::Regex::new(inner).err())
                    {
                        // A malformed `/regex/` pattern: report and keep editing.
                        editor.error = Some(format!("Invalid regex: {}", e));
                    } else {
                        // Accept the pattern (plain glob or a valid `/regex/`).
                        state.prefs.exclude_patterns.push(pattern);
                        editor.adding = false;
                        editor.reset_input();
                        editor.error = None;
                        editor.selected = state.prefs.exclude_patterns.len().saturating_sub(1);
                        fx.save_config();
                        fx.refresh_sessions();
                    }
                }
                // Confirm outside add mode has nothing to commit. Open and
                // Close never reach here — the arms above take them — but
                // naming them keeps this match exhaustive, so a new
                // `ExcludeAction` has to say what it does.
                ExcludeAction::Confirm | ExcludeAction::Open | ExcludeAction::Close => {}
            }
        }
    }
    fx
}

//! Reducer for the settings page and its sub-overlays (theme picker,
//! keybindings view, exclude-pattern editor). Split out of `reduce` to keep
//! the top-level dispatcher readable; entry point is `reduce_settings`.

use crate::app::settings::setting_rows;
use crate::effects::{Effect, SideEffect};
use crate::state::{step_clamped, AppState, FocusMode, MainView};
use crate::theme::indices_for_slot;

use super::{apply_action, SettingsAction};

pub(super) fn reduce_settings(state: &mut AppState, action: SettingsAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        SettingsAction::Open => {
            state.main_view = MainView::Settings;
            state.focus_mode = FocusMode::Main;
            state.settings.reset_pages();
            state.settings.theme_picker_open = false;
            state.settings.theme_picker_selected = state.prefs.theme_index;
            state.overlay.ssh_setting_editor = None;
        }
        SettingsAction::Close => {
            state.main_view = MainView::Terminal;
            state.focus_mode = FocusMode::Main;
            state.settings.reset_pages();
            state.settings.theme_picker_open = false;
            state.overlay.ssh_setting_editor = None;
        }
        SettingsAction::OpenPage(page) => {
            state.settings.push_page(page);
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
            state.cycle_frame_rate_limit(direction);
            fx.save_config();
        }
        SettingsAction::CycleAgentsProbeInterval(direction) => {
            state.cycle_agents_probe_interval(direction);
            fx.save_config();
        }
        SettingsAction::CycleSummaryAgent(direction) => {
            state.cycle_summary_agent(direction);
            fx.save_config();
        }
        SettingsAction::ToggleSummary => {
            state.prefs.summary_enabled = !state.prefs.summary_enabled;
            fx.save_config();
        }
        SettingsAction::ToggleSshConnectionReuse => {
            state.prefs.ssh_connection_reuse = !state.prefs.ssh_connection_reuse;
            if !state.prefs.ssh_connection_reuse {
                state.overlay.port_forward = None;
            }
            fx.save_config();
        }
        SettingsAction::OpenSshSettingEditor(field) => {
            let value = match field {
                crate::overlay::SshSettingField::ControlPath => &state.prefs.ssh_control_path,
                crate::overlay::SshSettingField::ControlPersist => &state.prefs.ssh_control_persist,
            };
            state.overlay.ssh_setting_editor =
                Some(crate::overlay::SshSettingEditorState::new(field, value));
        }
        SettingsAction::SshSettingInputKey(key) => {
            if let Some(editor) = state.overlay.ssh_setting_editor.as_mut() {
                editor.input.input(key);
                editor.error = None;
            }
        }
        SettingsAction::SshSettingConfirm => {
            let Some(mut editor) = state.overlay.ssh_setting_editor.take() else {
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
                state.overlay.ssh_setting_editor = Some(editor);
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
        SettingsAction::SshSettingCancel => {
            state.overlay.ssh_setting_editor = None;
        }
        SettingsAction::OpenAddRemotePicker => fx.push(Effect::OpenAddRemotePicker),
        // One aggregate row for every host — it opens the first host
        // that has forwards (else the first host); per-host editing stays on
        // each `@host` divider's `[⇄N]` badge button.
        SettingsAction::OpenPortForwards => fx.push(Effect::OpenConfiguredPortForwards),
        SettingsAction::OpenThemePicker(slot) => {
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
        SettingsAction::ToggleThemeAuto => {
            state.prefs.theme_auto = !state.prefs.theme_auto;
            fx.save_config();
            // Re-ask on the way in: the terminal may have flipped appearance
            // since startup, and if auto was off we ignored any report so far.
            if state.prefs.theme_auto {
                fx.push(Effect::QueryColorScheme);
            }
            fx.push(Effect::ApplyTmuxTheme);
        }
        SettingsAction::CloseThemePicker => {
            state.settings.theme_picker_open = false;
        }
        SettingsAction::ThemePickerNext => {
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
        SettingsAction::ThemePickerPrev => {
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
        SettingsAction::ConfirmThemePicker => {
            state.settings.theme_picker_open = false;
        }

        SettingsAction::OpenKeybindingsView => {
            state.settings.keybindings_view_open = true;
            state.settings.keybindings_view_scroll = 0;
        }
        SettingsAction::CloseKeybindingsView => {
            state.settings.keybindings_view_open = false;
        }
        SettingsAction::KeybindingsScrollUp => {
            state.settings.keybindings_view_scroll =
                state.settings.keybindings_view_scroll.saturating_sub(1);
        }
        SettingsAction::KeybindingsScrollDown => {
            state.settings.keybindings_view_scroll =
                state.settings.keybindings_view_scroll.saturating_add(1);
        }

        SettingsAction::ToggleUpdateCheck => {
            use crate::update::UpdateCheckMode::{Disabled, Enabled};
            let was_enabled = state.prefs.update_check_mode == Enabled;
            state.prefs.update_check_mode = if was_enabled { Disabled } else { Enabled };
            if was_enabled {
                state.update_available = None;
            }
            fx.save_config();
        }

        SettingsAction::ExcludeOpen => {
            state.overlay.exclude_editor = Some(crate::overlay::ExcludeEditorState::new());
        }
        SettingsAction::ExcludeClose => {
            state.overlay.exclude_editor = None;
        }
        // Every remaining action edits the open exclude editor; one guard for all.
        other => {
            let Some(editor) = state.overlay.exclude_editor.as_mut() else {
                return fx;
            };
            match other {
                SettingsAction::ExcludeNext => {
                    if !editor.adding && !state.prefs.exclude_patterns.is_empty() {
                        editor.selected =
                            step_clamped(editor.selected, state.prefs.exclude_patterns.len(), 1);
                    }
                }
                SettingsAction::ExcludePrev => {
                    if !editor.adding {
                        editor.selected =
                            step_clamped(editor.selected, state.prefs.exclude_patterns.len(), -1);
                    }
                }
                SettingsAction::ExcludeStartAdd | SettingsAction::ExcludeCancelAdd => {
                    editor.adding = matches!(other, SettingsAction::ExcludeStartAdd);
                    editor.reset_input();
                    editor.error = None;
                }
                SettingsAction::ExcludeDelete => {
                    if !editor.adding && !state.prefs.exclude_patterns.is_empty() {
                        state.prefs.exclude_patterns.remove(editor.selected);
                        if editor.selected >= state.prefs.exclude_patterns.len() {
                            editor.selected = state.prefs.exclude_patterns.len().saturating_sub(1);
                        }
                        fx.save_config();
                        fx.refresh_sessions();
                    }
                }
                SettingsAction::ExcludeInputKey(key) => {
                    if editor.adding {
                        editor.input.input(key);
                        editor.error = None;
                    }
                }
                SettingsAction::ExcludeConfirm if editor.adding => {
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
                _ => {}
            }
        }
    }
    fx
}

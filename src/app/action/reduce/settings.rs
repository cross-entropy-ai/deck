//! Reducer for the settings page and its sub-overlays (theme picker,
//! keybindings view, exclude-pattern editor). Split out of `reduce` to keep
//! the top-level dispatcher readable; entry point is `reduce_settings`.

use crate::app::settings::SETTING_ROWS;
use crate::effects::Effect;
use crate::state::{step_clamped, AppState, FocusMode, MainView, SideEffect};
use crate::theme::THEMES;

use super::{apply_action, SettingsAction};

pub(super) fn reduce_settings(state: &mut AppState, action: SettingsAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        SettingsAction::Open => {
            state.main_view = MainView::Settings;
            state.focus_mode = FocusMode::Main;
            state.settings.theme_picker_open = false;
            state.settings.theme_picker_selected = state.prefs.theme_index;
        }
        SettingsAction::Close => {
            state.main_view = MainView::Terminal;
            state.focus_mode = FocusMode::Main;
            state.settings.theme_picker_open = false;
        }
        SettingsAction::Next => {
            state.settings.selected = step_clamped(state.settings.selected, SETTING_ROWS.len(), 1);
        }
        SettingsAction::Prev => {
            state.settings.selected = step_clamped(state.settings.selected, SETTING_ROWS.len(), -1);
        }
        SettingsAction::Adjust | SettingsAction::AdjustPrev => {
            let direction = if matches!(action, SettingsAction::AdjustPrev) {
                -1
            } else {
                1
            };
            // Look up the selected row and fire its adjust — the row, not a
            // positional match, is the source of truth.
            if let Some(row) = SETTING_ROWS.get(state.settings.selected) {
                let inner = apply_action(state, (row.adjust)(direction));
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
        SettingsAction::ToggleSummary => {
            state.prefs.summary_enabled = !state.prefs.summary_enabled;
            fx.save_config();
        }
        SettingsAction::OpenAddRemotePicker => fx.open_add_remote_picker(),
        // ponytail: one aggregate row for every host — it opens the first host
        // that has forwards (else the first host); per-host editing stays on
        // each `@host` divider's `[⇄N]` badge button.
        SettingsAction::OpenPortForwards => {
            if let Some(r) = state
                .config_remotes
                .iter()
                .find(|r| !r.forwards.is_empty())
                .or_else(|| state.config_remotes.first())
            {
                fx.push(Effect::OpenForwardOverlay(r.host.clone()));
            }
        }
        SettingsAction::OpenThemePicker => {
            // Opens as a standalone overlay over the current view: from the
            // sidebar (`t`) it doesn't enter the settings page, from settings
            // it layers on top. Leaving `main_view`/`focus_mode` untouched lets
            // closing the picker return to wherever it was opened from.
            state.settings.theme_picker_open = true;
            state.settings.theme_picker_selected =
                state.prefs.theme_index.min(THEMES.len().saturating_sub(1));
        }
        SettingsAction::CloseThemePicker => {
            state.settings.theme_picker_open = false;
        }
        SettingsAction::ThemePickerNext => {
            state.settings.theme_picker_selected =
                step_clamped(state.settings.theme_picker_selected, THEMES.len(), 1);
            state.prefs.theme_index = state.settings.theme_picker_selected;
            fx.save_config();
            fx.apply_tmux_theme();
        }
        SettingsAction::ThemePickerPrev => {
            // Side effects only fire when the cursor actually moves (unlike
            // Next, which always re-applies) — preserve that asymmetry.
            if state.settings.theme_picker_selected > 0 {
                state.settings.theme_picker_selected =
                    step_clamped(state.settings.theme_picker_selected, THEMES.len(), -1);
                state.prefs.theme_index = state.settings.theme_picker_selected;
                fx.save_config();
                fx.apply_tmux_theme();
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
            state.prefs.update_check_mode = match state.prefs.update_check_mode {
                crate::update::UpdateCheckMode::Enabled => crate::update::UpdateCheckMode::Disabled,
                crate::update::UpdateCheckMode::Disabled => crate::update::UpdateCheckMode::Enabled,
            };
            if state.prefs.update_check_mode == crate::update::UpdateCheckMode::Disabled {
                state.update_available = None;
            }
            fx.save_config();
        }

        SettingsAction::ExcludeOpen => {
            state.overlay.exclude_editor = Some(crate::state::ExcludeEditorState::new());
        }
        SettingsAction::ExcludeClose => {
            state.overlay.exclude_editor = None;
        }
        SettingsAction::ExcludeNext => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                if !editor.adding && !state.prefs.exclude_patterns.is_empty() {
                    editor.selected =
                        step_clamped(editor.selected, state.prefs.exclude_patterns.len(), 1);
                }
            }
        }
        SettingsAction::ExcludePrev => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                if !editor.adding {
                    editor.selected =
                        step_clamped(editor.selected, state.prefs.exclude_patterns.len(), -1);
                }
            }
        }
        SettingsAction::ExcludeStartAdd => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                editor.adding = true;
                editor.reset_input();
                editor.error = None;
            }
        }
        SettingsAction::ExcludeCancelAdd => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                editor.adding = false;
                editor.reset_input();
                editor.error = None;
            }
        }
        SettingsAction::ExcludeDelete => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                if !editor.adding && !state.prefs.exclude_patterns.is_empty() {
                    state.prefs.exclude_patterns.remove(editor.selected);
                    if editor.selected >= state.prefs.exclude_patterns.len() {
                        editor.selected = state.prefs.exclude_patterns.len().saturating_sub(1);
                    }
                    fx.save_config();
                    fx.refresh_sessions();
                }
            }
        }
        SettingsAction::ExcludeInputKey(key) => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                if editor.adding {
                    editor.input.input(key);
                    editor.error = None;
                }
            }
        }
        SettingsAction::ExcludeConfirm => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                if editor.adding {
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
            }
        }
    }
    fx
}

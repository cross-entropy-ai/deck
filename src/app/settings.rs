//! The settings page as a descriptor table — the source of truth for its rows:
//! their order and what each shows and does. Adding a setting means adding one
//! [`SettingRow`]; the renderer (`app::render`) and reducer
//! (`app::action::reduce`) both iterate this slice, so a row's label, value,
//! help, and adjust action can't drift apart.
//!
//! Lives in `app`, not `model`: each row's `adjust` produces an [`Action`],
//! which `model` doesn't depend on. The value/help closures read `&AppState` so
//! dynamic text (frame-rate caveat, update-check "last checked" line) stays
//! beside the value and action it describes.

use crate::action::{Action, SettingsAction, SummaryAction};
use crate::state::{AppState, LayoutMode, ViewMode};
use crate::theme::THEMES;

use super::update::format_update_check_help;

/// One row of the settings page. The closures borrow `&AppState` so the
/// renderer builds display strings each frame; `adjust` maps a direction
/// (`+1` right / `-1` left) to the action left/right (or Enter) fires on that
/// row — toggles and openers ignore the direction.
pub struct SettingRow {
    pub label: &'static str,
    pub value: fn(&AppState) -> String,
    pub help: fn(&AppState) -> String,
    pub adjust: fn(i32) -> Action,
}

/// The settings page, top to bottom. Order is load-bearing: the reducer
/// indexes this by `state.settings.selected`, and the renderer draws it in
/// sequence.
pub const SETTING_ROWS: &[SettingRow] = &[
    SettingRow {
        label: "Theme",
        value: |s| THEMES[s.prefs.theme_index].name.to_string(),
        help: |_| "Left/right opens the theme list".to_string(),
        adjust: |_| Action::Settings(SettingsAction::OpenThemePicker),
    },
    SettingRow {
        label: "Transparent",
        value: |s| if s.prefs.transparent_bg { "On" } else { "Off" }.to_string(),
        help: |_| "Use terminal's default background (enables transparency)".to_string(),
        adjust: |_| Action::ToggleTransparentBg,
    },
    SettingRow {
        label: "Layout",
        value: |s| match s.prefs.layout_mode {
            LayoutMode::Horizontal => "Horizontal".to_string(),
            LayoutMode::Vertical => "Vertical".to_string(),
        },
        help: |_| "Left/right toggles the split direction".to_string(),
        adjust: |_| Action::ToggleLayout,
    },
    SettingRow {
        label: "Borders",
        value: |s| if s.prefs.show_borders { "On" } else { "Off" }.to_string(),
        help: |_| "Left/right toggles pane borders".to_string(),
        adjust: |_| Action::ToggleBorders,
    },
    SettingRow {
        label: "View",
        value: |s| match s.prefs.view_mode {
            ViewMode::Expanded => "Expanded".to_string(),
            ViewMode::Compact => "Compact".to_string(),
        },
        help: |_| "Left/right toggles compact mode".to_string(),
        adjust: |_| Action::ToggleViewMode,
    },
    SettingRow {
        label: "Frame rate",
        value: |s| crate::state::frame_rate_limit_label(s.prefs.frame_rate_limit).to_string(),
        help: |s| {
            if s.prefs.frame_rate_limit == 30 {
                "Smooth increases terminal rendering pressure"
            } else {
                "Left/right cycles the render limit"
            }
            .to_string()
        },
        adjust: |dir| Action::Settings(SettingsAction::CycleFrameRateLimit(dir)),
    },
    SettingRow {
        label: "Exclude",
        value: |s| format!("{} patterns", s.prefs.exclude_patterns.len()),
        help: |_| "Left/right opens the pattern editor".to_string(),
        adjust: |_| Action::Settings(SettingsAction::ExcludeOpen),
    },
    SettingRow {
        label: "Keybindings",
        value: |_| "View".to_string(),
        help: |_| "Left/right shows current key bindings".to_string(),
        adjust: |_| Action::Settings(SettingsAction::OpenKeybindingsView),
    },
    SettingRow {
        label: "Update check",
        value: |s| {
            if s.prefs.update_check_mode == crate::update::UpdateCheckMode::Enabled {
                "Enabled"
            } else {
                "Disabled"
            }
            .to_string()
        },
        help: |s| format_update_check_help(s.update_last_checked_secs),
        adjust: |_| Action::Settings(SettingsAction::ToggleUpdateCheck),
    },
    SettingRow {
        label: "Summary",
        value: |s| if s.prefs.summary_enabled { "On" } else { "Off" }.to_string(),
        help: |_| "Left/right shows or hides the inline Summary card".to_string(),
        adjust: |_| Action::Settings(SettingsAction::ToggleSummary),
    },
    SettingRow {
        label: "Summary lang",
        value: |s| crate::summary::language_label(&s.prefs.summary_language).to_string(),
        help: |_| "Left/right cycles the generated summary's language".to_string(),
        adjust: |_| Action::Summary(SummaryAction::OpenLanguageEditor),
    },
    SettingRow {
        label: "Agents probe",
        value: |s| {
            crate::state::agents_probe_interval_label(s.prefs.agents_probe_interval_secs)
                .to_string()
        },
        help: |_| "Left/right cycles how often the Agents tab probes".to_string(),
        adjust: |dir| Action::Settings(SettingsAction::CycleAgentsProbeInterval(dir)),
    },
    SettingRow {
        label: "Remotes",
        value: |s| format!("{} hosts", s.config_remotes.len()),
        help: |_| "Left/right adds a remote SSH host".to_string(),
        adjust: |_| Action::Settings(SettingsAction::OpenAddRemotePicker),
    },
    SettingRow {
        label: "Port forwards",
        value: |s| match s
            .config_remotes
            .iter()
            .map(|r| r.forwards.len())
            .sum::<usize>()
        {
            0 => "none".to_string(),
            n => format!("{n} forwards"),
        },
        help: |_| "Left/right opens a configured host's port forwards".to_string(),
        adjust: |_| Action::Settings(SettingsAction::OpenPortForwards),
    },
];

//! The settings pages as descriptor tables — the source of truth for their rows:
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
use crate::state::{AppState, LayoutMode, SettingsSubmenu, ViewMode};
use crate::theme::{ThemeSlot, THEMES};

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
        value: |s| {
            if s.prefs.theme_auto {
                "Auto  ›".to_string()
            } else {
                format!("{}  ›", THEMES[s.prefs.theme_index].name)
            }
        },
        help: |_| "Enter/right opens theme settings".to_string(),
        adjust: |_| Action::Settings(SettingsAction::OpenSubmenu(SettingsSubmenu::Theme)),
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
        label: "Agents",
        value: |_| "Configure  ›".to_string(),
        help: |_| "Enter/right opens agent settings".to_string(),
        adjust: |_| Action::Settings(SettingsAction::OpenSubmenu(SettingsSubmenu::Agents)),
    },
    SettingRow {
        label: "Remote",
        value: |s| format!("{} hosts  ›", s.config_remotes.len()),
        help: |_| "Enter/right opens remote settings".to_string(),
        adjust: |_| Action::Settings(SettingsAction::OpenSubmenu(SettingsSubmenu::Remote)),
    },
];

const AUTO_THEME_ROW: SettingRow = SettingRow {
    label: "Auto theme",
    value: |s| {
        if !s.prefs.theme_auto {
            "Off".to_string()
        } else if s.terminal_is_dark {
            "On (terminal is dark)".to_string()
        } else {
            "On (terminal is light)".to_string()
        }
    },
    help: |_| "Follow the terminal's own background color (OSC 11)".to_string(),
    adjust: |_| Action::Settings(SettingsAction::ToggleThemeAuto),
};

const FIXED_THEME_ROW: SettingRow = SettingRow {
    label: "Theme",
    value: |s| THEMES[s.prefs.theme_index].name.to_string(),
    help: |_| "Theme used while Auto theme is off".to_string(),
    adjust: |_| Action::Settings(SettingsAction::OpenThemePicker(ThemeSlot::Fixed)),
};

const DARK_THEME_ROW: SettingRow = SettingRow {
    label: "Dark theme",
    value: |s| THEMES[s.prefs.dark_theme_index].name.to_string(),
    help: |_| "Theme used when the terminal background is dark".to_string(),
    adjust: |_| Action::Settings(SettingsAction::OpenThemePicker(ThemeSlot::Dark)),
};

const LIGHT_THEME_ROW: SettingRow = SettingRow {
    label: "Light theme",
    value: |s| THEMES[s.prefs.light_theme_index].name.to_string(),
    help: |_| "Theme used when the terminal background is light".to_string(),
    adjust: |_| Action::Settings(SettingsAction::OpenThemePicker(ThemeSlot::Light)),
};

const TRANSPARENT_BACKGROUND_ROW: SettingRow = SettingRow {
    label: "Transparent background",
    value: |s| if s.prefs.transparent_bg { "On" } else { "Off" }.to_string(),
    help: |_| "Use the terminal's default background (enables transparency)".to_string(),
    adjust: |_| Action::ToggleTransparentBg,
};

const AGENTS_PROBE_ROW: SettingRow = SettingRow {
    label: "Agents probe",
    value: |s| {
        crate::state::agents_probe_interval_label(s.prefs.agents_probe_interval_secs).to_string()
    },
    help: |_| "Left/right cycles how often the Agents tab probes".to_string(),
    adjust: |dir| Action::Settings(SettingsAction::CycleAgentsProbeInterval(dir)),
};

const SUMMARY_ROW: SettingRow = SettingRow {
    label: "Summary",
    value: |s| if s.prefs.summary_enabled { "On" } else { "Off" }.to_string(),
    help: |_| "Enter/right shows or hides the inline Summary card".to_string(),
    adjust: |_| Action::Settings(SettingsAction::ToggleSummary),
};

const SUMMARY_LANGUAGE_ROW: SettingRow = SettingRow {
    label: "Summary lang",
    value: |s| crate::summary::language_label(&s.prefs.summary_language).to_string(),
    help: |_| "Enter/right edits the generated summary's language".to_string(),
    adjust: |_| Action::Summary(SummaryAction::OpenLanguageEditor),
};

const REMOTES_ROW: SettingRow = SettingRow {
    label: "Remotes",
    value: |s| format!("{} hosts", s.config_remotes.len()),
    help: |_| "Enter/right adds a remote SSH host".to_string(),
    adjust: |_| Action::Settings(SettingsAction::OpenAddRemotePicker),
};

const PORT_FORWARDS_ROW: SettingRow = SettingRow {
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
    help: |_| "Enter/right opens a configured host's port forwards".to_string(),
    adjust: |_| Action::Settings(SettingsAction::OpenPortForwards),
};

/// Rows visible in the Theme submenu. A fixed theme and the automatic
/// dark/light pair are mutually exclusive, keeping the page compact while
/// making the effect of Auto theme explicit.
fn theme_setting_rows(state: &AppState) -> Vec<&'static SettingRow> {
    if state.prefs.theme_auto {
        vec![
            &AUTO_THEME_ROW,
            &DARK_THEME_ROW,
            &LIGHT_THEME_ROW,
            &TRANSPARENT_BACKGROUND_ROW,
        ]
    } else {
        vec![
            &AUTO_THEME_ROW,
            &FIXED_THEME_ROW,
            &TRANSPARENT_BACKGROUND_ROW,
        ]
    }
}

/// The visible descriptor rows for the active settings page. Renderer and
/// reducer both call this so submenu presentation and actions stay aligned.
pub fn setting_rows(state: &AppState) -> Vec<&'static SettingRow> {
    match state.settings.submenu {
        None => SETTING_ROWS.iter().collect(),
        Some(SettingsSubmenu::Theme) => theme_setting_rows(state),
        Some(SettingsSubmenu::Agents) => {
            vec![&AGENTS_PROBE_ROW, &SUMMARY_ROW, &SUMMARY_LANGUAGE_ROW]
        }
        Some(SettingsSubmenu::Remote) => vec![&REMOTES_ROW, &PORT_FORWARDS_ROW],
    }
}

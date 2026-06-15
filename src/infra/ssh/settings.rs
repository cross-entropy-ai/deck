//! SSH's contribution to the Settings page. Implements the shared
//! [`SettingsProvider`](crate::settings_framework::SettingsProvider) contract
//! and is registered independently in `app::settings::SETTINGS_PROVIDERS` —
//! the ssh backend owns these rows, not the tmux system.

use crate::effects::Effect;
use crate::settings_framework::{SettingDef, SettingsCtx};

/// SSH settings rows. Today: jump to the add-remote-host picker, labelled with
/// the configured host count.
pub fn rows(ctx: &SettingsCtx) -> Vec<SettingDef> {
    vec![SettingDef {
        label: "Remotes",
        value: format!("{} hosts", ctx.remotes.len()),
        help: "Left/right adds a remote SSH host".to_string(),
        effect: |_| Effect::OpenAddRemotePicker,
    }]
}

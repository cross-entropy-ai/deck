//! The settings-provider framework: the contract a subsystem implements to add
//! rows to the Settings page, decoupled from the page itself.
//!
//! A provider is a plain `fn(&SettingsCtx) -> Vec<SettingDef>` — the unified
//! standard every contributor implements (ssh is the first; see
//! `infra::ssh::settings`). The app concatenates the registered providers'
//! rows after its core table. A row yields an [`Effect`], never an `Action`,
//! so a provider in a low layer (infra) stays independent of the app layer.

use crate::config::RemoteConfig;
use crate::effects::Effect;

/// Read-only state a provider reads to build its rows. Narrow on purpose —
/// providers compute row values from this, not from the app's `AppState`.
/// Widen as providers need more.
pub struct SettingsCtx<'a> {
    pub remotes: &'a [RemoteConfig],
}

/// One settings row a provider contributes. Mirrors the app's core
/// `SettingRow`, but `effect` yields an [`Effect`] (not an `Action`).
/// `value`/`help` are computed from [`SettingsCtx`] at build time, so this
/// carries plain strings.
pub struct SettingDef {
    pub label: &'static str,
    pub value: String,
    pub help: String,
    /// Direction (`+1` right / `-1` left; Enter is `+1`) → the row's effect.
    // ponytail: fn-ptr like the core table — no captured payload. If a row ever
    // needs to close over data (e.g. a per-host effect), widen to Box<dyn Fn>.
    pub effect: fn(i32) -> Effect,
}

/// The unified contract: a subsystem exposes one of these to add settings
/// rows. Registered in `app::settings::SETTINGS_PROVIDERS`.
pub type SettingsProvider = fn(&SettingsCtx) -> Vec<SettingDef>;

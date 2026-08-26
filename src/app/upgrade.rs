//! Deciding what "install the update" means from wherever deck is installed.
//!
//! The choice — run brew, re-exec ourselves as a downloader, or refuse and
//! explain — depends only on how deck was installed, whether we ship a binary
//! for this platform, and where our own executable is. Kept as a pure function
//! over those three so it can be checked without a brew, a release, or a
//! writable install directory.

use crate::overlay::WarningState;
use crate::self_update::{manual_upgrade_hint, InstallMethod};

/// What `TriggerUpgrade` should do, decided before anything is spawned.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum UpgradePlan {
    /// Run this in the upgrade pane.
    Run { program: String, args: Vec<String> },
    /// Deck cannot upgrade itself from here. Show this instead.
    Refuse(&'static str, String),
}

/// Resolve the plan for upgrading to `latest`.
///
/// `target_triple` is `None` on a platform we publish no binary for, and
/// `current_exe` is `None` when the process cannot name its own path — both
/// only matter to the direct-download route.
pub(super) fn plan_upgrade(
    method: InstallMethod,
    latest: &str,
    target_triple: Option<&str>,
    current_exe: Option<String>,
) -> UpgradePlan {
    match method {
        InstallMethod::Brew => UpgradePlan::Run {
            program: "brew".to_string(),
            args: vec!["upgrade".into(), "cross-entropy-ai/tap/deck".into()],
        },
        // Re-exec our own binary in the hidden `__upgrade-self` mode, which
        // drives the `self_update` crate: its progress then renders live in
        // the upgrade pane, and it replaces the binary in place.
        InstallMethod::DirectDownload if target_triple.is_some() => UpgradePlan::Run {
            program: current_exe.unwrap_or_else(|| "deck".to_string()),
            args: vec!["__upgrade-self".into(), latest.to_string()],
        },
        InstallMethod::DirectDownload => UpgradePlan::Refuse(
            "Unsupported platform",
            "deck doesn't ship a prebuilt binary for this platform. Rebuild from \
             source via `cargo install --git https://github.com/cross-entropy-ai/deck`."
                .to_string(),
        ),
        // We can't write to where deck lives (e.g. /usr/local/bin without
        // brew). Point the user at the install methods instead.
        InstallMethod::Manual => UpgradePlan::Refuse(
            "deck can't self-update from this location",
            manual_upgrade_hint(latest),
        ),
    }
}

impl UpgradePlan {
    /// The refusal as the warning overlay shows it.
    pub(super) fn warning(self) -> Option<WarningState> {
        match self {
            Self::Refuse(text, detail) => Some(WarningState { text, detail }),
            Self::Run { .. } => None,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/app/upgrade.rs"]
mod tests;

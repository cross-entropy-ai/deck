//! Cross-platform self-upgrade detection.
//!
//! deck reaches users via several install paths (Homebrew/linuxbrew, `cargo
//! install`, or a direct tarball from GitHub Releases), each needing a
//! different upgrade mechanism. `detect_install_method()` returns the
//! `InstallMethod` the dispatcher branches on:
//!
//! - `Brew`: binary resolves under `brew --prefix`; let brew upgrade so its
//!   metadata stays in sync.
//! - `DirectDownload`: binary in a user-writable dir (e.g. `~/.cargo/bin`);
//!   deck downloads the release tarball and atomically replaces itself.
//! - `Manual`: privileged path (root-owned, no brew) or exotic platform —
//!   deck prints a curl/tar one-liner for the running arch instead of writing
//!   where it can't.

use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub enum InstallMethod {
    Brew,
    /// The binary lives in a user-writable directory, so `self_update`
    /// can download a release and replace it in place (it resolves the
    /// current exe path itself — see `run_self_upgrade`).
    DirectDownload,
    /// We can't safely write to the binary's location. Caller should
    /// surface a manual upgrade hint (see `manual_upgrade_hint`).
    Manual,
}

/// GitHub-Release artifact target triple for the running binary. None
/// when we don't ship a prebuilt binary for this platform (the
/// release.yml matrix is the source of truth).
pub fn target_triple() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("aarch64-unknown-linux-musl")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-musl")
    } else {
        None
    }
}

pub fn detect_install_method() -> InstallMethod {
    let exe = match std::env::current_exe().and_then(std::fs::canonicalize) {
        Ok(p) => p,
        Err(_) => return InstallMethod::Manual,
    };

    if is_brew_managed(&exe) {
        return InstallMethod::Brew;
    }

    let Some(parent) = exe.parent() else {
        return InstallMethod::Manual;
    };
    if !is_dir_writable(parent) {
        return InstallMethod::Manual;
    }
    InstallMethod::DirectDownload
}

fn is_brew_managed(exe: &Path) -> bool {
    let Ok(out) = Command::new("brew")
        .arg("--prefix")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let prefix = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if prefix.is_empty() {
        return false;
    }
    // The resolved exe always traverses brew's Cellar, so a prefix
    // `starts_with` check holds even with symlinks pointing into the
    // cellar from elsewhere.
    exe.starts_with(&prefix)
}

/// True iff a new file can be created in `dir`. Used as a proxy for
/// "we can also rename(2) over an existing file in this dir", which
/// is what the direct-download upgrade ultimately does.
fn is_dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".deck-write-probe.{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Direct (non-brew) self-upgrade to `version` via the `self_update` crate:
/// fetch the GitHub release asset for `target`, extract `deck`, and replace
/// the running executable in place. Runs in the upgrade PTY subprocess
/// (`deck __upgrade-self`), so `show_download_progress` renders live. Blocks.
pub fn run_self_upgrade(version: &str) -> Result<(), String> {
    let target = target_triple().ok_or_else(|| {
        "no prebuilt binary ships for this platform; rebuild from source".to_string()
    })?;

    // deck tags releases `vX.Y.Z` and names assets
    // `deck-v{ver}-{target}.tar.gz`; the asset name contains `target`,
    // which is how self_update matches it.
    let status = self_update::backends::github::Update::configure()
        .repo_owner("cross-entropy-ai")
        .repo_name("deck")
        .bin_name("deck")
        .target(target)
        .target_version_tag(&format!("v{version}"))
        .current_version(env!("CARGO_PKG_VERSION"))
        .show_download_progress(true)
        .no_confirm(true)
        .build()
        .map_err(|e| e.to_string())?
        .update()
        .map_err(|e| e.to_string())?;

    println!(
        "\nUpgraded to {}. Restart deck to use it.",
        status.version()
    );
    Ok(())
}

/// Human-readable fallback shown when deck can't write its own binary
/// (`InstallMethod::Manual`) — e.g. a root-owned `/usr/local/bin` with no
/// brew. We can't self-replace, so point the user at the install methods.
pub fn manual_upgrade_hint(version: &str) -> String {
    format!(
        "deck can't self-update from this location (the binary isn't writable).\n\
         Upgrade with your package manager, or download deck v{version} from:\n\
         https://github.com/cross-entropy-ai/deck/releases/latest\n\
         or rebuild: cargo install --git https://github.com/cross-entropy-ai/deck"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_triple_known_for_supported_platforms() {
        // The test runner runs on one of the supported targets, so
        // we should always get Some(_).
        assert!(target_triple().is_some());
    }

    #[test]
    fn manual_hint_mentions_version() {
        assert!(manual_upgrade_hint("1.2.3").contains("1.2.3"));
    }
}

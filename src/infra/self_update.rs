//! Cross-platform self-upgrade detection.
//!
//! deck binaries reach users via several install paths — Homebrew on
//! macOS / linuxbrew on Linux, `cargo install`, or just a direct
//! tarball download from GitHub Releases. The original upgrade flow
//! shelled out to `brew upgrade` unconditionally, which left Linux
//! users without linuxbrew and macOS users without Homebrew stranded
//! with a "Homebrew not found" wall and a copy-pasteable cargo command.
//!
//! `detect_install_method()` figures out which path deck took to land
//! on this machine and returns an `InstallMethod` the dispatcher can
//! branch on:
//!
//! - `Brew`: the binary resolves under `brew --prefix`; let brew
//!   manage the upgrade so its own metadata stays in sync.
//! - `DirectDownload`: the binary lives in a user-writable directory
//!   (e.g. `~/.cargo/bin`); deck downloads the right release tarball
//!   and atomically replaces itself.
//! - `Manual`: privileged path (root-owned, no brew) or an exotic
//!   platform — deck prints a curl/tar one-liner tailored to the
//!   running architecture instead of trying to write where it can't.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub enum InstallMethod {
    Brew,
    DirectDownload {
        /// Resolved (symlinks-followed) path to the current deck
        /// binary. Tarball is extracted next to it and renamed over
        /// it; both files live on the same filesystem so the
        /// `rename(2)` is atomic.
        dest: PathBuf,
    },
    /// We can't safely write to the binary's location. Caller should
    /// surface a manual upgrade command (see `manual_upgrade_command`).
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
    InstallMethod::DirectDownload { dest: exe }
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
    // The resolved exe always traverses brew's Cellar directory, so
    // a prefix `starts_with` check is robust even when the user has
    // exotic symlinks pointing into the cellar from elsewhere.
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

/// Build the shell pipeline that runs inside the upgrade PTY for a
/// direct (non-brew) install. Quoted with single quotes around all
/// substituted paths so a `dest` containing spaces or shell metachars
/// is handled correctly; `version` and `target` we control.
pub fn direct_upgrade_command(version: &str, dest: &Path, target: &str) -> String {
    let url = release_url(version, target);
    let dest_str = shell_single_quote(&dest.display().to_string());
    format!(
        "set -e\n\
         DEST={dest}\n\
         echo \"Downloading deck v{ver} for {tgt}...\"\n\
         echo\n\
         TMP=$(mktemp -d)\n\
         trap 'rm -rf \"$TMP\"' EXIT\n\
         curl -fL --progress-bar '{url}' | tar xz -C \"$TMP\" --strip-components=1\n\
         chmod +x \"$TMP/deck\"\n\
         mv -f \"$TMP/deck\" \"$DEST.new\"\n\
         mv -f \"$DEST.new\" \"$DEST\"\n\
         echo\n\
         echo \"Upgraded to v{ver}. Restart deck to use it.\"\n",
        dest = dest_str,
        ver = version,
        tgt = target,
        url = url,
    )
}

/// Print the same logic in human form, for the Manual fallback when
/// deck can't safely overwrite its own binary.
pub fn manual_upgrade_command(version: &str, dest: &Path, target: &str) -> String {
    let url = release_url(version, target);
    format!(
        "# deck can't self-update from {dest}.\n\
         # Run this manually (you may need sudo for the final mv):\n\
         curl -fL '{url}' | tar xz\n\
         mv 'deck-v{ver}-{tgt}/deck' '{dest}'\n",
        dest = dest.display(),
        ver = version,
        tgt = target,
        url = url,
    )
}

fn release_url(version: &str, target: &str) -> String {
    format!(
        "https://github.com/cross-entropy-ai/deck/releases/download/v{ver}/deck-v{ver}-{tgt}.tar.gz",
        ver = version,
        tgt = target,
    )
}

/// Wrap `s` in single quotes, escaping any embedded single quotes
/// using the POSIX `'\''` trick. Keeps the upgrade shell command safe
/// against pathologically-named install paths.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_handles_normal_path() {
        assert_eq!(shell_single_quote("/usr/local/bin/deck"), "'/usr/local/bin/deck'");
    }

    #[test]
    fn quote_escapes_single_quote() {
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn url_matches_release_yml_format() {
        let url = release_url("0.3.0", "aarch64-apple-darwin");
        assert_eq!(
            url,
            "https://github.com/cross-entropy-ai/deck/releases/download/v0.3.0/deck-v0.3.0-aarch64-apple-darwin.tar.gz"
        );
    }

    #[test]
    fn target_triple_known_for_supported_platforms() {
        // The test runner runs on one of the supported targets, so
        // we should always get Some(_).
        assert!(target_triple().is_some());
    }
}

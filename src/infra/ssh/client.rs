//! SSH helpers for the remote-hosts feature.
//!
//! We use the system `ssh` client instead of re-parsing config: `ssh -G
//! <host>` prints the *effective* configuration after all `Host`/`Match`
//! blocks (including `Host *` wildcards) apply.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;

/// SSH options applied on *every* deck-initiated ssh invocation so all code
/// paths (one-shot tmux calls, attach PTY, port-forward control commands)
/// multiplex onto one ControlMaster socket per host, even when `ssh_config`
/// doesn't enable it. `BatchMode=yes` keeps ssh from blocking on an
/// interactive prompt in a background worker (a misconfigured host fails fast).
///
/// Single source of truth: every code path MUST pass this exact block, or
/// diverging options open separate masters and break connection sharing.
pub const CONTROL_OPTS: &[&str] = &[
    "-o",
    "ControlMaster=auto",
    "-o",
    "ControlPath=~/.ssh/cm-%r@%h:%p",
    "-o",
    "ControlPersist=10m",
    "-o",
    "ConnectTimeout=5",
    "-o",
    "ServerAliveInterval=30",
    "-o",
    "BatchMode=yes",
];

/// Result of querying `ssh -G <host>`. Keys are lowercased option names.
pub type SshEffectiveConfig = HashMap<String, String>;

/// Run `ssh -G <host>` and parse the output. Returns an error string if
/// ssh isn't on PATH or exits non-zero.
pub fn effective_config(host: &str) -> Result<SshEffectiveConfig, String> {
    let output = Command::new("ssh")
        .arg("-G")
        .arg(host)
        .output()
        .map_err(|e| format!("failed to invoke ssh: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ssh -G failed: {}", stderr.trim()));
    }
    let mut map = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((k, v)) = line.split_once(' ') {
            map.insert(k.to_ascii_lowercase(), v.to_string());
        }
    }
    Ok(map)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiplexStatus {
    pub control_master: Option<String>,
    pub control_path: Option<String>,
    pub control_persist: Option<String>,
}

impl MultiplexStatus {
    pub fn from_config(cfg: &SshEffectiveConfig) -> Self {
        MultiplexStatus {
            control_master: cfg.get("controlmaster").cloned(),
            control_path: cfg.get("controlpath").cloned(),
            control_persist: cfg.get("controlpersist").cloned(),
        }
    }

    /// True iff all three options are set in a way that actually enables
    /// connection sharing.
    pub fn is_enabled(&self) -> bool {
        let cm_ok = matches!(
            self.control_master.as_deref(),
            Some("auto" | "yes" | "ask" | "autoask")
        );
        let cp_ok = self
            .control_path
            .as_deref()
            .is_some_and(|s| !s.is_empty() && !s.eq_ignore_ascii_case("none"));
        let persist_ok = self
            .control_persist
            .as_deref()
            .is_some_and(|s| !matches!(s, "" | "no" | "0"));
        cm_ok && cp_ok && persist_ok
    }
}

/// Suggested ssh_config snippet to enable multiplexing for `host`.
pub fn suggested_snippet(host: &str) -> String {
    format!(
        "\nHost {host}\n    ControlMaster auto\n    ControlPath ~/.ssh/cm-%r@%h:%p\n    ControlPersist 10m\n",
        host = host
    )
}

fn ssh_config_path() -> PathBuf {
    crate::config::home_dir().join(".ssh").join("config")
}

/// Read `~/.ssh/config` and return its concrete `Host` aliases. A missing or
/// unreadable file yields an empty list (the picker still accepts typed input).
pub fn config_hosts() -> Vec<String> {
    let path = ssh_config_path();
    match std::fs::read_to_string(path) {
        Ok(content) => crate::infra::parser::ssh_config::parse_config_hosts(&content),
        Err(_) => Vec::new(),
    }
}

/// Append the snippet to `~/.ssh/config`, creating the file if needed.
pub fn append_to_ssh_config(snippet: &str) -> io::Result<PathBuf> {
    let path = ssh_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    f.write_all(snippet.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> SshEffectiveConfig {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn multiplex_enabled_when_all_three_set() {
        let c = cfg(&[
            ("controlmaster", "auto"),
            ("controlpath", "~/.ssh/cm-%r@%h:%p"),
            ("controlpersist", "600"),
        ]);
        assert!(MultiplexStatus::from_config(&c).is_enabled());
    }

    #[test]
    fn multiplex_disabled_when_persist_zero() {
        let c = cfg(&[
            ("controlmaster", "auto"),
            ("controlpath", "~/.ssh/cm"),
            ("controlpersist", "0"),
        ]);
        assert!(!MultiplexStatus::from_config(&c).is_enabled());
    }

    #[test]
    fn multiplex_disabled_when_path_none() {
        let c = cfg(&[
            ("controlmaster", "auto"),
            ("controlpath", "none"),
            ("controlpersist", "10m"),
        ]);
        assert!(!MultiplexStatus::from_config(&c).is_enabled());
    }

    #[test]
    fn multiplex_disabled_when_master_no() {
        let c = cfg(&[
            ("controlmaster", "no"),
            ("controlpath", "~/.ssh/cm"),
            ("controlpersist", "10m"),
        ]);
        assert!(!MultiplexStatus::from_config(&c).is_enabled());
    }

    #[test]
    fn suggested_snippet_contains_host() {
        let s = suggested_snippet("myhost");
        assert!(s.contains("Host myhost"));
        assert!(s.contains("ControlMaster auto"));
    }

}

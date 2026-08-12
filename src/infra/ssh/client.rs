//! SSH helpers for the remote-hosts feature.
//!
//! We use the system `ssh` client instead of re-parsing config: `ssh -G
//! <host>` prints the *effective* configuration after all `Host`/`Match`
//! blocks (including `Host *` wildcards) apply.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use crate::infra::command::{default_runner, CommandRunner};

/// The three Deck-owned connection-reuse settings. Ordinary SSH workers read
/// the process-wide snapshot immediately before spawning; the port-forward
/// worker carries its own snapshot so it can still close an old socket after a
/// path change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionSettings {
    pub enabled: bool,
    pub control_path: String,
    pub control_persist: String,
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            control_path: crate::config::DEFAULT_SSH_CONTROL_PATH.to_string(),
            control_persist: crate::config::DEFAULT_SSH_CONTROL_PERSIST.to_string(),
        }
    }
}

impl ConnectionSettings {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            enabled: config.ssh_connection_reuse,
            control_path: config.ssh_control_path.clone(),
            control_persist: config.ssh_control_persist.clone(),
        }
    }

    /// Whether moving from `self` to `next` abandons the socket `self` names,
    /// so masters at the old path must be closed (addressed with the *old*
    /// snapshot) and saved forwards re-established against the new one.
    ///
    /// False for a ControlPersist-only edit: the same master stays addressable,
    /// and `ssh -O exit` would kill the multiplexed `tmux attach` PTYs riding on
    /// it for nothing. Such an edit therefore applies to masters opened later,
    /// leaving live ones on their original idle timeout.
    pub fn abandons_socket(&self, next: &Self) -> bool {
        self.enabled && (!next.enabled || self.control_path != next.control_path)
    }

    /// Whether moving from `self` to `next` makes Deck re-establish every saved
    /// forward from scratch: either the old socket is gone, or reuse was off and
    /// no Deck-owned master existed to carry them. Callers that also diff
    /// per-rule forward changes must skip that diff exactly when this is true,
    /// or the two paths race each other over the same socket.
    pub fn rebuilds_forwards(&self, next: &Self) -> bool {
        next.enabled && (self.abandons_socket(next) || !self.enabled)
    }
}

static CONNECTION_SETTINGS: OnceLock<RwLock<ConnectionSettings>> = OnceLock::new();

const COMMON_OPTS: &[&str] = &[
    "-o",
    "ConnectTimeout=5",
    "-o",
    "ServerAliveInterval=30",
    "-o",
    "BatchMode=yes",
];

fn settings_lock() -> &'static RwLock<ConnectionSettings> {
    CONNECTION_SETTINGS.get_or_init(|| RwLock::new(ConnectionSettings::default()))
}

/// Replace the process-wide Deck SSH policy. Remote workers build argv just
/// before each spawn, so later settings/reload changes apply without restart.
pub fn configure_connection(settings: ConnectionSettings) {
    *settings_lock()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = settings;
}

pub fn connection_settings() -> ConnectionSettings {
    settings_lock()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// SSH options applied on every Deck-initiated SSH invocation. Both branches
/// explicitly override the three multiplexing options; common timeout,
/// keepalive, and non-interactive behavior remains unchanged.
///
/// The ControlPath value is wrapped in double quotes because OpenSSH tokenizes
/// a `-o` string the same way it tokenizes a config line: an unquoted space
/// ends the value and ssh dies with "keyword controlpath extra arguments at end
/// of line" (exit 255) on *every* invocation. Quoting is transparent to the
/// `~`, `%d`, and `%r@%h:%p` expansions ssh performs on the value, so it is
/// applied unconditionally rather than only for paths that look risky.
pub fn connection_opts_for(settings: &ConnectionSettings) -> Vec<String> {
    let mut opts = if settings.enabled {
        vec![
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            "-o".to_string(),
            format!(
                "ControlPath=\"{}\"",
                control_path_for_ssh(&settings.control_path)
            ),
            "-o".to_string(),
            format!("ControlPersist={}", settings.control_persist),
        ]
    } else {
        vec![
            "-o".to_string(),
            "ControlMaster=no".to_string(),
            "-o".to_string(),
            "ControlPath=none".to_string(),
            "-o".to_string(),
            "ControlPersist=no".to_string(),
        ]
    };
    opts.extend(COMMON_OPTS.iter().map(|value| (*value).to_string()));
    opts
}

/// OpenSSH expands `~`, `%d`, and `${HOME}` in ControlPath, but not the shell
/// spelling `$HOME`. Normalize the latter so the two common home spellings are
/// equivalent in Deck's setting. Keep every other token for OpenSSH itself.
fn control_path_for_ssh(value: &str) -> String {
    for prefix in ["$HOME/", "${HOME}/"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            return crate::config::home_dir()
                .join(rest)
                .to_string_lossy()
                .into_owned();
        }
    }
    if matches!(value, "$HOME" | "${HOME}") {
        return crate::config::home_dir().to_string_lossy().into_owned();
    }
    value.to_string()
}

pub fn connection_opts() -> Vec<String> {
    connection_opts_for(&connection_settings())
}

/// Create the directory holding Deck's ControlMaster sockets (the configured
/// ControlPath parent). OpenSSH never creates a missing parent itself. A newly
/// created directory gets `~/.ssh`-style 0700 permissions; an existing custom
/// directory is not chmodded out from under the user. Sockets remain managed
/// by OpenSSH.
pub fn ensure_control_dir(control_path: &str) -> io::Result<PathBuf> {
    let expanded = crate::config::expand_control_path_home(control_path);
    let dir = expanded.parent().unwrap_or_else(|| Path::new("."));
    // Defense in depth: `validate_ssh_control_path` already rejects these, so
    // neither a saved config nor the Settings editor can reach this arm.
    if dir.as_os_str().to_string_lossy().contains('%') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ControlPath directory contains host-dependent % tokens",
        ));
    }
    let dir = dir.to_path_buf();
    let existed = dir.exists();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let default_dir = crate::config::home_dir().join(".ssh").join("socks");
        if !existed || dir == default_dir {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(dir)
}

/// Result of querying `ssh -G <host>`. Keys are lowercased option names.
pub type SshEffectiveConfig = HashMap<String, String>;

const EFFECTIVE_CONFIG_TIMEOUT: Duration = Duration::from_secs(3);

/// Run `ssh -G <host>` and parse the output. Returns an error string if
/// ssh isn't on PATH or exits non-zero.
pub fn effective_config(host: &str) -> Result<SshEffectiveConfig, String> {
    effective_config_with(default_runner(), host)
}

fn effective_config_with(
    runner: &dyn CommandRunner,
    host: &str,
) -> Result<SshEffectiveConfig, String> {
    let output = runner
        .run("ssh", &["-G", host], EFFECTIVE_CONFIG_TIMEOUT)
        .map_err(|error| format!("ssh -G failed: {error}"))?;
    let mut map = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((k, v)) = line.split_once(' ') {
            map.insert(k.to_ascii_lowercase(), v.to_string());
        }
    }
    Ok(map)
}

fn ssh_config_path() -> PathBuf {
    crate::config::home_dir().join(".ssh").join("config")
}

/// Read `~/.ssh/config` and return its concrete `Host` aliases. A missing or
/// unreadable file yields an empty list (the picker still accepts typed input).
pub fn config_hosts() -> Vec<String> {
    std::fs::read_to_string(ssh_config_path())
        .map(|content| parse_config_hosts(&content))
        .unwrap_or_default()
}

/// Parse `~/.ssh/config` text into the list of concrete `Host` aliases. Keep
/// patterns without wildcard/negation chars (`*`, `?`, `!`), de-duped, in
/// first-seen order. Per-host options are irrelevant here — the picker only
/// needs the alias (later resolved via `ssh -G`).
fn parse_config_hosts(content: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let keyword = tokens.next().unwrap_or("");
        if !keyword.eq_ignore_ascii_case("host") {
            continue;
        }
        for pat in tokens {
            if pat.starts_with('#') {
                break; // rest of the line is an inline comment
            }
            if pat.contains('*') || pat.contains('?') || pat.contains('!') {
                continue;
            }
            if !hosts.iter().any(|h| h == pat) {
                hosts.push(pat.to_string());
            }
        }
    }
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EffectiveConfigRunner;

    impl CommandRunner for EffectiveConfigRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            timeout: Duration,
        ) -> Result<crate::infra::command::Output, crate::infra::command::CommandError> {
            assert_eq!(program, "ssh");
            assert_eq!(args, ["-G", "fixture"]);
            assert_eq!(timeout, EFFECTIVE_CONFIG_TIMEOUT);
            Ok(crate::infra::command::Output {
                stdout: b"hostname example.test\ncontrolmaster auto\n".to_vec(),
            })
        }
    }

    #[test]
    fn enabled_opts_force_deck_control_socket() {
        let opts = connection_opts_for(&ConnectionSettings::default()).join(" ");
        assert!(opts.contains("ControlMaster=auto"));
        assert!(opts.contains("ControlPath=\"~/.ssh/socks/cm-%r@%h:%p\""));
        assert!(opts.contains("ControlPersist=10m"));
        assert!(opts.contains("BatchMode=yes"));
    }

    #[test]
    fn control_path_value_is_quoted_so_a_space_cannot_split_the_option() {
        // ssh tokenizes a `-o` string like a config line: unquoted, a space
        // makes it reject the option and exit 255 on every invocation.
        let settings = ConnectionSettings {
            control_path: "/tmp/deck sockets/cm-%C".to_string(),
            ..ConnectionSettings::default()
        };
        let opts = connection_opts_for(&settings);
        assert!(opts.contains(&"ControlPath=\"/tmp/deck sockets/cm-%C\"".to_string()));
        // One argv element, quotes included — never split across two.
        assert_eq!(
            opts.iter()
                .filter(|opt| opt.contains("deck sockets"))
                .count(),
            1
        );
    }

    #[test]
    fn socket_transitions_distinguish_persist_edits_from_path_changes() {
        let on = ConnectionSettings::default();
        let off = ConnectionSettings {
            enabled: false,
            ..on.clone()
        };
        let other_path = ConnectionSettings {
            control_path: "~/.ssh/other/cm-%C".to_string(),
            ..on.clone()
        };
        let longer_persist = ConnectionSettings {
            control_persist: "1h".to_string(),
            ..on.clone()
        };

        // Persist-only: the same master stays addressable, so nothing is torn
        // down and no forward is re-established.
        assert!(!on.abandons_socket(&longer_persist));
        assert!(!on.rebuilds_forwards(&longer_persist));
        // Path change and turning reuse off both abandon the old socket.
        assert!(on.abandons_socket(&other_path));
        assert!(on.rebuilds_forwards(&other_path));
        assert!(on.abandons_socket(&off));
        assert!(!on.rebuilds_forwards(&off));
        // Turning reuse on owns no old socket but must restore the forwards.
        assert!(!off.abandons_socket(&on));
        assert!(off.rebuilds_forwards(&on));
    }

    #[test]
    fn effective_config_uses_bounded_runner_and_parses_output() {
        let config = effective_config_with(&EffectiveConfigRunner, "fixture").unwrap();
        assert_eq!(
            config.get("hostname").map(String::as_str),
            Some("example.test")
        );
        assert_eq!(
            config.get("controlmaster").map(String::as_str),
            Some("auto")
        );
    }

    #[test]
    fn disabled_opts_override_ssh_config() {
        let opts = connection_opts_for(&ConnectionSettings {
            enabled: false,
            ..ConnectionSettings::default()
        })
        .join(" ");
        assert!(opts.contains("ControlMaster=no"));
        assert!(opts.contains("ControlPath=none"));
        assert!(opts.contains("ControlPersist=no"));
        assert!(opts.contains("BatchMode=yes"));
    }

    #[test]
    fn expands_supported_home_prefixes_for_control_directory_creation() {
        use crate::config::expand_control_path_home as expand;
        let home = crate::config::home_dir();
        assert_eq!(expand("~/deck/cm-%C"), home.join("deck/cm-%C"));
        assert_eq!(expand("$HOME/deck/cm-%C"), home.join("deck/cm-%C"));
        assert_eq!(expand("${HOME}/deck/cm-%C"), home.join("deck/cm-%C"));
        assert_eq!(expand("%d/deck/cm-%C"), home.join("deck/cm-%C"));
    }

    #[test]
    fn bare_home_variable_is_normalized_before_invoking_ssh() {
        let settings = ConnectionSettings {
            control_path: "$HOME/.cache/deck/cm-%C".to_string(),
            ..ConnectionSettings::default()
        };
        let opts = connection_opts_for(&settings);
        let expected = format!(
            "ControlPath=\"{}\"",
            crate::config::home_dir()
                .join(".cache/deck/cm-%C")
                .display()
        );
        assert!(opts.contains(&expected));
    }

    #[test]
    fn rejects_host_dependent_tokens_in_control_path_directory() {
        let error = ensure_control_dir("/tmp/deck-%h/cm-%C").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn parses_concrete_hosts_excluding_wildcards() {
        let sample = "\
# work hosts
Host prod-web-1
    HostName 10.0.0.1
    User deploy

Host prod-web-2 staging
    HostName 10.0.0.2

Host *
    ServerAliveInterval 30

Host build-?
    HostName builder

Host !secret
    HostName x
";
        let hosts = parse_config_hosts(sample);
        assert_eq!(hosts, vec!["prod-web-1", "prod-web-2", "staging"]);
    }

    #[test]
    fn dedups_and_preserves_first_seen_order() {
        let sample = "Host a\nHost b\nHost a\n";
        assert_eq!(parse_config_hosts(sample), vec!["a", "b"]);
    }

    #[test]
    fn empty_or_no_hosts_yields_empty() {
        assert!(parse_config_hosts("").is_empty());
        assert!(parse_config_hosts("# comment only\n  IdentityFile ~/x\n").is_empty());
    }

    #[test]
    fn host_line_inline_comment_is_ignored() {
        let sample = "Host web # primary box\nHost db\n";
        assert_eq!(parse_config_hosts(sample), vec!["web", "db"]);
    }
}

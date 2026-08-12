//! Pure builders for the ssh subcommands that manage port forwards against a
//! host's ControlMaster. No IO here; callers spawn the returned `Command`s on
//! their own threads. All builders pass the same `ControlMaster`/`ControlPath`/
//! `ControlPersist` block so this worker and `app::ssh::remote_spawn` share the
//! same master socket per host. Port-forward control commands always use the
//! enabled Deck-owned socket options: the UI prevents creating forwards while
//! connection reuse is disabled, but cleanup must still reach an existing
//! master after a settings change.

use std::process::Command;

use crate::forwards::ForwardSpec;

/// The enabled Deck connection options plus `host`'s ForwardAgent setting,
/// followed by `host`, so this worker and multiplexed remote connections reach
/// the same master socket per host.
pub fn ssh_args_for_host(settings: &crate::ssh::ConnectionSettings, host: &str) -> Vec<String> {
    let mut enabled = settings.clone();
    enabled.enabled = true;
    crate::ssh::connection_opts_for(&enabled)
        .into_iter()
        .chain(
            crate::ssh::agent_forward_opts(host)
                .iter()
                .map(|opt| (*opt).to_string()),
        )
        .chain(std::iter::once(host.to_string()))
        .collect()
}

fn ssh_with(settings: &crate::ssh::ConnectionSettings, host: &str, leading: &[&str]) -> Command {
    let mut c = Command::new("ssh");
    for a in leading {
        c.arg(a);
    }
    for a in ssh_args_for_host(settings, host) {
        c.arg(a);
    }
    c
}

/// `ssh -fN <opts> <host>` — fork master into background, no remote command.
/// Returns immediately once the master is ready.
pub fn build_master_cmd(settings: &crate::ssh::ConnectionSettings, host: &str) -> Command {
    ssh_with(settings, host, &["-f", "-N"])
}

/// `ssh -O forward -L 8080:host:80 <opts> <host>` — add a forward to
/// the existing master. Fails with non-zero exit if master isn't up.
pub fn build_forward_cmd(
    settings: &crate::ssh::ConnectionSettings,
    host: &str,
    spec: &ForwardSpec,
) -> Command {
    let (flag, value) = spec.ssh_flag_and_value();
    ssh_with(settings, host, &["-O", "forward", flag, value.as_str()])
}

/// `ssh -O cancel -L 8080:host:80 <opts> <host>` — remove a forward.
pub fn build_cancel_cmd(
    settings: &crate::ssh::ConnectionSettings,
    host: &str,
    spec: &ForwardSpec,
) -> Command {
    let (flag, value) = spec.ssh_flag_and_value();
    ssh_with(settings, host, &["-O", "cancel", flag, value.as_str()])
}

/// `ssh -O exit <opts> <host>` — tear down the master entirely.
pub fn build_exit_cmd(settings: &crate::ssh::ConnectionSettings, host: &str) -> Command {
    ssh_with(settings, host, &["-O", "exit"])
}

#[cfg(test)]
#[path = "../../../tests/unit/infra/port_forward.rs"]
mod tests;

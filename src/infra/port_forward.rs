//! Pure builders for the ssh subcommands used to manage port forwards
//! against a host's ControlMaster. No IO happens here; callers spawn
//! the returned `Command`s on their own threads.
//!
//! All builders pass the same `-o ControlMaster=auto -o ControlPath=…
//! -o ControlPersist=…` block so this worker and `app::remote_spawn`
//! share the same master socket per host.

use std::process::Command;

use crate::config::ForwardSpec;

/// The common ssh argument block: control options + host. Keep in sync
/// with `app/remote_spawn.rs` so both code paths reach the same master.
// TODO(post-feature): consolidate this with remote_tmux::base_ssh_args
// and remote_spawn's ssh-options block (port-forward design doc, Open
// Question 1). All three must stay in sync for ControlMaster sharing
// to work.
pub fn ssh_args_for_host(host: &str) -> Vec<String> {
    vec![
        "-o".into(), "ControlMaster=auto".into(),
        "-o".into(), "ControlPath=~/.ssh/cm-%r@%h:%p".into(),
        "-o".into(), "ControlPersist=10m".into(),
        "-o".into(), "ConnectTimeout=5".into(),
        "-o".into(), "ServerAliveInterval=30".into(),
        "-o".into(), "BatchMode=yes".into(),
        host.into(),
    ]
}

fn ssh_with(host: &str, leading: &[&str]) -> Command {
    let mut c = Command::new("ssh");
    for a in leading {
        c.arg(a);
    }
    for a in ssh_args_for_host(host) {
        c.arg(a);
    }
    c
}

/// `ssh -fN <opts> <host>` — fork master into background, no remote command.
/// Returns immediately once the master is ready.
pub fn build_master_cmd(host: &str) -> Command {
    ssh_with(host, &["-f", "-N"])
}

/// `ssh -O forward -L 8080:host:80 <opts> <host>` — add a forward to
/// the existing master. Fails with non-zero exit if master isn't up.
pub fn build_forward_cmd(host: &str, spec: &ForwardSpec) -> Command {
    let (flag, value) = spec.ssh_flag_and_value();
    let mut c = Command::new("ssh");
    c.arg("-O").arg("forward").arg(flag).arg(value);
    for a in ssh_args_for_host(host) {
        c.arg(a);
    }
    c
}

/// `ssh -O cancel -L 8080:host:80 <opts> <host>` — remove a forward.
pub fn build_cancel_cmd(host: &str, spec: &ForwardSpec) -> Command {
    let (flag, value) = spec.ssh_flag_and_value();
    let mut c = Command::new("ssh");
    c.arg("-O").arg("cancel").arg(flag).arg(value);
    for a in ssh_args_for_host(host) {
        c.arg(a);
    }
    c
}

/// `ssh -O exit <opts> <host>` — tear down the master entirely.
pub fn build_exit_cmd(host: &str) -> Command {
    ssh_with(host, &["-O", "exit"])
}

#[cfg(test)]
#[path = "../../tests/unit/infra/port_forward.rs"]
mod tests;

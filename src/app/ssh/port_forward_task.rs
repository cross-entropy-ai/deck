//! Port-forward worker. Owns SSH process lifecycle: per-host ControlMaster
//! bring-up and individual `-O forward / -O cancel` calls. UI thread sends
//! `Op` messages on a channel; the worker returns one `OpResult` per step.
//!
//! The I/O-bearing logic (process tracking, threading) lives here, separate
//! from `infra::ssh::port_forward`, so it's testable via the `Runner` trait
//! without shelling out to real `ssh`.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

use crate::forwards::ForwardSpec;

/// Commands the UI sends to the worker.
#[derive(Debug)]
pub enum Op {
    /// Bring up master + apply every spec, host-by-host, in given order.
    Bootstrap {
        hosts: Vec<(String, Vec<ForwardSpec>)>,
    },
    AddForward {
        host: String,
        spec: ForwardSpec,
    },
    CancelForward {
        host: String,
        spec: ForwardSpec,
    },
    /// Atomically move the worker from one Deck-owned ControlPath/Persist
    /// snapshot to another. Old masters are addressed with the old snapshot;
    /// saved forwards are then restored only when the new snapshot is enabled.
    Reconfigure {
        settings: crate::ssh::ConnectionSettings,
        stop_hosts: Vec<String>,
        forward_hosts: Vec<(String, Vec<ForwardSpec>)>,
    },
    /// Tear down the host's master entirely (used when a host is removed
    /// from config via hot-reload).
    StopHost {
        host: String,
    },
}

/// Identifier for what the result is reporting on. Mirrored on
/// `PfAction::TaskResult` so the reducer can pick the right place to
/// surface the message.
#[derive(Debug, Clone)]
pub enum OpKind {
    Master(String),
    Forward(String, ForwardSpec),
    Cancel(String),
    Exit(String),
}

impl OpKind {
    /// The host this result pertains to.
    pub fn host(&self) -> &str {
        match self {
            OpKind::Master(h) | OpKind::Exit(h) => h,
            OpKind::Forward(h, _) => h,
            OpKind::Cancel(h) => h,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpResult {
    pub kind: OpKind,
    pub ok: bool,
    pub message: String,
}

impl OpResult {
    pub(crate) fn into_lane_result(
        self,
    ) -> (crate::lane::LaneId, crate::action::PfTaskKind, bool, String) {
        let host = self.kind.host().to_string();
        let kind = match self.kind {
            OpKind::Master(_) => crate::action::PfTaskKind::Master,
            OpKind::Forward(_, spec) => crate::action::PfTaskKind::Forward(spec),
            OpKind::Cancel(_) => crate::action::PfTaskKind::Cancel,
            OpKind::Exit(_) => crate::action::PfTaskKind::Exit,
        };
        (
            crate::system::tmux::TmuxSystem::host_lane(&host),
            kind,
            self.ok,
            self.message,
        )
    }
}

/// Indirection over actually shelling out — lets tests verify ordering
/// without spawning ssh.
pub trait Runner: Send + 'static {
    fn run_master(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
    ) -> Result<(), String>;
    fn run_forward(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
        spec: &ForwardSpec,
    ) -> Result<(), String>;
    fn run_cancel(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
        spec: &ForwardSpec,
    ) -> Result<(), String>;
    fn run_exit(&self, settings: &crate::ssh::ConnectionSettings, host: &str)
        -> Result<(), String>;
}

/// The default Runner — actually shells out via `infra::ssh::port_forward`.
pub struct SshRunner;

impl Runner for SshRunner {
    fn run_master(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
    ) -> Result<(), String> {
        let mut cmd = crate::infra::ssh::port_forward::build_master_cmd(settings, host);
        run_bounded(&mut cmd)
    }
    fn run_forward(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
        spec: &ForwardSpec,
    ) -> Result<(), String> {
        let mut cmd = crate::infra::ssh::port_forward::build_forward_cmd(settings, host, spec);
        run_bounded(&mut cmd)
    }
    fn run_cancel(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
        spec: &ForwardSpec,
    ) -> Result<(), String> {
        let mut cmd = crate::infra::ssh::port_forward::build_cancel_cmd(settings, host, spec);
        run_bounded(&mut cmd)
    }
    fn run_exit(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
    ) -> Result<(), String> {
        let mut cmd = crate::infra::ssh::port_forward::build_exit_cmd(settings, host);
        run_bounded(&mut cmd)
    }
}

fn run_bounded(cmd: &mut std::process::Command) -> Result<(), String> {
    const PORT_FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
    let program = cmd.get_program().to_string_lossy().into_owned();
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::infra::command::default_runner()
        .run(&program, &args, PORT_FORWARD_TIMEOUT)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Pure command-handling core. Carries the per-host master-up set
/// across calls. `handle()` is sync; the public `spawn()` glues it
/// to an mpsc channel and a thread.
pub struct Worker<R: Runner> {
    runner: R,
    settings: crate::ssh::ConnectionSettings,
    masters_up: HashSet<String>,
}

impl<R: Runner> Worker<R> {
    pub fn new(runner: R, settings: crate::ssh::ConnectionSettings) -> Self {
        Self {
            runner,
            settings,
            masters_up: HashSet::new(),
        }
    }

    pub fn handle(&mut self, op: Op) -> Vec<OpResult> {
        match op {
            Op::Bootstrap { hosts } => {
                let mut out = Vec::new();
                if self.settings.enabled {
                    self.bootstrap(hosts, &mut out);
                }
                out
            }
            Op::AddForward { host, spec } => {
                let mut out = Vec::new();
                if !self.settings.enabled {
                    return out;
                }
                if !self.ensure_master(&host, &mut out) {
                    return out;
                }
                let r = self.runner.run_forward(&self.settings, &host, &spec);
                out.push(result_from(OpKind::Forward(host, spec), r));
                out
            }
            Op::CancelForward { host, spec } => {
                if !self.settings.enabled {
                    return Vec::new();
                }
                let r = self.runner.run_cancel(&self.settings, &host, &spec);
                vec![result_from(OpKind::Cancel(host), r)]
            }
            Op::Reconfigure {
                settings,
                stop_hosts,
                forward_hosts,
            } => {
                let old_settings = std::mem::replace(&mut self.settings, settings);
                let mut out = Vec::new();
                // `ssh -O exit` kills the master *and* every session multiplexed
                // on it, including Deck's live `tmux attach` PTYs — so only close
                // sockets actually being abandoned, and only re-establish
                // forwards that lost their master. A ControlPersist-only edit
                // satisfies neither and is a no-op here: live connections stay
                // up, and the new idle timeout applies to later masters.
                let closes_old = old_settings.abandons_socket(&self.settings);
                let restores = old_settings.rebuilds_forwards(&self.settings);
                if !closes_old && !restores {
                    return out;
                }
                if closes_old {
                    let mut stopped = HashSet::new();
                    for host in stop_hosts {
                        if stopped.insert(host.clone()) {
                            let r = self.runner.run_exit(&old_settings, &host);
                            out.push(result_from(OpKind::Exit(host), r));
                        }
                    }
                }
                self.masters_up.clear();
                if restores {
                    self.bootstrap(forward_hosts, &mut out);
                }
                out
            }
            Op::StopHost { host } => {
                self.masters_up.remove(&host);
                if self.settings.enabled {
                    let r = self.runner.run_exit(&self.settings, &host);
                    vec![result_from(OpKind::Exit(host), r)]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn bootstrap(&mut self, hosts: Vec<(String, Vec<ForwardSpec>)>, out: &mut Vec<OpResult>) {
        for (host, specs) in hosts {
            if !self.ensure_master(&host, out) {
                continue;
            }
            for spec in specs {
                let r = self.runner.run_forward(&self.settings, &host, &spec);
                out.push(result_from(OpKind::Forward(host.clone(), spec), r));
            }
        }
    }

    /// Bring the host's master up if not already. Returns true on success.
    /// Records the master attempt result in `out`.
    fn ensure_master(&mut self, host: &str, out: &mut Vec<OpResult>) -> bool {
        if self.masters_up.contains(host) {
            return true;
        }
        let r = self.runner.run_master(&self.settings, host);
        let ok = r.is_ok();
        out.push(result_from(OpKind::Master(host.to_string()), r));
        if ok {
            self.masters_up.insert(host.to_string());
        }
        ok
    }
}

fn result_from(kind: OpKind, r: Result<(), String>) -> OpResult {
    match r {
        Ok(()) => OpResult {
            kind,
            ok: true,
            message: String::new(),
        },
        Err(message) => OpResult {
            kind,
            ok: false,
            message,
        },
    }
}

/// Spawn a worker thread that reads `Op`s and forwards `OpResult`s.
/// Returns the channel sender. The thread runs until the sender is
/// dropped.
pub fn spawn(results: Sender<OpResult>, settings: crate::ssh::ConnectionSettings) -> Sender<Op> {
    let (op_tx, op_rx): (Sender<Op>, Receiver<Op>) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("deck-port-forward".into())
        .spawn(move || {
            let mut worker = Worker::new(SshRunner, settings);
            for op in op_rx {
                for r in worker.handle(op) {
                    if results.send(r).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("port-forward worker thread");
    op_tx
}

/// Stop SSH forwarding for a lane at the built-in adapter boundary. Generic
/// effect routing remains lane-keyed and never decodes the lane payload.
pub(crate) fn stop_lane(sender: &Sender<Op>, lane: &crate::lane::LaneId) {
    if let Some(host) = crate::system::tmux::TmuxSystem::host_of(lane) {
        let _ = sender.send(Op::StopHost {
            host: host.to_string(),
        });
    }
}

pub(crate) fn add_for_lane(sender: &Sender<Op>, lane: &crate::lane::LaneId, spec: ForwardSpec) {
    if let Some(host) = crate::system::tmux::TmuxSystem::host_of(lane) {
        let _ = sender.send(Op::AddForward {
            host: host.to_string(),
            spec,
        });
    }
}

pub(crate) fn cancel_for_lane(sender: &Sender<Op>, lane: &crate::lane::LaneId, spec: ForwardSpec) {
    if let Some(host) = crate::system::tmux::TmuxSystem::host_of(lane) {
        let _ = sender.send(Op::CancelForward {
            host: host.to_string(),
            spec,
        });
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/app/port_forward_task.rs"]
mod tests;

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

/// Indirection over actually shelling out — lets tests verify ordering
/// without spawning ssh.
pub trait Runner: Send + 'static {
    fn run_master(&self, host: &str) -> Result<(), String>;
    fn run_forward(&self, host: &str, spec: &ForwardSpec) -> Result<(), String>;
    fn run_cancel(&self, host: &str, spec: &ForwardSpec) -> Result<(), String>;
    fn run_exit(&self, host: &str) -> Result<(), String>;
}

/// The default Runner — actually shells out via `infra::ssh::port_forward`.
pub struct SshRunner;

impl Runner for SshRunner {
    fn run_master(&self, host: &str) -> Result<(), String> {
        let mut cmd = crate::infra::ssh::port_forward::build_master_cmd(host);
        run_bounded(&mut cmd)
    }
    fn run_forward(&self, host: &str, spec: &ForwardSpec) -> Result<(), String> {
        let mut cmd = crate::infra::ssh::port_forward::build_forward_cmd(host, spec);
        run_bounded(&mut cmd)
    }
    fn run_cancel(&self, host: &str, spec: &ForwardSpec) -> Result<(), String> {
        let mut cmd = crate::infra::ssh::port_forward::build_cancel_cmd(host, spec);
        run_bounded(&mut cmd)
    }
    fn run_exit(&self, host: &str) -> Result<(), String> {
        let mut cmd = crate::infra::ssh::port_forward::build_exit_cmd(host);
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
    masters_up: HashSet<String>,
}

impl<R: Runner> Worker<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            masters_up: HashSet::new(),
        }
    }

    pub fn handle(&mut self, op: Op) -> Vec<OpResult> {
        match op {
            Op::Bootstrap { hosts } => {
                let mut out = Vec::new();
                for (host, specs) in hosts {
                    let master_ok = self.ensure_master(&host, &mut out);
                    if !master_ok {
                        continue;
                    }
                    for spec in specs {
                        let r = self.runner.run_forward(&host, &spec);
                        out.push(result_from(OpKind::Forward(host.clone(), spec), r));
                    }
                }
                out
            }
            Op::AddForward { host, spec } => {
                let mut out = Vec::new();
                if !self.ensure_master(&host, &mut out) {
                    return out;
                }
                let r = self.runner.run_forward(&host, &spec);
                out.push(result_from(OpKind::Forward(host, spec), r));
                out
            }
            Op::CancelForward { host, spec } => {
                let r = self.runner.run_cancel(&host, &spec);
                vec![result_from(OpKind::Cancel(host), r)]
            }
            Op::StopHost { host } => {
                let r = self.runner.run_exit(&host);
                self.masters_up.remove(&host);
                vec![result_from(OpKind::Exit(host), r)]
            }
        }
    }

    /// Bring the host's master up if not already. Returns true on success.
    /// Records the master attempt result in `out`.
    fn ensure_master(&mut self, host: &str, out: &mut Vec<OpResult>) -> bool {
        if self.masters_up.contains(host) {
            return true;
        }
        let r = self.runner.run_master(host);
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
pub fn spawn(results: Sender<OpResult>) -> Sender<Op> {
    let (op_tx, op_rx): (Sender<Op>, Receiver<Op>) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("deck-port-forward".into())
        .spawn(move || {
            let mut worker = Worker::new(SshRunner);
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

#[cfg(test)]
#[path = "../../../tests/unit/app/port_forward_task.rs"]
mod tests;

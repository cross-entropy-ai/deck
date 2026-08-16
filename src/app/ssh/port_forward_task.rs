//! Port-forward worker. Owns SSH process lifecycle: per-host ControlMaster
//! bring-up and individual `-O forward / -O cancel` calls. UI thread sends
//! `Op` messages on a channel; the worker returns one `OpResult` per step.
//!
//! The I/O-bearing logic (process tracking, threading) lives here, separate
//! from `infra::ssh::port_forward`, so it's testable via the `Runner` trait
//! without shelling out to real `ssh`.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

use crate::forwards::ForwardSpec;
use crate::lane::LaneId;

/// A container a forward points into, as the engine on the host knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerEndpoint {
    pub engine: String,
    pub name: String,
}

/// Everything a forward needs about where it lives: which lane owns it, which
/// ssh destination its `-O` commands address, and — for a container lane — what
/// to ask the engine about.
///
/// The three used to be one `host: String`, which worked only while they could
/// not differ. A container lane's forward is reported to *its* lane, runs over
/// its *host's* master, and points at an endpoint neither of those names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardEndpoint {
    /// Identity: whose overlay a result belongs to.
    pub lane: LaneId,
    /// ssh destination owning the ControlMaster. Always a host, never a
    /// container id — see `config_adapter::forward_endpoint`.
    pub host: String,
    /// `None` on a host lane: the target endpoint is whatever the rule says.
    pub container: Option<ContainerEndpoint>,
}

/// Commands the UI sends to the worker.
#[derive(Debug)]
pub enum Op {
    /// Bring up master + apply every spec, lane-by-lane, in given order.
    Bootstrap {
        lanes: Vec<(ForwardEndpoint, Vec<ForwardSpec>)>,
    },
    AddForward {
        endpoint: ForwardEndpoint,
        spec: ForwardSpec,
    },
    CancelForward {
        endpoint: ForwardEndpoint,
        spec: ForwardSpec,
    },
    /// Atomically move the worker from one Deck-owned ControlPath/Persist
    /// snapshot to another. Old masters are addressed with the old snapshot;
    /// saved forwards are then restored only when the new snapshot is enabled.
    Reconfigure {
        settings: crate::ssh::ConnectionSettings,
        stop_hosts: Vec<MasterTarget>,
        forward_lanes: Vec<(ForwardEndpoint, Vec<ForwardSpec>)>,
    },
    /// Tear down the host's master entirely (used when a host is removed
    /// from config via hot-reload).
    StopHost { target: MasterTarget },
}

/// A ControlMaster to close: the ssh destination, and the lane whose overlay
/// hears the outcome. Only ever a host — `ssh -O exit` kills the master and
/// every session multiplexed on it, so a container lane must never reach here
/// (it shares its host's master with that host's live PTYs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterTarget {
    pub lane: LaneId,
    pub host: String,
}

/// Identifier for what the result is reporting on. Mirrored on
/// `PfAction::TaskResult` so the reducer can pick the right place to
/// surface the message.
#[derive(Debug, Clone)]
pub enum OpKind {
    Master(LaneId),
    Forward(LaneId, ForwardSpec),
    Cancel(LaneId),
    Exit(LaneId),
}

impl OpKind {
    /// The lane this result pertains to.
    pub fn lane(&self) -> &LaneId {
        match self {
            OpKind::Master(lane) | OpKind::Exit(lane) => lane,
            OpKind::Forward(lane, _) => lane,
            OpKind::Cancel(lane) => lane,
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
        let lane = self.kind.lane().clone();
        let kind = match self.kind {
            OpKind::Master(_) => crate::action::PfTaskKind::Master,
            OpKind::Forward(_, spec) => crate::action::PfTaskKind::Forward(spec),
            OpKind::Cancel(_) => crate::action::PfTaskKind::Cancel,
            OpKind::Exit(_) => crate::action::PfTaskKind::Exit,
        };
        (lane, kind, self.ok, self.message)
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
    /// Ask `host`'s engine where a `-L` into `container` should point, as an
    /// `addr:port` reachable from the host. Blocking (one ssh hop), like the
    /// rest of this trait.
    fn resolve_container_target(
        &self,
        host: &str,
        container: &ContainerEndpoint,
        port: u16,
    ) -> Result<String, String>;
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
    fn resolve_container_target(
        &self,
        host: &str,
        container: &ContainerEndpoint,
        port: u16,
    ) -> Result<String, String> {
        crate::remote_tmux::container_forward_target(host, &container.engine, &container.name, port)
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
    /// Where each live container forward was pointed when it was created.
    ///
    /// Cancelling has to name the same endpoint the forward was added with, and
    /// a container's address is resolved fresh every apply — so re-resolving at
    /// cancel time would name a *different* endpoint the moment the container
    /// restarted, and the listener would stay up with nothing to close it.
    /// Keyed by the listener, which is what ssh can only hold one of.
    resolved: HashMap<(LaneId, ForwardKey), String>,
}

/// The listener half of a forward — its identity as far as ssh is concerned,
/// since one of those is all a port can carry.
type ForwardKey = (crate::forwards::ForwardMode, Option<String>, u16);

fn forward_key(spec: &ForwardSpec) -> ForwardKey {
    (spec.mode, spec.bind_addr.clone(), spec.listen_port)
}

impl<R: Runner> Worker<R> {
    pub fn new(runner: R, settings: crate::ssh::ConnectionSettings) -> Self {
        Self {
            runner,
            settings,
            masters_up: HashSet::new(),
            resolved: HashMap::new(),
        }
    }

    pub fn handle(&mut self, op: Op) -> Vec<OpResult> {
        match op {
            Op::Bootstrap { lanes } => {
                let mut out = Vec::new();
                if self.settings.enabled {
                    self.bootstrap(lanes, &mut out);
                }
                out
            }
            Op::AddForward { endpoint, spec } => {
                let mut out = Vec::new();
                if !self.settings.enabled {
                    return out;
                }
                if !self.ensure_master(&endpoint, &mut out) {
                    return out;
                }
                self.forward(&endpoint, spec, &mut out);
                out
            }
            Op::CancelForward { endpoint, spec } => {
                if !self.settings.enabled {
                    return Vec::new();
                }
                let concrete = match self.cancel_spec(&endpoint, &spec) {
                    Ok(concrete) => concrete,
                    Err(message) => {
                        return vec![OpResult {
                            kind: OpKind::Cancel(endpoint.lane),
                            ok: false,
                            message,
                        }]
                    }
                };
                let r = self
                    .runner
                    .run_cancel(&self.settings, &endpoint.host, &concrete);
                self.resolved
                    .remove(&(endpoint.lane.clone(), forward_key(&spec)));
                vec![result_from(OpKind::Cancel(endpoint.lane), r)]
            }
            Op::Reconfigure {
                settings,
                stop_hosts,
                forward_lanes,
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
                    for target in stop_hosts {
                        if stopped.insert(target.host.clone()) {
                            let r = self.runner.run_exit(&old_settings, &target.host);
                            out.push(result_from(OpKind::Exit(target.lane), r));
                        }
                    }
                }
                self.masters_up.clear();
                // Every remembered address belonged to a master that is gone;
                // the rebuild below resolves each container again.
                self.resolved.clear();
                if restores {
                    self.bootstrap(forward_lanes, &mut out);
                }
                out
            }
            Op::StopHost { target } => {
                self.masters_up.remove(&target.host);
                self.resolved.retain(|(lane, _), _| *lane != target.lane);
                if self.settings.enabled {
                    let r = self.runner.run_exit(&self.settings, &target.host);
                    vec![result_from(OpKind::Exit(target.lane), r)]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn bootstrap(
        &mut self,
        lanes: Vec<(ForwardEndpoint, Vec<ForwardSpec>)>,
        out: &mut Vec<OpResult>,
    ) {
        for (endpoint, specs) in lanes {
            if !self.ensure_master(&endpoint, out) {
                continue;
            }
            for spec in specs {
                self.forward(&endpoint, spec, out);
            }
        }
    }

    /// Apply one forward, resolving a container's endpoint first and
    /// remembering where it landed so the cancel can name the same one.
    fn forward(&mut self, endpoint: &ForwardEndpoint, spec: ForwardSpec, out: &mut Vec<OpResult>) {
        let concrete = match self.resolve(endpoint, &spec) {
            Ok(concrete) => concrete,
            // A container that cannot be reached is reported as the forward
            // failing, which is what it is: nothing was bound, and the user's
            // rule is not silently kept as if it had been.
            Err(message) => {
                out.push(OpResult {
                    kind: OpKind::Forward(endpoint.lane.clone(), spec),
                    ok: false,
                    message,
                });
                return;
            }
        };
        let r = self
            .runner
            .run_forward(&self.settings, &endpoint.host, &concrete);
        if r.is_ok() && endpoint.container.is_some() {
            // Both halves: a published port moves the address *and* the port
            // away from what the rule says, so remembering the address alone
            // would later cancel a listener that was never opened.
            self.resolved.insert(
                (endpoint.lane.clone(), forward_key(&spec)),
                format!(
                    "{}:{}",
                    concrete.target_host.clone().unwrap_or_default(),
                    concrete.target_port.unwrap_or_default()
                ),
            );
        }
        out.push(result_from(OpKind::Forward(endpoint.lane.clone(), spec), r));
    }

    /// The spec as ssh must see it: unchanged on a host lane, and with the
    /// container's current address filled in on a container lane.
    fn resolve(
        &self,
        endpoint: &ForwardEndpoint,
        spec: &ForwardSpec,
    ) -> Result<ForwardSpec, String> {
        let Some(container) = endpoint.container.as_ref() else {
            return Ok(spec.clone());
        };
        // A container lane offers `-L` only (see `ForwardEndpointKind`), so a rule
        // without a target port is one this lane could not have created.
        let port = spec
            .target_port
            .ok_or_else(|| "a container forward needs a port inside the container".to_string())?;
        let target = self
            .runner
            .resolve_container_target(&endpoint.host, container, port)?;
        Ok(with_target(spec, &target))
    }

    /// The spec to hand `-O cancel`: the endpoint the forward was created with,
    /// remembered at add time. Falls back to resolving again — the master can
    /// outlive Deck (ControlPersist), so a forward this process never added is
    /// still cancellable as long as the container has not moved.
    fn cancel_spec(
        &self,
        endpoint: &ForwardEndpoint,
        spec: &ForwardSpec,
    ) -> Result<ForwardSpec, String> {
        if endpoint.container.is_none() {
            return Ok(spec.clone());
        }
        match self
            .resolved
            .get(&(endpoint.lane.clone(), forward_key(spec)))
        {
            Some(target) => Ok(with_target(spec, target)),
            None => self.resolve(endpoint, spec),
        }
    }

    /// Bring the endpoint's master up if not already. Returns true on success.
    /// Records the master attempt result in `out`.
    fn ensure_master(&mut self, endpoint: &ForwardEndpoint, out: &mut Vec<OpResult>) -> bool {
        if self.masters_up.contains(&endpoint.host) {
            return true;
        }
        let r = self.runner.run_master(&self.settings, &endpoint.host);
        let ok = r.is_ok();
        out.push(result_from(OpKind::Master(endpoint.lane.clone()), r));
        if ok {
            self.masters_up.insert(endpoint.host.clone());
        }
        ok
    }
}

/// The rule as ssh must see it, with a resolved `addr:port` filled in.
fn with_target(spec: &ForwardSpec, target: &str) -> ForwardSpec {
    let (host, port) = match target.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => (host, Some(port)),
            Err(_) => (target, None),
        },
        None => (target, None),
    };
    ForwardSpec {
        target_host: Some(host.to_string()),
        // No parseable port leaves the rule's own in place: that is the port
        // inside the container, which is what the user asked for.
        target_port: port.or(spec.target_port),
        ..spec.clone()
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
///
/// The ControlMaster this lane owns, or `None` for a lane that owns none — the
/// local lane, and a container lane, which rides its host's. A container's
/// forwards are cancelled one by one instead (`cancel_all_for_lane`): `-O exit`
/// on the master it shares would take the host's live PTYs with it.
///
/// Handing the worker a raw remote id would make it run `ssh -O … 'host#container'`
/// with the id as a *hostname*: harmless-looking with the default per-host
/// ControlPath, but a user-set literal path makes all hosts share one socket, and
/// then removing a container lane would `-O exit` that shared master and kill
/// every live deck PTY multiplexed on it.
pub(crate) fn master_target(lane: &crate::lane::LaneId) -> Option<MasterTarget> {
    let remote_id = crate::system::tmux::TmuxSystem::host_of(lane)?;
    crate::remote_tmux::parse_remote_id(remote_id)
        .container
        .is_none()
        .then(|| MasterTarget {
            lane: lane.clone(),
            host: remote_id.to_string(),
        })
}

/// Release whatever a lane holds in the worker: its master if it owns one,
/// otherwise each of its own forwards. Called when a lane leaves the config —
/// without it, a removed container's listeners stay bound until its host's
/// master happens to expire.
pub(crate) fn release_lane(
    sender: &Sender<Op>,
    lane: &crate::lane::LaneId,
    endpoint: Option<ForwardEndpoint>,
    forwards: &[ForwardSpec],
) {
    if let Some(target) = master_target(lane) {
        let _ = sender.send(Op::StopHost { target });
        return;
    }
    let Some(endpoint) = endpoint else {
        return;
    };
    for spec in forwards {
        let _ = sender.send(Op::CancelForward {
            endpoint: endpoint.clone(),
            spec: spec.clone(),
        });
    }
}

pub(crate) fn add_for_lane(sender: &Sender<Op>, endpoint: ForwardEndpoint, spec: ForwardSpec) {
    let _ = sender.send(Op::AddForward { endpoint, spec });
}

pub(crate) fn cancel_for_lane(sender: &Sender<Op>, endpoint: ForwardEndpoint, spec: ForwardSpec) {
    let _ = sender.send(Op::CancelForward { endpoint, spec });
}

#[cfg(test)]
#[path = "../../../tests/unit/app/port_forward_task.rs"]
mod tests;

//! App-level SSH/remote orchestration: the stateful machinery that drives
//! deck's remote hosts, distinct from the stateless backend in
//! `infra::ssh`.
//!
//! - [`remote_conn`]: the remote-connection state machine
//!   (`RemoteConnManager`) owning one long-lived `ssh -tt host tmux attach`
//!   PTY per host.
//! - [`remote_spawn`]: the async spawner that brings those PTYs up off the
//!   app loop's thread.
//! - [`port_forward_task`]: the port-forward worker owning SSH
//!   ControlMaster process lifecycle.

pub mod port_forward_task;
pub(super) mod remote_conn;
mod remote_spawn;

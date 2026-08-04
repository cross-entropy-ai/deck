//! App-level SSH/remote orchestration: the stateful machinery driving deck's
//! remote hosts, distinct from the stateless `infra::ssh` backend.
//!
//! - [`remote_conn`]: remote-connection state machine (`RemoteConnManager`),
//!   owning one long-lived `ssh -tt host tmux attach` PTY per host.
//! - [`remote_spawn`]: async spawner bringing those PTYs up off the app loop.
//! - [`port_forward_task`]: port-forward worker owning SSH ControlMaster
//!   process lifecycle.

pub(crate) mod config_adapter;
pub mod port_forward_task;
pub(super) mod remote_conn;
mod remote_spawn;

//! The tmux backends: `local` talks to this machine's tmux server, `remote`
//! drives a host's server over ssh. Both produce the same shapes (D4) — the
//! split lives only in how the inputs are gathered.

pub mod local;
pub mod remote;

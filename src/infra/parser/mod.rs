//! Pure parsers for backend command output — text in, structs out, no I/O.
//! Shared by the local and remote backends (CLAUDE.md: push the local/remote
//! split as low as it goes); shelling-out, quoting, and error semantics stay
//! in each owning module (`infra::tmux`, `infra::remote_tmux`, `infra::ssh`).
pub mod dir;
pub mod listeners;
pub mod pane;
pub mod release;
pub mod ssh_config;
pub mod tmux;

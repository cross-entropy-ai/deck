//! SSH and remote-forward backend: `ssh` client helpers ([`client`]),
//! port-forward command builders ([`port_forward`]), the ssh-agent relay that
//! reaches inside a container ([`agent_relay`]), and SSH-specific model types
//! ([`model`]).
//!
//! `client`'s public API is re-exported here so Deck's connection options,
//! `crate::ssh::config_hosts`, etc. resolve directly off the module.

pub mod agent_relay;
pub mod client;
pub mod divider;
pub mod model;
pub mod port_forward;

pub use client::*;

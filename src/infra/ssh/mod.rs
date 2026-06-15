//! SSH and remote-forward backend: the `ssh` client helpers ([`client`]),
//! the port-forward command builders ([`port_forward`]), local listener
//! enumeration for forward liveness ([`listeners`]), and the SSH-specific
//! model types ([`model`]).
//!
//! `client`'s public API is re-exported here so `crate::ssh::CONTROL_OPTS`,
//! `crate::ssh::config_hosts`, etc. resolve directly off the module.

pub mod client;
pub mod listeners;
pub mod model;
pub mod port_forward;

pub use client::*;

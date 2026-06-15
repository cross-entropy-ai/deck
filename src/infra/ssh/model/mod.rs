//! SSH-specific model types: the per-host key newtype ([`host_key`]), the
//! port-forward liveness/badge/form state ([`forwards`]), and the "Add
//! Remote Host" picker state ([`add_remote`]).

pub mod add_remote;
pub mod forwards;
pub mod host_key;

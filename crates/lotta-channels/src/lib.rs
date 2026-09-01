//! Messaging-channel protocol and control-client adapters.
//!
//! This crate implements channel ports. It must not depend on transport adapters or unrelated
//! concrete adapters.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Bounded newline-delimited management protocol and runtime-tool registry.
pub mod control_plane;
/// Supervised child-process lifecycle.
pub mod supervisor;
/// Channel store, capability, sandbox, and process topology.
pub mod topology;

/// The compatibility child implementation run by the composition binary.
pub mod host;

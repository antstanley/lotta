//! Skills, hooks, mods, MCP, and subagent adapters.
//!
//! This crate implements focused extension behavior. Skill discovery uses explicit roots and
//! security-sensitive skill scripts reuse the shared tool policy and sandbox ports.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::struct_field_names
)]

/// Shared bounded local-pipe contract for extension sidecars.
pub mod sidecar;
/// Skill discovery, loading, selection, and script execution policy.
pub mod skills;

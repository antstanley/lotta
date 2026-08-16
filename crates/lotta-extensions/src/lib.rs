//! Skills, hooks, mods, MCP, and subagent adapters.
//!
//! This crate implements focused extension behavior. Skill discovery uses explicit roots and
//! security-sensitive skill scripts reuse the shared tool policy and sandbox ports.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Skill discovery, loading, selection, and script execution policy.
pub mod skills;

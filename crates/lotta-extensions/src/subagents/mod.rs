//! Pinned, bounded, and confined Letta Code subagent compatibility adapter.

/// Canonical filesystem and history capability confinement.
pub mod confinement;
/// Bounded parent status snapshots and stream events.
pub mod snapshot;
/// Pinned process launcher and parent-scoped task manager.
pub mod spawn;
/// Strict request types and context plans.
pub mod types;

#[cfg(test)]
mod bounds_certificate;
#[cfg(test)]
mod confinement_certificate;
#[cfg(test)]
mod process_certificate;
#[cfg(test)]
mod status_certificate;
#[cfg(test)]
mod types_certificate;

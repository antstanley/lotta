//! Bounded external TypeScript mod compatibility host.
//!
//! The subsystem uses Task 42 framing and supervision, Task 32 atomic tool publication, and Task
//! 44 lifecycle discriminants. Children receive only declared capabilities and one opaque scoped
//! conversation handle.

/// Declared capability broker and dependency-neutral runtime port.
pub mod capabilities;
/// Deterministic host lifecycle controller.
pub mod controller;
/// Task 42 framed JSON-RPC host transport.
pub mod host;
/// Shell-free production process launcher and packaged bridge.
pub mod launcher;
/// Strict versioned JSON-RPC wire contract.
pub mod protocol;
/// Six exact typed registration kinds.
pub mod registrations;
/// Atomic six-store and Task 32 publication adapters.
pub mod registry;
/// Safe-mode and no-mods startup policy.
pub mod safe_mode;
/// Shared identifiers, owners, generations, scopes, and errors.
pub mod types;

#[cfg(test)]
mod cancellation_certificate;
#[cfg(test)]
mod capabilities_certificate;
#[cfg(test)]
mod controller_certificate;
#[cfg(test)]
mod lifecycle_certificate;
#[cfg(test)]
mod no_mods_flag_certificate;
#[cfg(test)]
mod real_host_certificate;
#[cfg(test)]
mod registrations_certificate;
#[cfg(test)]
mod runtime_snapshot_certificate;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod transaction_certificate;
#[cfg(test)]
mod wire_certificate;

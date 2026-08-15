//! Application orchestration for queues, turns, approvals, and compaction.
//!
//! This crate owns application workflows and their effect interfaces. It depends inward on
//! `lotta-domain` plus minimal concurrency support. Persistence, `MemFS`, provider, tool, channel,
//! extension, and transport adapters depend on these runtime interfaces; the runtime never
//! depends on those concrete adapters.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Validated, bounded values shared by runtime ports.
pub mod boundary;
/// Immutable Task 06 runtime resource bounds exported for inward-dependent adapters.
pub mod bounds;
/// Lease-guarded post-await effects.
pub mod lease;
/// Runtime lifecycle ownership and projections.
pub mod lifecycle;
/// Effect interfaces implemented by adapters outside this crate.
pub mod ports;
/// Bounded listener-owned runtime registry.
pub mod registry;
/// Explicit task-local per-turn context.
pub mod scope_context;
/// Worktree watcher lifetime state.
pub mod worktree_watcher;

pub use lease::{CancellationPolicy, LeaseEffect, LeaseGuard, SuppressionReason};
pub use lifecycle::{LifecycleOwner, LifecycleProjection};
pub use registry::{ListenerRuntime, ResidencyUpdate, RuntimeHandle, RuntimeKey, RuntimeResidency};
pub use scope_context::{
    ScopeContextInput, ScopeContextSnapshot, WorkspaceSandbox, scope_operation, spawn_scoped,
    try_current,
};
pub use worktree_watcher::{WORKTREE_WATCHER_IDLE_STOP_MS, WorktreeWatcher};

/// Stable runtime-boundary failure shared by all core ports.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RuntimeError {
    /// A requested resource does not exist.
    #[error("not_found: {context}")]
    NotFound {
        /// Stable resource or operation context supplied by the adapter.
        context: String,
    },
    /// A durable resource changed concurrently.
    #[error("conflict: {context}")]
    Conflict {
        /// Stable resource or operation context supplied by the adapter.
        context: String,
    },
    /// Input or stored data violates the port contract.
    #[error("invalid_data: {context}")]
    InvalidData {
        /// Stable validation context without concrete parser details.
        context: String,
    },
    /// A named resource ceiling rejected the operation.
    #[error("limit_exceeded: {context}")]
    LimitExceeded {
        /// Stable limit or operation context supplied by the adapter.
        context: String,
    },
    /// The operation was cancelled through its explicit token or dropped future.
    #[error("cancelled: {context}")]
    Cancelled {
        /// Stable operation context supplied by the adapter.
        context: String,
    },
    /// The operation exceeded its configured deadline.
    #[error("timeout: {context}")]
    Timeout {
        /// Stable operation context supplied by the adapter.
        context: String,
    },
    /// The operating system denied the requested effect.
    #[error("permission_denied: {context}")]
    PermissionDenied {
        /// Stable resource or operation context supplied by the adapter.
        context: String,
    },
    /// A required adapter capability is unavailable on this platform or configuration.
    #[error("unsupported: {context}")]
    Unsupported {
        /// Stable capability context supplied by the adapter.
        context: String,
    },
    /// A stable adapter operation failed after concrete errors were translated.
    #[error("adapter_failure[{code}]: {context}")]
    AdapterFailure {
        /// Adapter-defined stable machine-readable code that survives vendor refactors.
        code: &'static str,
        /// Scrubbed resource or operation context without a concrete error value.
        context: String,
    },
}

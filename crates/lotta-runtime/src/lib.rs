//! Application orchestration for queues, turns, approvals, and compaction.
//!
//! This crate owns application workflows and their effect interfaces. It depends inward on
//! `lotta-domain` plus minimal concurrency support. Persistence, `MemFS`, provider, tool, channel,
//! extension, and transport adapters depend on these runtime interfaces; the runtime never
//! depends on those concrete adapters.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Serialized per-runtime input admission.
pub mod admission;
/// Durable approval ownership and recovery.
pub mod approval;
/// Validated, bounded values shared by runtime ports.
pub mod boundary;
/// Immutable Task 06 runtime resource bounds exported for inward-dependent adapters.
pub mod bounds;
/// Transcript compaction planning and service orchestration.
pub mod compaction;
/// Dependency-neutral hook wire types and firing capability.
pub mod hooks;
/// Lease-guarded post-await effects.
pub mod lease;
/// Runtime lifecycle ownership and projections.
pub mod lifecycle;
/// Dependency-neutral model precedence selection.
pub mod model;
/// Runtime-scoped structured events and bounded metrics.
pub mod observe;
/// Effect interfaces implemented by adapters outside this crate.
pub mod ports;
/// Bounded per-conversation FIFO queue.
pub mod queue;
/// Authoritative queue mutation snapshots.
pub mod queue_snapshot;
/// Bounded listener-owned runtime registry.
pub mod registry;
/// Runtime-owned deterministic provider retry and fallback.
pub mod retry;
/// Schedule persistence-neutral timing and lifecycle logic.
pub mod schedule;
/// Explicit task-local per-turn context.
pub mod scope_context;
/// Lease-guarded provider and local-tool turn loop.
pub mod turn;
/// Worktree watcher lifetime state.
pub mod worktree_watcher;

pub use admission::{AdmissionOutcome, AdmissionRequest, AdmissionRoute, admit_control_snapshot};
pub use approval::{
    ApprovalJournal, ApprovalManager, ApprovalRecovery, ApprovalRequest, ApprovalResolution,
    ApprovalResolutionInput, ApprovalState, EditedInputValidator,
    PENDING_APPROVALS_PER_RUNTIME_MAX, RecoveryAction,
};
pub use bounds::APPROVAL_WAIT_MS_MAX;
pub use compaction::{
    COMPACTION_RECENT_PERCENT_DEFAULT, COMPACTION_RECENT_PERCENT_MAX, CompactionCommand,
    CompactionEffects, CompactionMode, CompactionPlan, CompactionRecovery, CompactionService,
    CompactionSummarizer, CompactionSummary, CompactionTrigger,
};
pub use lease::{
    CancellationClaim, CancellationPolicy, CancellationReceipt, LeaseEffect, LeaseGuard,
    SuppressionReason,
};
pub use lifecycle::{LifecycleOwner, LifecycleProjection};
pub use queue::{ConversationQueue, PumpDirective, PumpMutation};
pub use queue_snapshot::{QueueMutation, QueueMutationEvent, QueueSnapshot};
pub use registry::{ListenerRuntime, ResidencyUpdate, RuntimeHandle, RuntimeKey, RuntimeResidency};
pub use retry::{RetryExecutor, RetryTerminal};
pub use scope_context::{
    ScopeContextInput, ScopeContextSnapshot, WorkspaceSandbox, scope_operation, spawn_scoped,
    try_current,
};
pub use turn::{
    CompactionPort, CompactionProgress, ConfiguredFallback, ProjectionKind,
    ProviderTurnExecutorPort, RequestRefreshPort, ToolResultRecord, ToolSnapshotHandle,
    TurnEffectPort, TurnEvent, TurnPorts, TurnProjection, TurnProvider, TurnRunOutcome,
    TurnToolCatalog, run_turn,
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
    /// Provider context capacity was exceeded with safe structured detail.
    #[error("context_overflow")]
    ContextOverflow {
        /// Serializable non-secret overflow detail.
        detail: ports::ProviderContextOverflowDetail,
    },
    /// Required transcript compaction service is not registered.
    #[error("compaction_unavailable")]
    CompactionUnavailable,
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

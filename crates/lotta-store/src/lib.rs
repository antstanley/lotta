//! Persistence adapters for compatible JSON and JSONL storage.
//!
//! This crate implements persistence ports below the runtime layer. Its atomic replacement lock
//! serializes participating Lotta writers only; external and TypeScript writers are detected by
//! revision comparison and concurrent mixed-runtime writes remain unsupported.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod adapter;
mod agent;
/// Atomic bounded runtime approval journal.
pub mod approval;
/// Durable bounded atomic file replacement.
pub mod atomic;
/// Durable process-safe compaction transaction journal.
pub mod compaction;
mod confinement;
mod conversation;
mod error;
mod lock;
/// Explicit bounded transcript migration.
pub mod migration;
/// Baseline-compatible local-backend paths and encoded keys.
pub mod paths;
/// Durable bounded post-turn job queue.
pub mod post_turn;
/// Bounded canonical JSON/JSONL query API.
pub mod query;
mod refresh;
/// Opaque bounded baseline side-store persistence.
pub mod side;
/// Bounded append-only transcript persistence.
pub mod transcript;
/// Non-mutating strict transcript diagnostics.
pub mod verify;

pub use adapter::{LOCAL_STORE_BLOCKING_MAX, LocalStore};
pub use agent::AGENTS_MAX;
pub use atomic::{
    ATOMIC_WRITE_BYTES_MAX, ATOMIC_WRITE_RETRIES_MAX, AtomicObserver, WriteMode, atomic_delete,
    atomic_delete_observed, atomic_write, atomic_write_observed,
};
pub use compaction::{
    COMPACTION_JOURNAL_BYTES_MAX, COMPACTION_PROJECTION_BYTES_MAX, COMPACTION_TRANSACTIONS_MAX,
    CompactionClaim, CompactionProjection, CompactionTransaction, CompactionTransactionState,
};
pub use conversation::CONVERSATIONS_PER_AGENT_MAX;
pub use error::{StoreError, StoreErrorKind};
pub use lock::{LOTTA_STORAGE_LOCK_WAIT_MS, LottaStorageLock};
pub use paths::{ConversationKey, LETTA_LOCAL_BACKEND_DIR, StorePaths};
pub use post_turn::{
    MemoryPushJob, POST_TURN_COMPLETED_JOBS_MAX, POST_TURN_JOB_ATTEMPTS_MAX,
    POST_TURN_JOBS_BYTES_MAX, POST_TURN_JOBS_MAX, PostTurnClaim, PostTurnExecution, PostTurnJob,
    PostTurnJobKey, PostTurnJobKind, PostTurnJobRunner, PostTurnJobState, PostTurnQueue,
    ReflectionJob,
};
pub use side::{ChannelFile, OpaqueFile, ProjectFile, SidePaths, SideRevision};
pub use transcript::{TRANSCRIPT_BYTES_MAX, TRANSCRIPT_LINE_BYTES_MAX};

#[cfg(test)]
mod compaction_tests;
#[cfg(test)]
mod post_turn_tests;
#[cfg(test)]
mod tests;

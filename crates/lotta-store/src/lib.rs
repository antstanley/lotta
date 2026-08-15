//! Persistence adapters for compatible JSON and JSONL storage.
//!
//! This crate implements persistence ports below the runtime layer. Its atomic replacement lock
//! serializes participating Lotta writers only; external and TypeScript writers are detected by
//! revision comparison and concurrent mixed-runtime writes remain unsupported.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod adapter;
/// Durable bounded atomic file replacement.
pub mod atomic;
mod confinement;
mod error;
mod lock;
/// Baseline-compatible local-backend paths and encoded keys.
pub mod paths;

pub use adapter::{LOCAL_STORE_BLOCKING_MAX, LocalStore};
pub use atomic::{
    ATOMIC_WRITE_BYTES_MAX, ATOMIC_WRITE_RETRIES_MAX, AtomicObserver, WriteMode, atomic_delete,
    atomic_delete_observed, atomic_write, atomic_write_observed,
};
pub use error::{StoreError, StoreErrorKind};
pub use lock::{LOTTA_STORAGE_LOCK_WAIT_MS, LottaStorageLock};
pub use paths::{ConversationKey, LETTA_LOCAL_BACKEND_DIR, StorePaths};

#[cfg(test)]
mod tests;

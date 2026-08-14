//! Application orchestration for queues, turns, approvals, and compaction.
//!
//! This crate owns application workflows and port traits. It must not depend on concrete
//! adapters or transport implementations.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

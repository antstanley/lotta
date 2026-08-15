//! Native-Git memory filesystem adapter with confined durable file operations.
//!
//! This crate depends only on domain/runtime boundaries and Tokio. Each agent owns one repository
//! at `<backend-root>/memfs/<agent-id>/memory/`; no remote is inferred or configured.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod fs;
mod git;
mod labels;
mod ops;
mod repo;
mod validation;
mod worktree;

pub use labels::{normalize_label, render_block};
pub use lotta_runtime::bounds::{MEMORY_FILE_BYTES_MAX, MEMORY_FILES_MAX};
pub use ops::GitMemFs;

#[cfg(test)]
mod tests;

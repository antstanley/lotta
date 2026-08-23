//! Pinned-agent side store (`${LETTA_HOME}/pinned-agents.json`).
//!
//! One minimal document recording which agent identifiers the client pinned
//! globally, mirroring the pinned baseline's `settingsManager.pinAgent`
//! effect. The document shape is `{"agents": ["<agent-id>", ...]}` with
//! identifiers sorted and deduplicated; everything else about the file stays
//! opaque to this crate.

use super::{OpaqueFile, SidePaths};
use crate::StoreError;

/// Reads `pinned-agents.json` as exact opaque bytes.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn read(paths: &SidePaths) -> Result<OpaqueFile, StoreError> {
    super::io::read(&paths.pinned_agents()?)
}

/// Atomically replaces `pinned-agents.json` with exact opaque bytes.
///
/// # Errors
/// Returns a typed path, limit, conflict, lock, or filesystem failure.
pub fn write(paths: &SidePaths, bytes: &[u8]) -> Result<(), StoreError> {
    super::io::write(&paths.pinned_agents()?, bytes)
}

/// Atomically replaces the document only when the read revision remains
/// current.
///
/// # Errors
/// Returns `StorageConflict` when the source changed, otherwise a typed
/// storage failure.
pub fn write_expected(
    paths: &SidePaths,
    source: &OpaqueFile,
    bytes: &[u8],
) -> Result<(), StoreError> {
    super::io::write_expected(&paths.pinned_agents()?, bytes, source.revision())
}

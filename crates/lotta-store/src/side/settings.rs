use super::{OpaqueFile, SidePaths};
use crate::StoreError;
#[cfg(test)]
use crate::atomic::AtomicObserver;

/// Reads global settings as exact opaque bytes.
///
/// # Errors
/// Returns typed path, missing, limit, conflict, or filesystem failures.
pub fn read(paths: &SidePaths) -> Result<OpaqueFile, StoreError> {
    super::io::read(&paths.settings()?)
}

/// Atomically replaces global settings with exact opaque bytes.
///
/// # Errors
/// Returns typed path, limit, conflict, lock, or filesystem failures.
pub fn write(paths: &SidePaths, bytes: &[u8]) -> Result<(), StoreError> {
    super::io::write(&paths.settings()?, bytes)
}

/// Atomically replaces global settings only when the read revision remains current.
///
/// # Errors
/// Returns `StorageConflict` when the source changed, otherwise a typed storage failure.
pub fn write_expected(
    paths: &SidePaths,
    source: &OpaqueFile,
    bytes: &[u8],
) -> Result<(), StoreError> {
    super::io::write_expected(&paths.settings()?, bytes, source.revision())
}

#[cfg(test)]
pub(crate) fn write_observed(
    paths: &SidePaths,
    bytes: &[u8],
    observer: &dyn AtomicObserver,
) -> Result<(), StoreError> {
    super::io::write_observed(&paths.settings()?, bytes, observer)
}

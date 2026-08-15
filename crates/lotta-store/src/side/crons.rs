use super::{OpaqueFile, SidePaths};
use crate::StoreError;
#[cfg(test)]
use crate::atomic::AtomicObserver;

/// Reads `crons.json` as exact opaque bytes.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn read(paths: &SidePaths) -> Result<OpaqueFile, StoreError> {
    super::io::read(&paths.crons()?)
}

/// Atomically replaces `crons.json` with exact opaque bytes.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn write(paths: &SidePaths, bytes: &[u8]) -> Result<(), StoreError> {
    super::io::write(&paths.crons()?, bytes)
}

/// Reads one run log as exact opaque bytes.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn read_run(paths: &SidePaths, schedule_id: &str) -> Result<OpaqueFile, StoreError> {
    super::io::read(&paths.run_log(schedule_id)?)
}

/// Atomically replaces one complete run log; append semantics belong to Task 60.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn write_run(paths: &SidePaths, schedule_id: &str, bytes: &[u8]) -> Result<(), StoreError> {
    super::io::write(&paths.run_log(schedule_id)?, bytes)
}

#[cfg(test)]
pub(crate) fn write_observed(
    paths: &SidePaths,
    bytes: &[u8],
    observer: &dyn AtomicObserver,
) -> Result<(), StoreError> {
    super::io::write_observed(&paths.crons()?, bytes, observer)
}

#[cfg(test)]
pub(crate) fn write_run_observed(
    paths: &SidePaths,
    schedule_id: &str,
    bytes: &[u8],
    observer: &dyn AtomicObserver,
) -> Result<(), StoreError> {
    super::io::write_observed(&paths.run_log(schedule_id)?, bytes, observer)
}

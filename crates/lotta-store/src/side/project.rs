#[cfg(test)]
use super::io::SideReadObserver;
use super::{OpaqueFile, ProjectFile, SidePaths};
use crate::StoreError;
#[cfg(test)]
use crate::atomic::AtomicObserver;
use std::path::Path;

/// Reads one exact project settings file after the pure scope gate succeeds.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn read(
    paths: &SidePaths,
    workspace: &Path,
    file: ProjectFile,
) -> Result<OpaqueFile, StoreError> {
    super::io::read(&paths.project_file(workspace, file)?)
}

/// Atomically replaces one exact project settings file after scope gating.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn write(
    paths: &SidePaths,
    workspace: &Path,
    file: ProjectFile,
    bytes: &[u8],
) -> Result<(), StoreError> {
    super::io::write(&paths.project_file(workspace, file)?, bytes)
}

#[cfg(test)]
pub(crate) fn read_observed(
    paths: &SidePaths,
    workspace: &Path,
    file: ProjectFile,
    observer: &dyn SideReadObserver,
) -> Result<OpaqueFile, StoreError> {
    let path = paths.project_file(workspace, file)?;
    super::io::read_observed(&path, observer)
}

#[cfg(test)]
pub(crate) fn write_observed(
    paths: &SidePaths,
    workspace: &Path,
    file: ProjectFile,
    bytes: &[u8],
    observer: &dyn AtomicObserver,
) -> Result<(), StoreError> {
    super::io::write_observed(&paths.project_file(workspace, file)?, bytes, observer)
}

#[cfg(test)]
#[test]
fn scope_gate() {
    crate::side::evidence::project::scope_gate();
}

use crate::atomic::{
    AtomicObserver, FileRevision, WriteMode, atomic_write_observed,
    atomic_write_with_expected_observed,
};
use crate::confinement::{backend_root, validate_existing, validate_regular_file};
use crate::{StoreError, StoreErrorKind};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Maximum opaque bytes in one side-store file.
pub const SIDE_FILE_BYTES_MAX: usize = crate::ATOMIC_WRITE_BYTES_MAX;
const READ_BUFFER_BYTES: usize = 64 * 1_024;

/// Opaque side-store contents coupled to the revision that proved the read.
#[derive(Clone, Debug)]
pub struct OpaqueFile {
    bytes: Vec<u8>,
    revision: SideRevision,
}

impl OpaqueFile {
    /// Borrows exact persisted bytes, including any final line feed.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Borrows the expected revision for conflict-aware replacement.
    #[must_use]
    pub const fn revision(&self) -> &SideRevision {
        &self.revision
    }
}

/// Opaque revision token accepted only by side-store expected writes.
#[derive(Clone, Debug)]
pub struct SideRevision(FileRevision);

pub(crate) fn read(path: &Path) -> Result<OpaqueFile, StoreError> {
    read_inner(path, &SideReadNoopObserver)
}

#[cfg(test)]
pub(crate) fn read_observed(
    path: &Path,
    observer: &dyn SideReadObserver,
) -> Result<OpaqueFile, StoreError> {
    read_inner(path, observer)
}

fn read_inner(path: &Path, observer: &dyn SideReadObserver) -> Result<OpaqueFile, StoreError> {
    observer.before_access(path)?;
    let root = backend_root(path)?;
    validate_existing(root, path)?;
    validate_regular_file(root, path)?;
    let before = FileRevision::sample_path_bounded(path, SIDE_FILE_BYTES_MAX as u64)?;
    if !before.exists() {
        return Err(StoreError::new(StoreErrorKind::NotFound, path));
    }
    observer.after_first_revision(path)?;
    let bytes = read_exact_bounded(root, path, before.length())?;
    let after = FileRevision::sample_path_bounded(path, SIDE_FILE_BYTES_MAX as u64)?;
    if before != after || !revision_matches(&before, &bytes)? {
        return Err(StoreError::new(StoreErrorKind::StorageConflict, path));
    }
    Ok(OpaqueFile {
        bytes,
        revision: SideRevision(before),
    })
}

pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    validate_payload(path, bytes)?;
    atomic_write_observed(path, bytes, WriteMode::Standard, &SideNoopObserver)
}

#[cfg(test)]
pub(crate) fn write_observed(
    path: &Path,
    bytes: &[u8],
    observer: &dyn AtomicObserver,
) -> Result<(), StoreError> {
    validate_payload(path, bytes)?;
    atomic_write_observed(path, bytes, WriteMode::Standard, observer)
}

pub(crate) fn write_expected(
    path: &Path,
    bytes: &[u8],
    expected: &SideRevision,
) -> Result<(), StoreError> {
    validate_payload(path, bytes)?;
    atomic_write_with_expected_observed(
        path,
        bytes,
        WriteMode::Standard,
        &expected.0,
        &SideNoopObserver,
    )
}

struct SideNoopObserver;
impl AtomicObserver for SideNoopObserver {}

pub(crate) trait SideReadObserver {
    fn before_access(&self, _path: &Path) -> Result<(), StoreError> {
        Ok(())
    }

    fn after_first_revision(&self, _path: &Path) -> Result<(), StoreError> {
        Ok(())
    }
}

struct SideReadNoopObserver;
impl SideReadObserver for SideReadNoopObserver {}

fn read_exact_bounded(root: &Path, path: &Path, length: u64) -> Result<Vec<u8>, StoreError> {
    let capacity = usize::try_from(length).map_err(|_| limit(path))?;
    if capacity > SIDE_FILE_BYTES_MAX {
        return Err(limit(path));
    }
    let mut file = File::open(path).map_err(|error| StoreError::from_io(path, &error))?;
    validate_regular_file(root, path)?;
    let metadata = file
        .metadata()
        .map_err(|error| StoreError::from_io(path, &error))?;
    if metadata.len() != length {
        return Err(conflict(path));
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|_| limit(path))?;
    let mut chunk = Vec::new();
    chunk
        .try_reserve_exact(READ_BUFFER_BYTES)
        .map_err(|_| limit(path))?;
    chunk.resize(READ_BUFFER_BYTES, 0);
    loop {
        let count = file
            .read(&mut chunk)
            .map_err(|error| StoreError::from_io(path, &error))?;
        if count == 0 {
            break;
        }
        let total = bytes.len().checked_add(count).ok_or_else(|| limit(path))?;
        if total > capacity {
            return Err(conflict(path));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    if bytes.len() != capacity {
        return Err(conflict(path));
    }
    Ok(bytes)
}

fn revision_matches(revision: &FileRevision, bytes: &[u8]) -> Result<bool, StoreError> {
    let length = u64::try_from(bytes.len()).map_err(|_| limit("side-file"))?;
    let checksum: [u8; 32] = Sha256::digest(bytes).into();
    Ok(revision.same_contents(&FileRevision::from_contents(length, checksum)))
}

fn validate_payload(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    if bytes.len() > SIDE_FILE_BYTES_MAX {
        return Err(limit(path));
    }
    Ok(())
}

fn limit(path: impl Into<std::path::PathBuf>) -> StoreError {
    StoreError::new(StoreErrorKind::Limit, path)
}

fn conflict(path: &Path) -> StoreError {
    StoreError::new(StoreErrorKind::StorageConflict, path)
}

use crate::confinement::{
    backend_root, create_confined_parent, validate_existing, validate_regular_file,
};
use crate::{LottaStorageLock, StoreError, StoreErrorKind};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Exact maximum number of atomic write attempts.
pub const ATOMIC_WRITE_RETRIES_MAX: usize = 3;
/// Maximum payload accepted by the shared atomic record primitive.
pub const ATOMIC_WRITE_BYTES_MAX: usize = 8 * 1_024 * 1_024;
const TEMP_CREATE_RETRIES_MAX: usize = 64;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Permission policy applied to an atomic replacement target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteMode {
    /// Ordinary record using platform defaults.
    Standard,
    /// Provider authentication file and parent secure modes.
    ProviderAuth,
}

/// Narrow observation and deterministic failure seam at actual operation boundaries.
pub trait AtomicObserver: Send + Sync {
    /// Reads the source modification time at each real revision sample.
    ///
    /// # Errors
    /// Returns a typed filesystem failure or may inject one for deterministic tests.
    fn source_modified(
        &self,
        target: &Path,
        metadata: &std::fs::Metadata,
    ) -> Result<SystemTime, StoreError> {
        metadata
            .modified()
            .map_err(|error| StoreError::from_io(target, &error))
    }
    /// Called at each pre-commit attempt boundary.
    ///
    /// # Errors
    /// May inject a scrubbed typed failure.
    fn attempt(&self, _attempt: usize, _target: &Path) -> Result<(), StoreError> {
        Ok(())
    }
    /// Called immediately before the final revision comparison and rename.
    ///
    /// # Errors
    /// May inject a scrubbed typed failure.
    fn before_rename(&self, _target: &Path) -> Result<(), StoreError> {
        Ok(())
    }
    /// Called after rename, immediately before the real parent `sync_all`.
    ///
    /// # Errors
    /// May inject a scrubbed typed durability failure.
    fn before_parent_sync(&self, _parent: &Path) -> Result<(), StoreError> {
        Ok(())
    }
    /// Called after the production directory file handle is successfully flushed.
    fn parent_flushed(&self, _parent: &Path) {}
    /// Called immediately before delete revision comparison and removal.
    ///
    /// # Errors
    /// May inject a scrubbed typed failure.
    fn before_remove(&self, _target: &Path) -> Result<(), StoreError> {
        Ok(())
    }
}

struct NoopObserver;
impl AtomicObserver for NoopObserver {}

/// Atomically replaces one bounded file while holding the participating Lotta writer lock.
///
/// # Errors
/// Returns typed path, lock, conflict, limit, or filesystem failures.
pub fn atomic_write(path: &Path, bytes: &[u8], mode: WriteMode) -> Result<(), StoreError> {
    logged_once(|| atomic_write_inner(path, bytes, mode, &NoopObserver))
}

/// Atomically replaces one file with an observer at actual production step boundaries.
///
/// # Errors
/// Returns typed operation or injected observer failures.
pub fn atomic_write_observed(
    path: &Path,
    bytes: &[u8],
    mode: WriteMode,
    observer: &dyn AtomicObserver,
) -> Result<(), StoreError> {
    logged_once(|| atomic_write_inner(path, bytes, mode, observer))
}

fn atomic_write_inner(
    path: &Path,
    bytes: &[u8],
    mode: WriteMode,
    observer: &dyn AtomicObserver,
) -> Result<(), StoreError> {
    validate_target(path, bytes)?;
    let parent = path.parent().ok_or_else(|| invalid_path(path))?;
    let root = backend_root(path)?;
    create_confined_parent(root, parent)?;
    apply_directory_mode(parent, mode)?;
    validate_existing(root, path)?;
    let _lock = LottaStorageLock::try_acquire_confined(root)?;
    let source = Revision::sample(root, path, observer)?;
    let mut last = None;
    for attempt in 1..=ATOMIC_WRITE_RETRIES_MAX {
        if let Err(error) = observer.attempt(attempt, path) {
            if retryable(error.kind()) {
                last = Some(error);
                continue;
            }
            return Err(error);
        }
        match write_precommit(root, path, bytes, mode, &source, observer) {
            Ok(parent) => return flush_parent(&parent, observer),
            Err(error) if retryable(error.kind()) => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or_else(|| StoreError::new(StoreErrorKind::Io, path)))
}

fn write_precommit(
    root: &Path,
    path: &Path,
    bytes: &[u8],
    mode: WriteMode,
    source: &Revision,
    observer: &dyn AtomicObserver,
) -> Result<PathBuf, StoreError> {
    let parent = path.parent().ok_or_else(|| invalid_path(path))?;
    validate_existing(root, parent)?;
    let (temp_path, mut temp) = create_temp(root, parent, path)?;
    let result = (|| {
        temp.write_all(bytes)
            .map_err(|error| StoreError::from_io(&temp_path, &error))?;
        apply_file_mode(&temp, path, mode)?;
        temp.sync_all()
            .map_err(|error| StoreError::from_io(&temp_path, &error))?;
        observer.before_rename(path)?;
        ensure_revision(root, path, source, observer)?;
        validate_existing(root, parent)?;
        validate_existing(root, path)?;
        std::fs::rename(&temp_path, path).map_err(|error| StoreError::from_io(path, &error))?;
        Ok(parent.to_path_buf())
    })();
    if result.is_err() {
        let _cleanup = std::fs::remove_file(&temp_path);
    }
    result
}

/// Removes one file with lock, conflict comparison, and post-commit parent durability.
///
/// # Errors
/// Returns typed path, lock, conflict, or filesystem failures.
pub fn atomic_delete(path: &Path) -> Result<(), StoreError> {
    atomic_delete_observed(path, &NoopObserver)
}

/// Removes one file with an observer at actual delete boundaries.
///
/// # Errors
/// Returns typed operation or injected observer failures.
pub fn atomic_delete_observed(
    path: &Path,
    observer: &dyn AtomicObserver,
) -> Result<(), StoreError> {
    logged_once(|| atomic_delete_inner(path, observer))
}

fn atomic_delete_inner(path: &Path, observer: &dyn AtomicObserver) -> Result<(), StoreError> {
    validate_target(path, &[])?;
    let root = backend_root(path)?;
    validate_regular_file(root, path)?;
    let _lock = LottaStorageLock::try_acquire_confined(root)?;
    let source = Revision::sample(root, path, observer)?;
    observer.before_remove(path)?;
    ensure_revision(root, path, &source, observer)?;
    validate_regular_file(root, path)?;
    std::fs::remove_file(path).map_err(|error| StoreError::from_io(path, &error))?;
    let parent = path.parent().ok_or_else(|| invalid_path(path))?;
    flush_parent(parent, observer)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Revision {
    exists: bool,
    modified: Option<SystemTime>,
    length: u64,
    checksum: [u8; 32],
}

impl Revision {
    fn sample(root: &Path, path: &Path, observer: &dyn AtomicObserver) -> Result<Self, StoreError> {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::absent()),
            Err(error) => Err(StoreError::from_io(path, &error)),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                Err(invalid_path(path))
            }
            Ok(metadata) => {
                validate_regular_file(root, path)?;
                if metadata.len() > ATOMIC_WRITE_BYTES_MAX as u64 {
                    return Err(StoreError::new(StoreErrorKind::Limit, path));
                }
                let bytes = bounded_read(path, metadata.len())?;
                Ok(Self {
                    exists: true,
                    modified: Some(observer.source_modified(path, &metadata)?),
                    length: metadata.len(),
                    checksum: Sha256::digest(bytes).into(),
                })
            }
        }
    }

    const fn absent() -> Self {
        Self {
            exists: false,
            modified: None,
            length: 0,
            checksum: [0; 32],
        }
    }
}

fn ensure_revision(
    root: &Path,
    path: &Path,
    expected: &Revision,
    observer: &dyn AtomicObserver,
) -> Result<(), StoreError> {
    if &Revision::sample(root, path, observer)? == expected {
        Ok(())
    } else {
        Err(StoreError::new(StoreErrorKind::StorageConflict, path))
    }
}

fn bounded_read(path: &Path, length: u64) -> Result<Vec<u8>, StoreError> {
    let length =
        usize::try_from(length).map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    let mut file = File::open(path).map_err(|error| StoreError::from_io(path, &error))?;
    std::io::Read::take(&mut file, ATOMIC_WRITE_BYTES_MAX as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| StoreError::from_io(path, &error))?;
    if bytes.len() != length {
        return Err(StoreError::new(StoreErrorKind::StorageConflict, path));
    }
    Ok(bytes)
}

fn create_temp(root: &Path, parent: &Path, target: &Path) -> Result<(PathBuf, File), StoreError> {
    for _ in 0..TEMP_CREATE_RETRIES_MAX {
        validate_existing(root, parent)?;
        let sequence = next_sequence(&TEMP_SEQUENCE, target)?;
        let path = parent.join(format!(
            ".lotta-write-{}-{sequence}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(StoreError::from_io(target, &error)),
        }
    }
    Err(StoreError::new(StoreErrorKind::Limit, target))
}

pub(crate) fn next_sequence(counter: &AtomicU64, path: &Path) -> Result<u64, StoreError> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))
}

fn flush_parent(parent: &Path, observer: &dyn AtomicObserver) -> Result<(), StoreError> {
    observer.before_parent_sync(parent)?;
    let directory = File::open(parent).map_err(|error| StoreError::from_io(parent, &error))?;
    directory
        .sync_all()
        .map_err(|error| StoreError::from_io(parent, &error))?;
    observer.parent_flushed(parent);
    Ok(())
}

fn validate_target(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    if !path.is_absolute() || bytes.len() > ATOMIC_WRITE_BYTES_MAX {
        return Err(invalid_path(path));
    }
    let name = path.file_name().ok_or_else(|| invalid_path(path))?;
    if name.is_empty()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(invalid_path(path));
    }
    Ok(())
}

const fn retryable(kind: StoreErrorKind) -> bool {
    matches!(kind, StoreErrorKind::Io | StoreErrorKind::DiskFull)
}

fn logged_once<T>(operation: impl FnOnce() -> Result<T, StoreError>) -> Result<T, StoreError> {
    operation().map_err(StoreError::log)
}

fn invalid_path(path: &Path) -> StoreError {
    StoreError::new(StoreErrorKind::InvalidPath, path)
}

#[cfg(unix)]
fn apply_directory_mode(path: &Path, mode: WriteMode) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    if mode == WriteMode::ProviderAuth {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| StoreError::from_io(path, &error))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_directory_mode(_path: &Path, _mode: WriteMode) -> Result<(), StoreError> {
    Ok(())
}

#[cfg(unix)]
fn apply_file_mode(file: &File, path: &Path, mode: WriteMode) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    if mode == WriteMode::ProviderAuth {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| StoreError::from_io(path, &error))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_file_mode(_file: &File, _path: &Path, _mode: WriteMode) -> Result<(), StoreError> {
    Ok(())
}

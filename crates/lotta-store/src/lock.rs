use crate::confinement::{create_confined_parent, validate_existing};
use crate::{StoreError, StoreErrorKind};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Lotta writer lock wait bound: immediate nonblocking acquisition.
pub const LOTTA_STORAGE_LOCK_WAIT_MS: u64 = 0;
const LOCK_FILE_NAME: &str = ".lotta-storage.lock";

/// RAII advisory lock serializing participating Lotta writers only.
///
/// TypeScript and external processes do not acquire this lock; source revision checks remain
/// required and mixed-runtime concurrent writes are unsupported. Standard-library path walks are
/// best-effort against TOCTOU replacement and do not claim handle-relative race freedom.
#[derive(Debug)]
pub struct LottaStorageLock {
    file: File,
    path: PathBuf,
    root: PathBuf,
}

impl LottaStorageLock {
    /// Immediately attempts to acquire the local-backend writer lock.
    ///
    /// # Errors
    /// Returns a typed path, filesystem, or immediate contention failure.
    pub fn try_acquire(root: &Path) -> Result<Self, StoreError> {
        Self::try_acquire_confined(root).map_err(StoreError::log)
    }

    pub(crate) fn try_acquire_confined(root: &Path) -> Result<Self, StoreError> {
        debug_assert_eq!(LOTTA_STORAGE_LOCK_WAIT_MS, 0);
        create_confined_parent(root, root)?;
        validate_existing(root, root)?;
        let path = root.join(LOCK_FILE_NAME);
        validate_existing(root, &path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| StoreError::from_io(&path, &error))?;
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| StoreError::from_io(&path, &error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, &path));
        }
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => StoreError::new(StoreErrorKind::LottaLock, &path),
            std::fs::TryLockError::Error(error) => StoreError::from_io(&path, &error),
        })?;
        Ok(Self {
            file,
            path,
            root: root.to_path_buf(),
        })
    }

    /// Returns the lock-file path for safe diagnostics.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn guards_root(&self, root: &Path) -> bool {
        self.root == root
    }
}

impl Drop for LottaStorageLock {
    fn drop(&mut self) {
        if let Err(error) = self.file.unlock() {
            tracing::error!(
                error_code = "lotta_lock_release",
                path = %self.path.display(),
                error_kind = ?error.kind(),
                "storage lock release failed"
            );
        }
    }
}

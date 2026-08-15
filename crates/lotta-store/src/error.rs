use std::io;
use std::path::{Path, PathBuf};

/// Stable machine-readable persistence error kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreErrorKind {
    /// The filesystem reported exhausted storage capacity.
    DiskFull,
    /// The operating system denied access.
    Permission,
    /// Stored JSON could not be parsed or validated.
    Parse,
    /// Persisted bytes failed an integrity check.
    Checksum,
    /// The source changed after its revision was sampled.
    StorageConflict,
    /// Another participating Lotta writer holds the advisory lock.
    LottaLock,
    /// A requested record is absent.
    NotFound,
    /// A path, identifier, or configured root is invalid.
    InvalidPath,
    /// A bounded resource ceiling rejected the operation.
    Limit,
    /// An unclassified filesystem operation failed.
    Io,
}

impl StoreErrorKind {
    /// Returns the stable textual error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::DiskFull => "disk_full",
            Self::Permission => "permission",
            Self::Parse => "parse",
            Self::Checksum => "checksum",
            Self::StorageConflict => "storage_conflict",
            Self::LottaLock => "lotta_lock",
            Self::NotFound => "not_found",
            Self::InvalidPath => "invalid_path",
            Self::Limit => "limit",
            Self::Io => "io",
        }
    }
}

/// Scrubbed persistence failure containing a path but never file contents.
#[derive(Debug, thiserror::Error)]
#[error("{kind_code}: storage operation failed at {path}", kind_code = .kind.code())]
pub struct StoreError {
    kind: StoreErrorKind,
    path: PathBuf,
}

impl StoreError {
    /// Creates a scrubbed typed error.
    #[must_use]
    pub fn new(kind: StoreErrorKind, path: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            path: path.into(),
        }
    }

    /// Returns the stable error kind.
    #[must_use]
    pub const fn kind(&self) -> StoreErrorKind {
        self.kind
    }

    /// Returns the safe path involved in the operation.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn from_io(path: &Path, error: &io::Error) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::PermissionDenied => StoreErrorKind::Permission,
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => StoreErrorKind::DiskFull,
            io::ErrorKind::NotFound => StoreErrorKind::NotFound,
            _ => StoreErrorKind::Io,
        };
        Self::new(kind, path)
    }

    /// Emits the scrubbed stable code and path without record contents.
    #[must_use]
    pub fn log(self) -> Self {
        tracing::error!(
            error_code = self.kind.code(),
            path = %self.path.display(),
            "storage operation failed"
        );
        self
    }
}

impl From<StoreError> for lotta_runtime::RuntimeError {
    fn from(error: StoreError) -> Self {
        let context = error.path.display().to_string();
        match error.kind {
            StoreErrorKind::NotFound => Self::NotFound { context },
            StoreErrorKind::StorageConflict => Self::Conflict { context },
            StoreErrorKind::Permission => Self::PermissionDenied { context },
            StoreErrorKind::InvalidPath | StoreErrorKind::Parse | StoreErrorKind::Checksum => {
                Self::InvalidData { context }
            }
            StoreErrorKind::Limit => Self::LimitExceeded { context },
            kind => Self::AdapterFailure {
                code: kind.code(),
                context,
            },
        }
    }
}

//! Capability-confined workspace and artifact file tools.

mod control;
mod definitions;
mod executor;
mod fs;
mod glob;
mod operations;
pub(crate) mod patch;

use crate::registry::ToolRegistration;
use cap_std::{ambient_authority, fs::Dir};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub use operations::{IMAGE_BYTES_MAX, TEXT_FILE_BYTES_MAX};

/// Timeout applied to each raw listener file operation.
pub const LISTENER_OPERATION_TIMEOUT_SECS: u64 = 30;

/// Fixed failure of a raw listener file operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawFileError {
    /// The path or payload violated tool policy bounds.
    Rejected,
    /// Local filesystem infrastructure failed.
    Infrastructure,
    /// The operation was interrupted or exceeded its timeout.
    Interrupted,
}

/// Replacement positions reported by a raw listener edit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListenerEditReport {
    /// Number of replacements applied.
    pub replacements: usize,
    /// One-based line of the first replacement.
    pub start_line: usize,
}

fn classify(error: fs::FileError) -> RawFileError {
    match error {
        fs::FileError::Tool => RawFileError::Rejected,
        fs::FileError::Infrastructure => RawFileError::Infrastructure,
        fs::FileError::Control(_) => RawFileError::Interrupted,
    }
}

/// Fixed file bundle construction failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileBundleError;

/// Configured file tools sharing explicit workspace and artifact capabilities.
pub struct FileToolBundle {
    state: Arc<FileState>,
    registrations: Vec<ToolRegistration>,
}

pub(crate) struct FileState {
    workspace_root: PathBuf,
    artifact_root: PathBuf,
    workspace: Dir,
    artifacts: Dir,
    mutations: Mutex<()>,
}

impl FileToolBundle {
    /// Opens canonical, existing, non-symlink roots and constructs exact registrations.
    ///
    /// # Errors
    /// Returns a fixed error unless both roots are explicit canonical ordinary directories.
    pub fn new(workspace_root: &Path, artifact_root: &Path) -> Result<Self, FileBundleError> {
        validate_root(workspace_root)?;
        validate_root(artifact_root)?;
        let workspace = Dir::open_ambient_dir(workspace_root, ambient_authority())
            .map_err(|_| FileBundleError)?;
        let artifacts = Dir::open_ambient_dir(artifact_root, ambient_authority())
            .map_err(|_| FileBundleError)?;
        let state = Arc::new(FileState {
            workspace_root: workspace_root.to_owned(),
            artifact_root: artifact_root.to_owned(),
            workspace,
            artifacts,
            mutations: Mutex::new(()),
        });
        let registrations = definitions::registrations(&state)?;
        Ok(Self {
            state,
            registrations,
        })
    }

    /// Returns all canonical core and pinned adapter registrations.
    #[must_use]
    pub fn registrations(&self) -> &[ToolRegistration] {
        &self.registrations
    }

    /// Returns the explicit canonical workspace root.
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.state.workspace_root
    }

    /// Returns the explicit canonical artifact root.
    #[must_use]
    pub fn artifact_root(&self) -> &Path {
        &self.state.artifact_root
    }

    /// Reads raw, unformatted workspace bytes for the WebSocket files group.
    ///
    /// This is the Task 65 listener seam: it routes through the same
    /// confinement (`workspace_relative`) and no-follow bounded read
    /// (`read_regular`) as the `Read` tool executor, but returns raw bytes
    /// instead of an LLM-formatted report.
    ///
    /// # Errors
    /// Returns [`RawFileError`] when the path or size violates tool policy or
    /// the filesystem read fails.
    pub fn listener_read_bytes(
        &self,
        value: &str,
        byte_max: usize,
    ) -> Result<Vec<u8>, RawFileError> {
        let control = Self::raw_control()?;
        operations::read_raw(&self.state, value, byte_max, &control).map_err(classify)
    }

    /// Creates or fully replaces one workspace text file with `Write` semantics.
    ///
    /// # Errors
    /// Returns [`RawFileError`] when the path or content violates tool policy
    /// or the atomic write fails.
    pub fn listener_write_text(&self, value: &str, content: &str) -> Result<(), RawFileError> {
        let control = Self::raw_control()?;
        operations::write_raw(&self.state, value, content, &control).map_err(classify)
    }

    /// Applies `Edit` semantics and reports replacement positions.
    ///
    /// # Errors
    /// Returns [`RawFileError`] when the edit violates tool policy (including
    /// replacement-count expectations) or the filesystem write fails.
    pub fn listener_edit(
        &self,
        value: &str,
        old: &str,
        new: &str,
        replace_all: bool,
        expected_replacements: Option<usize>,
    ) -> Result<ListenerEditReport, RawFileError> {
        let control = Self::raw_control()?;
        operations::edit_raw(
            &self.state,
            value,
            old,
            new,
            replace_all,
            expected_replacements,
            &control,
        )
        .map(|(replacements, start_line)| ListenerEditReport {
            replacements,
            start_line,
        })
        .map_err(classify)
    }

    fn raw_control() -> Result<control::OperationControl, RawFileError> {
        control::OperationControl::new(
            CancellationToken::new(),
            std::time::Duration::from_secs(LISTENER_OPERATION_TIMEOUT_SECS),
        )
        .map_err(|_| RawFileError::Rejected)
    }
}

fn validate_root(path: &Path) -> Result<(), FileBundleError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| FileBundleError)?;
    if !path.is_absolute() || metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(FileBundleError);
    }
    if path.canonicalize().map_err(|_| FileBundleError)? != path {
        return Err(FileBundleError);
    }
    Ok(())
}

#[cfg(test)]
mod clamps;
#[cfg(test)]
mod confinement;
#[cfg(test)]
mod execution;
#[cfg(test)]
mod names_match_baseline;
#[cfg(test)]
mod patch_tests;
#[cfg(test)]
mod test_support;

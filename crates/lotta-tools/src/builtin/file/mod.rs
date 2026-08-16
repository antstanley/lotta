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

pub use operations::{IMAGE_BYTES_MAX, TEXT_FILE_BYTES_MAX};

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

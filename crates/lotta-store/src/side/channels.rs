use super::{ChannelFile, OpaqueFile, SidePaths};
#[cfg(test)]
use crate::atomic::AtomicObserver;
use crate::confinement::validate_existing;
use crate::{StoreError, StoreErrorKind};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Maximum regular files returned from one channel tree.
pub const CHANNEL_FILES_MAX: usize = 256;
/// Maximum relative path depth in channel enumeration.
pub const CHANNEL_FILE_DEPTH_MAX: usize = 8;
/// Maximum total directory entries visited in one channel tree.
pub const CHANNEL_ENTRIES_MAX: usize = 512;

/// Reads the pending-control file as exact opaque bytes.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn read_pending(paths: &SidePaths) -> Result<OpaqueFile, StoreError> {
    super::io::read(&paths.pending_control()?)
}

/// Atomically replaces the pending-control file.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn write_pending(paths: &SidePaths, bytes: &[u8]) -> Result<(), StoreError> {
    super::io::write(&paths.pending_control()?, bytes)
}

/// Reads one known channel file as exact opaque bytes.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn read(
    paths: &SidePaths,
    channel_id: &str,
    file: ChannelFile,
) -> Result<OpaqueFile, StoreError> {
    super::io::read(&paths.channel_file(channel_id, file)?)
}

/// Atomically replaces only one known channel file without inspecting siblings.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn write(
    paths: &SidePaths,
    channel_id: &str,
    file: ChannelFile,
    bytes: &[u8],
) -> Result<(), StoreError> {
    super::io::write(&paths.channel_file(channel_id, file)?, bytes)
}

/// Lists deterministic channel-relative regular files, including unknown plugin files.
///
/// # Errors
/// Returns a typed path, limit, conflict, missing, lock, or filesystem failure.
pub fn list(paths: &SidePaths, channel_id: &str) -> Result<Vec<PathBuf>, StoreError> {
    let directory = paths.channel_dir(channel_id)?;
    let root = paths.channels_root()?;
    validate_existing(&root, &directory)?;
    require_directory(&directory)?;
    collect_tree(&directory)
}

fn require_directory(path: &Path) -> Result<(), StoreError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| StoreError::from_io(path, &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid(path));
    }
    Ok(())
}

fn collect_tree(root: &Path) -> Result<Vec<PathBuf>, StoreError> {
    let mut queue = VecDeque::new();
    queue.try_reserve(1).map_err(|_| limit(root))?;
    queue.push_back(PathBuf::new());
    let mut output = Vec::new();
    let mut visited = 0_usize;
    while let Some(relative) = queue.pop_front() {
        let directory = root.join(&relative);
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| StoreError::from_io(&directory, &error))?
        {
            visited = visited.checked_add(1).ok_or_else(|| limit(root))?;
            if visited > CHANNEL_ENTRIES_MAX {
                return Err(limit(root));
            }
            collect_entry(root, &relative, entry, &mut queue, &mut output)?;
        }
    }
    output.sort();
    Ok(output)
}

fn collect_entry(
    root: &Path,
    parent: &Path,
    entry: Result<std::fs::DirEntry, std::io::Error>,
    queue: &mut VecDeque<PathBuf>,
    output: &mut Vec<PathBuf>,
) -> Result<(), StoreError> {
    let entry = entry.map_err(|error| StoreError::from_io(root, &error))?;
    let kind = entry
        .file_type()
        .map_err(|error| StoreError::from_io(root, &error))?;
    let name = entry.file_name();
    let text = name.to_str().ok_or_else(|| invalid(root))?;
    if parent.as_os_str().is_empty() && text == ".lotta-storage.lock" {
        return Ok(());
    }
    let relative = parent.join(text);
    validate_relative(root, &relative)?;
    if kind.is_symlink() || !(kind.is_file() || kind.is_dir()) {
        return Err(invalid(root));
    }
    if kind.is_dir() {
        queue.try_reserve(1).map_err(|_| limit(root))?;
        queue.push_back(relative);
    } else {
        if output.len() >= CHANNEL_FILES_MAX {
            return Err(limit(root));
        }
        output.try_reserve(1).map_err(|_| limit(root))?;
        output.push(relative);
    }
    Ok(())
}

fn validate_relative(root: &Path, relative: &Path) -> Result<(), StoreError> {
    let text = relative.to_str().ok_or_else(|| invalid(root))?;
    if text.is_empty()
        || text.len() > super::paths::SIDE_PATH_BYTES_MAX
        || relative.components().count() > CHANNEL_FILE_DEPTH_MAX
    {
        return Err(limit(root));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn write_observed(
    paths: &SidePaths,
    channel_id: &str,
    file: ChannelFile,
    bytes: &[u8],
    observer: &dyn AtomicObserver,
) -> Result<(), StoreError> {
    super::io::write_observed(&paths.channel_file(channel_id, file)?, bytes, observer)
}

fn invalid(path: &Path) -> StoreError {
    StoreError::new(StoreErrorKind::InvalidPath, path)
}

fn limit(path: &Path) -> StoreError {
    StoreError::new(StoreErrorKind::Limit, path)
}

#[cfg(test)]
#[test]
fn preserves_plugin_files() {
    crate::side::evidence::channels::preserves_plugin_files();
}

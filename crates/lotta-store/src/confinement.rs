use crate::{StoreError, StoreErrorKind};
use std::path::{Component, Path, PathBuf};

pub(crate) fn backend_root(path: &Path) -> Result<&Path, StoreError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if matches!(
            candidate.file_name().and_then(|value| value.to_str()),
            Some("agents" | "conversations" | "memfs" | "providers" | "indexes" | "runtime")
        ) {
            return candidate.parent().ok_or_else(|| invalid(path));
        }
        current = candidate.parent();
    }
    path.parent().ok_or_else(|| invalid(path))
}

pub(crate) fn validate_root(root: &Path) -> Result<(), StoreError> {
    let mut current = root;
    loop {
        match std::fs::symlink_metadata(current) {
            Ok(metadata) if current == root && metadata.file_type().is_symlink() => {
                return Err(invalid(root));
            }
            Ok(metadata) if current == root && !metadata.is_dir() => return Err(invalid(root)),
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(parent) = current.parent() else {
                    return Ok(());
                };
                current = parent;
            }
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => return Ok(()),
            Err(error) => return Err(StoreError::from_io(current, &error)),
        }
    }
}

pub(crate) fn validate_existing(root: &Path, target: &Path) -> Result<(), StoreError> {
    if !target.starts_with(root) {
        return Err(invalid(target));
    }
    validate_root(root)?;
    let relative = target.strip_prefix(root).map_err(|_| invalid(target))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(invalid(target));
        };
        current.push(value);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(invalid(target)),
            Ok(metadata) if current != target && !metadata.is_dir() => return Err(invalid(target)),
            Ok(metadata) if current == target && !metadata.is_dir() && !metadata.is_file() => {
                return Err(invalid(target));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(StoreError::from_io(&current, &error)),
        }
    }
    Ok(())
}

pub(crate) fn create_confined_parent(root: &Path, parent: &Path) -> Result<(), StoreError> {
    validate_existing(root, parent)?;
    let relative = parent.strip_prefix(root).map_err(|_| invalid(parent))?;
    if !root.exists() {
        validate_root(root)?;
        std::fs::create_dir_all(root).map_err(|error| StoreError::from_io(root, &error))?;
        validate_root(root)?;
    }
    let mut current = PathBuf::from(root);
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(invalid(parent));
        };
        current.push(value);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(invalid(parent));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)
                    .map_err(|cause| StoreError::from_io(&current, &cause))?;
                let metadata = std::fs::symlink_metadata(&current)
                    .map_err(|cause| StoreError::from_io(&current, &cause))?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(invalid(parent));
                }
            }
            Err(error) => return Err(StoreError::from_io(&current, &error)),
        }
    }
    validate_existing(root, parent)
}

pub(crate) fn validate_regular_file(root: &Path, path: &Path) -> Result<(), StoreError> {
    validate_existing(root, path)?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| StoreError::from_io(path, &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid(path));
    }
    Ok(())
}

fn invalid(path: &Path) -> StoreError {
    StoreError::new(StoreErrorKind::InvalidPath, path)
}

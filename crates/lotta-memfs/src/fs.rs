use crate::{MEMORY_FILE_BYTES_MAX, MEMORY_FILES_MAX};
use cap_std::fs::{Dir, OpenOptions};
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{MemoryFileContent, RepositoryPath};
use std::io::{Read, Write};
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn adapter(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "memfs_io",
        context: context.into(),
    }
}

fn classify(error: &std::io::Error, context: &'static str) -> RuntimeError {
    match error.kind() {
        std::io::ErrorKind::NotFound => RuntimeError::NotFound {
            context: context.into(),
        },
        std::io::ErrorKind::PermissionDenied => RuntimeError::PermissionDenied {
            context: context.into(),
        },
        std::io::ErrorKind::AlreadyExists => RuntimeError::Conflict {
            context: context.into(),
        },
        _ => adapter(context),
    }
}

pub(crate) fn validate_repository_path(path: &RepositoryPath) -> Result<(), RuntimeError> {
    if path
        .as_path()
        .components()
        .next()
        .is_some_and(|part| part.as_os_str() == ".git")
    {
        return Err(RuntimeError::InvalidData {
            context: "internal repository path".into(),
        });
    }
    Ok(())
}

pub(crate) fn ensure_confined_directory(
    parent: &Path,
    directory: &Path,
) -> Result<(), RuntimeError> {
    validate_root(parent)?;
    if directory.parent() != Some(parent) || !directory.is_absolute() {
        return Err(invalid("confined directory"));
    }
    match std::fs::symlink_metadata(directory) {
        Ok(_) => validate_root(directory),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(directory)
                .map_err(|cause| classify(&cause, "confined directory"))?;
            validate_root(directory)
        }
        Err(error) => Err(classify(&error, "confined directory")),
    }
}

pub(crate) fn validate_root(root: &Path) -> Result<(), RuntimeError> {
    if !root.is_absolute() {
        return Err(RuntimeError::InvalidData {
            context: "memfs repository".into(),
        });
    }
    let metadata =
        std::fs::symlink_metadata(root).map_err(|error| classify(&error, "memfs repository"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(RuntimeError::InvalidData {
            context: "memfs repository".into(),
        });
    }
    let canonical =
        std::fs::canonicalize(root).map_err(|error| classify(&error, "memfs repository"))?;
    if canonical != root {
        return Err(RuntimeError::InvalidData {
            context: "memfs repository authority".into(),
        });
    }
    Ok(())
}

fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}

fn filename(path: &RepositoryPath) -> Result<&std::ffi::OsStr, RuntimeError> {
    path.as_path()
        .file_name()
        .ok_or_else(|| invalid("repository filename"))
}

fn open_parent(root: &Dir, path: &RepositoryPath, create: bool) -> Result<Dir, RuntimeError> {
    validate_repository_path(path)?;
    let mut current = root
        .try_clone()
        .map_err(|error| classify(&error, "repository parent"))?;
    let parent = path
        .as_path()
        .parent()
        .ok_or_else(|| invalid("repository parent"))?;
    for component in parent.components() {
        let Component::Normal(name) = component else {
            return Err(invalid("repository parent"));
        };
        match current.symlink_metadata(name) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
            Ok(_) => return Err(invalid("repository ancestor")),
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => current
                .create_dir(name)
                .map_err(|cause| classify(&cause, "repository parent"))?,
            Err(error) => return Err(classify(&error, "repository parent")),
        }
        current = current
            .open_dir(name)
            .map_err(|error| classify(&error, "repository parent"))?;
    }
    Ok(current)
}

fn regular(metadata: &cap_std::fs::Metadata) -> Result<(), RuntimeError> {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("memory regular file"));
    }
    Ok(())
}

pub(crate) fn read_file(
    root: &Dir,
    relative: &RepositoryPath,
) -> Result<MemoryFileContent, RuntimeError> {
    read_file_with_hook(root, relative, || {})
}

pub(crate) fn read_file_with_hook<F>(
    root: &Dir,
    relative: &RepositoryPath,
    hook: F,
) -> Result<MemoryFileContent, RuntimeError>
where
    F: FnOnce(),
{
    let parent = open_parent(root, relative, false)?;
    let name = filename(relative)?;
    regular(
        &parent
            .symlink_metadata(name)
            .map_err(|error| classify(&error, "memory file"))?,
    )?;
    hook();
    let file = parent
        .open(name)
        .map_err(|error| classify(&error, "memory file"))?;
    let metadata = file
        .metadata()
        .map_err(|error| classify(&error, "memory file"))?;
    regular(&metadata)?;
    let length = usize::try_from(metadata.len()).map_err(|_| RuntimeError::LimitExceeded {
        context: MEMORY_FILE_BYTES_MAX.name.into(),
    })?;
    validate_file_bytes(length)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve(length)
        .map_err(|_| RuntimeError::LimitExceeded {
            context: MEMORY_FILE_BYTES_MAX.name.into(),
        })?;
    file.take(MEMORY_FILE_BYTES_MAX.value as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| adapter("memory file"))?;
    validate_file_bytes(bytes.len())?;
    MemoryFileContent::new(bytes)
}

fn sync_directory(directory: &Dir) -> Result<(), RuntimeError> {
    directory
        .try_clone()
        .map(Dir::into_std_file)
        .and_then(|file| file.sync_all())
        .map_err(|error| classify(&error, "repository parent"))
}

pub(crate) fn write_file(
    root: &Dir,
    relative: &RepositoryPath,
    contents: &MemoryFileContent,
) -> Result<(), RuntimeError> {
    write_file_with_hook(root, relative, contents, || {})
}

pub(crate) fn write_file_with_hook<F>(
    root: &Dir,
    relative: &RepositoryPath,
    contents: &MemoryFileContent,
    hook: F,
) -> Result<(), RuntimeError>
where
    F: FnOnce(),
{
    validate_file_bytes(contents.as_slice().len())?;
    let parent = open_parent(root, relative, true)?;
    let name = filename(relative)?;
    match parent.symlink_metadata(name) {
        Ok(metadata) => regular(&metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            validate_new_file_count(scan_working(root)?.len())?;
        }
        Err(error) => return Err(classify(&error, "memory file")),
    }
    atomic_replace(&parent, name, contents.as_slice(), hook)
}

fn atomic_replace<F>(
    parent: &Dir,
    target: &std::ffi::OsStr,
    contents: &[u8],
    hook: F,
) -> Result<(), RuntimeError>
where
    F: FnOnce(),
{
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| adapter("memory temporary file"))?
        .as_nanos();
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = format!(".lotta-memfs-{stamp}-{}-{sequence}", std::process::id());
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = parent
        .open_with(&temporary, &options)
        .map_err(|error| classify(&error, "memory temporary file"))?;
    let result = (|| {
        file.write_all(contents)
            .map_err(|_| adapter("memory file"))?;
        file.sync_all().map_err(|_| adapter("memory file"))?;
        hook();
        Dir::rename(parent, &temporary, parent, target)
            .map_err(|error| classify(&error, "memory file"))?;
        sync_directory(parent)
    })();
    if result.is_err() {
        drop(parent.remove_file(&temporary));
    }
    result
}

pub(crate) fn delete_file(root: &Dir, relative: &RepositoryPath) -> Result<(), RuntimeError> {
    delete_file_with_hook(root, relative, || {})
}

pub(crate) fn delete_file_with_hook<F>(
    root: &Dir,
    relative: &RepositoryPath,
    hook: F,
) -> Result<(), RuntimeError>
where
    F: FnOnce(),
{
    let parent = open_parent(root, relative, false)?;
    let name = filename(relative)?;
    regular(
        &parent
            .symlink_metadata(name)
            .map_err(|error| classify(&error, "memory file"))?,
    )?;
    hook();
    parent
        .remove_file(name)
        .map_err(|error| classify(&error, "memory file"))?;
    sync_directory(&parent)
}

pub(crate) fn rename_file(
    root: &Dir,
    source: &RepositoryPath,
    target: &RepositoryPath,
) -> Result<(), RuntimeError> {
    rename_file_with_hook(root, source, target, || {})
}

pub(crate) fn rename_file_with_hook<F>(
    root: &Dir,
    source: &RepositoryPath,
    target: &RepositoryPath,
    hook: F,
) -> Result<(), RuntimeError>
where
    F: FnOnce(),
{
    let target_parent = open_parent(root, target, true)?;
    let target_name = filename(target)?;
    match target_parent.symlink_metadata(target_name) {
        Ok(_) => {
            return Err(RuntimeError::Conflict {
                context: "memory rename target".into(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(classify(&error, "memory rename target")),
    }
    let source_parent = open_parent(root, source, false)?;
    let source_name = filename(source)?;
    regular(
        &source_parent
            .symlink_metadata(source_name)
            .map_err(|error| classify(&error, "memory source file"))?,
    )?;
    hook();
    Dir::rename(&source_parent, source_name, &target_parent, target_name)
        .map_err(|error| classify(&error, "memory rename"))?;
    sync_directory(&source_parent)?;
    sync_directory(&target_parent)
}

pub(crate) fn scan_working(root: &Dir) -> Result<Vec<RepositoryPath>, RuntimeError> {
    let mut files = Vec::new();
    scan_directory(root, Path::new(""), &mut files)?;
    files.sort();
    Ok(files)
}

fn scan_directory(
    directory: &Dir,
    prefix: &Path,
    files: &mut Vec<RepositoryPath>,
) -> Result<(), RuntimeError> {
    let mut entries = Vec::new();
    for entry in directory
        .entries()
        .map_err(|error| classify(&error, "memory tree"))?
    {
        validate_new_file_count(entries.len())?;
        entries
            .try_reserve(1)
            .map_err(|_| RuntimeError::LimitExceeded {
                context: MEMORY_FILES_MAX.name.into(),
            })?;
        entries.push(entry.map_err(|error| classify(&error, "memory tree"))?);
    }
    entries.sort_by_key(cap_std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        if prefix.as_os_str().is_empty() && name == ".git" {
            continue;
        }
        if name.to_str().is_none() {
            return Err(invalid("memory tree entry"));
        }
        let metadata = entry
            .file_type()
            .map_err(|error| classify(&error, "memory tree"))?;
        if metadata.is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
            return Err(invalid("memory tree entry"));
        }
        let path = prefix.join(&name);
        if metadata.is_dir() {
            let child = directory
                .open_dir(&name)
                .map_err(|error| classify(&error, "memory tree"))?;
            scan_directory(&child, &path, files)?;
        } else {
            validate_file_count(files.len())?;
            files
                .try_reserve(1)
                .map_err(|_| RuntimeError::LimitExceeded {
                    context: MEMORY_FILES_MAX.name.into(),
                })?;
            files.push(RepositoryPath::new(path)?);
            validate_file_count(files.len())?;
        }
    }
    Ok(())
}

pub(crate) fn validate_file_bytes(value: usize) -> Result<(), RuntimeError> {
    if value > MEMORY_FILE_BYTES_MAX.value {
        return Err(RuntimeError::LimitExceeded {
            context: MEMORY_FILE_BYTES_MAX.name.into(),
        });
    }
    Ok(())
}

pub(crate) fn validate_new_file_count(existing: usize) -> Result<(), RuntimeError> {
    let value = existing
        .checked_add(1)
        .ok_or_else(|| RuntimeError::LimitExceeded {
            context: MEMORY_FILES_MAX.name.into(),
        })?;
    validate_file_count(value)
}

pub(crate) fn validate_file_count(value: usize) -> Result<(), RuntimeError> {
    if value > MEMORY_FILES_MAX.value {
        return Err(RuntimeError::LimitExceeded {
            context: MEMORY_FILES_MAX.name.into(),
        });
    }
    Ok(())
}

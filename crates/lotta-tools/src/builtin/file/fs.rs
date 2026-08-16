use super::{
    FileState,
    control::OperationControl,
    operations::{PATH_BYTES_MAX, PATH_COMPONENTS_MAX},
};
use crate::permissions::{canonicalize_invocation_path, path_within};
use cap_fs_ext::{FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const TEMP_RETRIES_MAX: usize = 32;

#[cfg(test)]
thread_local! {
    static WRITE_HOOKS: std::cell::RefCell<Vec<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileError {
    Tool,
    Infrastructure,
    Control(super::control::ControlError),
}

pub(super) fn workspace_relative(state: &FileState, value: &str) -> Result<PathBuf, FileError> {
    strict_lexical(value, true)?;
    let resolved =
        canonicalize_invocation_path(&state.workspace_root, value).map_err(|_| FileError::Tool)?;
    if !path_within(&resolved, &state.workspace_root) {
        return Err(FileError::Tool);
    }
    let relative = resolved
        .strip_prefix(&state.workspace_root)
        .map_err(|_| FileError::Tool)?;
    revalidate_existing(&state.workspace, relative)?;
    Ok(relative.to_owned())
}

pub(super) fn artifact_relative(state: &FileState, value: &str) -> Result<PathBuf, FileError> {
    let stripped = value
        .strip_prefix("external/artifacts/")
        .or_else(|| value.strip_prefix("artifacts/"))
        .unwrap_or(value);
    strict_lexical(stripped, false)?;
    let path = Path::new(stripped);
    revalidate_existing(&state.artifacts, path)?;
    Ok(path.to_owned())
}

fn strict_lexical(value: &str, allow_absolute: bool) -> Result<(), FileError> {
    if value.is_empty() || value.len() > PATH_BYTES_MAX || value.contains(['\0', '\\']) {
        return Err(FileError::Tool);
    }
    let path = Path::new(value);
    if path.is_absolute() && !allow_absolute {
        return Err(FileError::Tool);
    }
    let mut count = 0usize;
    for component in path.components() {
        match component {
            Component::ParentDir | Component::Prefix(_) => return Err(FileError::Tool),
            Component::Normal(value) if value == OsStr::new("..") => return Err(FileError::Tool),
            Component::Normal(_) => count = count.checked_add(1).ok_or(FileError::Tool)?,
            Component::RootDir | Component::CurDir => {}
        }
    }
    if count == 0 || count > PATH_COMPONENTS_MAX {
        Err(FileError::Tool)
    } else {
        Ok(())
    }
}

pub(super) fn revalidate_existing(dir: &Dir, relative: &Path) -> Result<(), FileError> {
    let mut current = PathBuf::new();
    let component_count = relative.components().count();
    if component_count > PATH_COMPONENTS_MAX {
        return Err(FileError::Tool);
    }
    let mut components = Vec::new();
    components
        .try_reserve_exact(component_count)
        .map_err(|_| FileError::Tool)?;
    components.extend(relative.components());
    for (index, component) in components.iter().enumerate() {
        if let Component::Normal(value) = component {
            current.push(value);
            match dir.symlink_metadata(&current) {
                Ok(meta) => {
                    if meta.file_type().is_symlink()
                        || (!meta.is_dir() && index + 1 < components.len())
                    {
                        return Err(FileError::Tool);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => return Err(FileError::Tool),
            }
        }
    }
    Ok(())
}

pub(super) fn read_regular(
    dir: &Dir,
    relative: &Path,
    maximum: usize,
    control: &OperationControl,
) -> Result<Vec<u8>, FileError> {
    revalidate_existing(dir, relative)?;
    let meta = dir
        .symlink_metadata(relative)
        .map_err(|_| FileError::Tool)?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > maximum as u64 {
        return Err(FileError::Tool);
    }
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = dir
        .open_with(relative, &options)
        .map_err(|_| FileError::Tool)?;
    let opened_meta = file.metadata().map_err(|_| FileError::Tool)?;
    if !opened_meta.is_file() || opened_meta.len() > maximum as u64 {
        return Err(FileError::Tool);
    }
    let capacity = usize::try_from(opened_meta.len())
        .map_err(|_| FileError::Tool)?
        .checked_add(1)
        .ok_or(FileError::Tool)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| FileError::Tool)?;
    let mut limited = file.take((maximum as u64).checked_add(1).ok_or(FileError::Tool)?);
    let mut chunk = Vec::new();
    chunk
        .try_reserve_exact(64 * 1024)
        .map_err(|_| FileError::Tool)?;
    chunk.resize(64 * 1024, 0);
    loop {
        control.check()?;
        let count = limited.read(&mut chunk).map_err(|_| FileError::Tool)?;
        if count == 0 {
            break;
        }
        let length = output.len().checked_add(count).ok_or(FileError::Tool)?;
        if length > maximum {
            return Err(FileError::Tool);
        }
        output.try_reserve(count).map_err(|_| FileError::Tool)?;
        output.extend_from_slice(&chunk[..count]);
    }
    Ok(output)
}

pub(super) fn atomic_write(dir: &Dir, relative: &Path, bytes: &[u8]) -> Result<(), FileError> {
    let parent = relative.parent().unwrap_or(Path::new(""));
    create_parents(dir, parent)?;
    revalidate_existing(dir, relative)?;
    let stage = create_stage(dir, parent, relative, bytes)?;
    let result = replace_staged(dir, relative, &stage);
    if dir.remove_file(&stage).is_err() {
        return Err(FileError::Infrastructure);
    }
    result
}

fn replace_staged(dir: &Dir, target: &Path, stage: &Path) -> Result<(), FileError> {
    revalidate_existing(dir, target)?;
    let identity = match dir.symlink_metadata(target) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Some(identity(&meta)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Ok(_) | Err(_) => return Err(FileError::Tool),
    };
    run_write_hook();
    match identity {
        None => install_absent(dir, target, stage),
        Some(expected) => replace_existing(dir, target, stage, expected),
    }
}

fn install_absent(dir: &Dir, target: &Path, stage: &Path) -> Result<(), FileError> {
    dir.hard_link(stage, dir, target)
        .map_err(|_| FileError::Tool)?;
    sync_parent(dir, target)
}

fn replace_existing(
    dir: &Dir,
    target: &Path,
    stage: &Path,
    expected: (u64, u64),
) -> Result<(), FileError> {
    let backup = unique_unused(dir, target, "bak")?;
    dir.rename(target, dir, &backup)
        .map_err(|_| FileError::Tool)?;
    if backup_identity(dir, &backup)? != expected {
        run_write_hook();
        return restore_mismatch(dir, target, &backup);
    }
    run_write_hook();
    if dir.hard_link(stage, dir, target).is_err() {
        return rollback_backup(dir, target, &backup);
    }
    sync_parent(dir, target)?;
    dir.remove_file(&backup)
        .map_err(|_| FileError::Infrastructure)?;
    sync_parent(dir, target)
}

fn restore_mismatch(dir: &Dir, target: &Path, backup: &Path) -> Result<(), FileError> {
    match dir.symlink_metadata(target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            dir.hard_link(backup, dir, target)
                .map_err(|_| FileError::Infrastructure)?;
            sync_parent(dir, target).map_err(|_| FileError::Infrastructure)?;
            dir.remove_file(backup)
                .map_err(|_| FileError::Infrastructure)?;
        }
        Ok(_) | Err(_) => return Err(FileError::Infrastructure),
    }
    Err(FileError::Tool)
}

fn rollback_backup(dir: &Dir, target: &Path, backup: &Path) -> Result<(), FileError> {
    if dir.hard_link(backup, dir, target).is_err() {
        return Err(FileError::Infrastructure);
    }
    sync_parent(dir, target).map_err(|_| FileError::Infrastructure)?;
    dir.remove_file(backup)
        .map_err(|_| FileError::Infrastructure)?;
    Err(FileError::Tool)
}

fn backup_identity(dir: &Dir, backup: &Path) -> Result<(u64, u64), FileError> {
    let meta = dir
        .symlink_metadata(backup)
        .map_err(|_| FileError::Infrastructure)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(FileError::Infrastructure);
    }
    Ok(identity(&meta))
}

#[cfg(unix)]
fn identity(meta: &cap_std::fs::Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn identity(meta: &cap_std::fs::Metadata) -> (u64, u64) {
    (meta.len(), 0)
}

fn create_stage(
    dir: &Dir,
    parent: &Path,
    target: &Path,
    bytes: &[u8],
) -> Result<PathBuf, FileError> {
    for _ in 0..TEMP_RETRIES_MAX {
        let path = unique_name(parent, target, "tmp");
        match write_temp(dir, &path, bytes) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(FileError::Infrastructure),
        }
    }
    Err(FileError::Infrastructure)
}

fn unique_unused(dir: &Dir, target: &Path, suffix: &str) -> Result<PathBuf, FileError> {
    let parent = target.parent().unwrap_or(Path::new(""));
    for _ in 0..TEMP_RETRIES_MAX {
        let path = unique_name(parent, target, suffix);
        match dir.symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(path),
            Ok(_) => {}
            Err(_) => return Err(FileError::Infrastructure),
        }
    }
    Err(FileError::Infrastructure)
}

fn unique_name(parent: &Path, target: &Path, suffix: &str) -> PathBuf {
    let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = target.file_name().and_then(OsStr::to_str).unwrap_or("file");
    parent.join(format!(
        ".{name}.lotta-{}-{count}.{suffix}",
        std::process::id()
    ))
}

fn write_temp(dir: &Dir, path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = dir.open_with(path, &options)?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    if result.is_err() {
        let _ = dir.remove_file(path);
    }
    result
}

fn sync_parent(dir: &Dir, target: &Path) -> Result<(), FileError> {
    sync_dir(dir, target.parent().unwrap_or(Path::new("")))
}

fn run_write_hook() {
    #[cfg(test)]
    WRITE_HOOKS.with(|slot| {
        if !slot.borrow().is_empty() {
            let hook = slot.borrow_mut().remove(0);
            hook();
        }
    });
}

#[cfg(test)]
pub(crate) fn set_write_hooks(hooks: Vec<Box<dyn FnOnce()>>) {
    WRITE_HOOKS.with(|slot| *slot.borrow_mut() = hooks);
}

pub(super) fn create_parents(dir: &Dir, parent: &Path) -> Result<(), FileError> {
    let mut current = PathBuf::new();
    for component in parent.components() {
        let Component::Normal(value) = component else {
            continue;
        };
        current.push(value);
        match dir.symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                dir.create_dir(&current).map_err(|_| FileError::Tool)?;
            }
            Ok(_) | Err(_) => return Err(FileError::Tool),
        }
    }
    Ok(())
}

pub(super) fn sync_dir(dir: &Dir, path: &Path) -> Result<(), FileError> {
    let target = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    let opened = dir.open_dir(target).map_err(|_| FileError::Tool)?;
    opened
        .open(".")
        .and_then(|file| file.sync_all())
        .map_err(|_| FileError::Tool)
}

pub(super) fn file_type(dir: &Dir, path: &Path) -> Result<cap_std::fs::FileType, FileError> {
    let meta = dir.symlink_metadata(path).map_err(|_| FileError::Tool)?;
    if meta.file_type().is_symlink() {
        return Err(FileError::Tool);
    }
    Ok(meta.file_type())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::ambient_authority;
    use std::fs;

    fn fixture(label: &str) -> (PathBuf, Dir) {
        let path = std::env::temp_dir().join(format!(
            "lotta-fs-{}-{label}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let dir = Dir::open_ambient_dir(&path, ambient_authority()).unwrap();
        (path, dir)
    }

    fn residue(path: &Path) -> Vec<PathBuf> {
        fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".lotta-")
            })
            .collect()
    }

    #[test]
    fn lexical_byte_and_component_boundaries() {
        assert_eq!(
            strict_lexical(&"a".repeat(PATH_BYTES_MAX - 1), false),
            Ok(())
        );
        assert_eq!(strict_lexical(&"a".repeat(PATH_BYTES_MAX), false), Ok(()));
        assert_eq!(
            strict_lexical(&"a".repeat(PATH_BYTES_MAX + 1), false),
            Err(FileError::Tool)
        );
        let components = |count| {
            std::iter::repeat_n("a", count)
                .collect::<Vec<_>>()
                .join("/")
        };
        assert_eq!(
            strict_lexical(&components(PATH_COMPONENTS_MAX - 1), false),
            Ok(())
        );
        assert_eq!(
            strict_lexical(&components(PATH_COMPONENTS_MAX), false),
            Ok(())
        );
        assert_eq!(
            strict_lexical(&components(PATH_COMPONENTS_MAX + 1), false),
            Err(FileError::Tool)
        );
    }

    #[test]
    fn atomic_create_and_replace_leave_no_residue() {
        let (root, dir) = fixture("success");
        atomic_write(&dir, Path::new("new"), b"created").unwrap();
        assert_eq!(fs::read(root.join("new")).unwrap(), b"created");
        fs::write(root.join("old"), b"original").unwrap();
        atomic_write(&dir, Path::new("old"), b"replacement").unwrap();
        assert_eq!(fs::read(root.join("old")).unwrap(), b"replacement");
        assert!(residue(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_absent_race_preserves_attacker() {
        let (root, dir) = fixture("absent-race");
        let target = root.join("target");
        set_write_hooks(vec![Box::new(move || {
            fs::write(target, b"attacker").unwrap();
        })]);
        assert_eq!(
            atomic_write(&dir, Path::new("target"), b"tool"),
            Err(FileError::Tool)
        );
        assert_eq!(fs::read(root.join("target")).unwrap(), b"attacker");
        assert!(residue(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_existing_identity_race_preserves_attacker() {
        let (root, dir) = fixture("identity-race");
        fs::write(root.join("target"), b"original").unwrap();
        let target = root.join("target");
        set_write_hooks(vec![Box::new(move || {
            fs::remove_file(&target).unwrap();
            fs::write(target, b"attacker").unwrap();
        })]);
        assert_eq!(
            atomic_write(&dir, Path::new("target"), b"tool"),
            Err(FileError::Tool)
        );
        assert_eq!(fs::read(root.join("target")).unwrap(), b"attacker");
        assert!(residue(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_nested_identity_race_retains_both_attackers() {
        let (root, dir) = fixture("nested-identity-race");
        fs::write(root.join("target"), b"original").unwrap();
        let first = root.join("target");
        let second = first.clone();
        set_write_hooks(vec![
            Box::new(move || {
                fs::remove_file(&first).unwrap();
                fs::write(first, b"attacker-one").unwrap();
            }),
            Box::new(move || fs::write(second, b"attacker-two").unwrap()),
        ]);
        assert_eq!(
            atomic_write(&dir, Path::new("target"), b"tool"),
            Err(FileError::Infrastructure)
        );
        assert_eq!(fs::read(root.join("target")).unwrap(), b"attacker-two");
        let evidence = residue(&root);
        assert_eq!(evidence.len(), 1);
        assert_eq!(fs::read(&evidence[0]).unwrap(), b"attacker-one");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_post_backup_race_retains_original_recovery() {
        let (root, dir) = fixture("backup-race");
        fs::write(root.join("target"), b"original").unwrap();
        let target = root.join("target");
        set_write_hooks(vec![
            Box::new(|| {}),
            Box::new(move || {
                fs::write(target, b"attacker").unwrap();
            }),
        ]);
        assert_eq!(
            atomic_write(&dir, Path::new("target"), b"tool"),
            Err(FileError::Infrastructure)
        );
        assert_eq!(fs::read(root.join("target")).unwrap(), b"attacker");
        let evidence = residue(&root);
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].extension().is_some_and(|value| value == "bak"));
        assert_eq!(fs::read(&evidence[0]).unwrap(), b"original");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retry_bound_accepts_below_and_at_then_exhausts() {
        let (root, dir) = fixture("retries");
        let target = Path::new("target");
        let start = TEMP_COUNTER.load(Ordering::Relaxed);
        for count in start..start + TEMP_RETRIES_MAX as u64 - 1 {
            let name = format!(".target.lotta-{}-{count}.tmp", std::process::id());
            fs::write(root.join(name), b"collision").unwrap();
        }
        assert!(create_stage(&dir, Path::new(""), target, b"ok").is_ok());
        let start = TEMP_COUNTER.load(Ordering::Relaxed);
        for count in start..start + TEMP_RETRIES_MAX as u64 {
            let name = format!(".target.lotta-{}-{count}.tmp", std::process::id());
            fs::write(root.join(name), b"collision").unwrap();
        }
        assert_eq!(
            create_stage(&dir, Path::new(""), target, b"no"),
            Err(FileError::Infrastructure)
        );
        fs::remove_dir_all(root).unwrap();
    }
}

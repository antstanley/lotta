use super::apply_patch::Effect;
use crate::builtin::file::fs::{FileError, create_parents, sync_dir};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TRANSACTION_COUNTER: AtomicU64 = AtomicU64::new(0);
const TRANSACTION_RETRIES_MAX: usize = 32;
#[cfg(test)]
thread_local! {
    static FAIL_COMMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
    static PHASE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

struct Entry {
    stage: Option<PathBuf>,
    backup: Option<PathBuf>,
    source: Option<PathBuf>,
    destination: Option<PathBuf>,
    snapshot: Option<Vec<u8>>,
    source_backed_up: bool,
    destination_installed: bool,
}

pub(super) fn commit(dir: &Dir, effects: &[Effect]) -> Result<(), FileError> {
    let transaction = create_transaction(dir)?;
    let (result, retain) = transact(dir, effects, &transaction);
    if retain {
        return Err(FileError::Infrastructure);
    }
    let cleanup = cleanup_transaction(dir, &transaction);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (_, Err(())) => Err(FileError::Infrastructure),
    }
}

fn create_transaction(dir: &Dir) -> Result<PathBuf, FileError> {
    for _ in 0..TRANSACTION_RETRIES_MAX {
        let transaction = transaction_path();
        match dir.create_dir(&transaction) {
            Ok(()) => return Ok(transaction),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(FileError::Infrastructure),
        }
    }
    Err(FileError::Infrastructure)
}

fn transact(dir: &Dir, effects: &[Effect], transaction: &Path) -> (Result<(), FileError>, bool) {
    let mut entries = match stage_entries(dir, effects, transaction) {
        Ok(entries) => entries,
        Err(error) => return (Err(error), false),
    };
    for index in 0..entries.len() {
        let result = if inject_failure(index) {
            Err(FileError::Tool)
        } else {
            apply_entry(dir, &mut entries[index])
        };
        if let Err(error) = result {
            return match rollback(dir, &entries[..=index]) {
                Ok(()) => (Err(error), false),
                Err(()) => (Err(FileError::Infrastructure), true),
            };
        }
    }
    if remove_backups(dir, &entries, transaction).is_err() {
        return (Err(FileError::Infrastructure), true);
    }
    (Ok(()), false)
}

fn stage_entries(
    dir: &Dir,
    effects: &[Effect],
    transaction: &Path,
) -> Result<Vec<Entry>, FileError> {
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(effects.len())
        .map_err(|_| FileError::Infrastructure)?;
    for (index, effect) in effects.iter().enumerate() {
        let stage = stage_content(dir, transaction, index, effect.content.as_deref())?;
        let backup = effect
            .snapshot
            .as_ref()
            .map(|_| transaction.join(format!("backup-{index}")));
        entries.push(Entry {
            stage,
            backup,
            source: effect.source.clone(),
            destination: effect.destination.clone(),
            snapshot: effect.snapshot.clone(),
            source_backed_up: false,
            destination_installed: false,
        });
    }
    sync_dir(dir, transaction).map_err(|_| FileError::Infrastructure)?;
    Ok(entries)
}

fn stage_content(
    dir: &Dir,
    transaction: &Path,
    index: usize,
    content: Option<&[u8]>,
) -> Result<Option<PathBuf>, FileError> {
    content
        .map(|bytes| {
            let path = transaction.join(format!("stage-{index}"));
            write_new(dir, &path, bytes)?;
            Ok(path)
        })
        .transpose()
}

fn apply_entry(dir: &Dir, entry: &mut Entry) -> Result<(), FileError> {
    validate_entry(dir, entry)?;
    run_phase_hook();
    backup_source(dir, entry)?;
    let target = entry.destination.as_ref().or(entry.source.as_ref());
    if let (Some(stage), Some(destination)) = (&entry.stage, target) {
        install(dir, stage, destination)?;
        entry.destination_installed = true;
    }
    Ok(())
}

fn backup_source(dir: &Dir, entry: &mut Entry) -> Result<(), FileError> {
    let (Some(source), Some(backup), Some(snapshot)) = (
        entry.source.as_ref(),
        entry.backup.as_ref(),
        entry.snapshot.as_ref(),
    ) else {
        return Ok(());
    };
    dir.rename(source, dir, backup)
        .map_err(|_| FileError::Tool)?;
    entry.source_backed_up = true;
    sync_parent(dir, source)?;
    let current = read_snapshot(dir, backup, snapshot.len())?;
    if current == *snapshot {
        return Ok(());
    }
    restore_no_replace(dir, backup, source).map_err(|()| FileError::Infrastructure)?;
    entry.source_backed_up = false;
    Err(FileError::Tool)
}

fn validate_entry(dir: &Dir, entry: &Entry) -> Result<(), FileError> {
    if let (Some(source), Some(snapshot)) = (&entry.source, &entry.snapshot) {
        let current = read_snapshot(dir, source, snapshot.len())?;
        if current != *snapshot {
            return Err(FileError::Tool);
        }
    }
    if let Some(destination) = &entry.destination {
        match dir.symlink_metadata(destination) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(FileError::Tool),
        }
    }
    Ok(())
}

fn read_snapshot(dir: &Dir, path: &Path, length: usize) -> Result<Vec<u8>, FileError> {
    let metadata = dir.symlink_metadata(path).map_err(|_| FileError::Tool)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != length as u64 {
        return Err(FileError::Tool);
    }
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = dir.open_with(path, &options).map_err(|_| FileError::Tool)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(length.checked_add(1).ok_or(FileError::Tool)?)
        .map_err(|_| FileError::Infrastructure)?;
    file.take((length as u64).checked_add(1).ok_or(FileError::Tool)?)
        .read_to_end(&mut output)
        .map_err(|_| FileError::Tool)?;
    Ok(output)
}

fn install(dir: &Dir, stage: &Path, destination: &Path) -> Result<(), FileError> {
    let parent = destination.parent().unwrap_or(Path::new(""));
    create_parents(dir, parent)?;
    dir.hard_link(stage, dir, destination)
        .map_err(|_| FileError::Tool)?;
    sync_dir(dir, parent)
}

fn rollback(dir: &Dir, entries: &[Entry]) -> Result<(), ()> {
    let mut failed = false;
    for entry in entries.iter().rev() {
        if rollback_entry(dir, entry).is_err() {
            failed = true;
        }
    }
    if failed { Err(()) } else { Ok(()) }
}

fn rollback_entry(dir: &Dir, entry: &Entry) -> Result<(), ()> {
    if entry.destination_installed {
        let destination = entry
            .destination
            .as_ref()
            .or(entry.source.as_ref())
            .ok_or(())?;
        let stage = entry.stage.as_ref().ok_or(())?;
        remove_installed(dir, stage, destination)?;
    }
    if entry.source_backed_up {
        let source = entry.source.as_ref().ok_or(())?;
        let backup = entry.backup.as_ref().ok_or(())?;
        restore_no_replace(dir, backup, source)?;
    }
    Ok(())
}

#[cfg(unix)]
fn same_inode(dir: &Dir, left: &Path, right: &Path) -> Result<bool, ()> {
    use cap_fs_ext::MetadataExt;
    let left = dir.symlink_metadata(left).map_err(|_| ())?;
    let right = dir.symlink_metadata(right).map_err(|_| ())?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(not(unix))]
fn same_inode(_: &Dir, _: &Path, _: &Path) -> Result<bool, ()> {
    Err(())
}

fn remove_installed(dir: &Dir, stage: &Path, destination: &Path) -> Result<(), ()> {
    if !same_inode(dir, stage, destination)? {
        return Err(());
    }
    dir.remove_file(destination).map_err(|_| ())?;
    sync_parent(dir, destination).map_err(|_| ())
}

fn restore_no_replace(dir: &Dir, backup: &Path, destination: &Path) -> Result<(), ()> {
    let parent = destination.parent().unwrap_or(Path::new(""));
    create_parents(dir, parent).map_err(|_| ())?;
    dir.hard_link(backup, dir, destination).map_err(|_| ())?;
    sync_dir(dir, parent).map_err(|_| ())?;
    dir.remove_file(backup).map_err(|_| ())?;
    Ok(())
}

fn remove_backups(dir: &Dir, entries: &[Entry], transaction: &Path) -> Result<(), ()> {
    for entry in entries {
        if entry.source_backed_up {
            let backup = entry.backup.as_ref().ok_or(())?;
            dir.remove_file(backup).map_err(|_| ())?;
        }
    }
    sync_dir(dir, transaction).map_err(|_| ())
}

fn sync_parent(dir: &Dir, path: &Path) -> Result<(), FileError> {
    sync_dir(dir, path.parent().unwrap_or(Path::new("")))
}

fn write_new(dir: &Dir, path: &Path, bytes: &[u8]) -> Result<(), FileError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = dir
        .open_with(path, &options)
        .map_err(|_| FileError::Infrastructure)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| FileError::Infrastructure)
}

fn cleanup_transaction(dir: &Dir, transaction: &Path) -> Result<(), ()> {
    let entries = dir.read_dir(transaction).map_err(|_| ())?;
    let mut failed = false;
    for entry in entries {
        let entry = entry.map_err(|_| ())?;
        let path = transaction.join(entry.file_name());
        if dir.remove_file(path).is_err() {
            failed = true;
        }
    }
    if dir.remove_dir(transaction).is_err() {
        failed = true;
    }
    sync_dir(dir, Path::new("")).map_err(|_| ())?;
    if failed { Err(()) } else { Ok(()) }
}

fn transaction_path() -> PathBuf {
    let value = TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!(".lotta-patch-{}-{value}", std::process::id()))
}

fn inject_failure(index: usize) -> bool {
    #[cfg(test)]
    {
        FAIL_COMMIT.with(|failure| {
            if failure.get() == index {
                failure.set(usize::MAX);
                true
            } else {
                false
            }
        })
    }
    #[cfg(not(test))]
    {
        let _ = index;
        false
    }
}

fn run_phase_hook() {
    #[cfg(test)]
    PHASE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
pub(crate) fn set_phase_hook(hook: impl FnOnce() + 'static) {
    PHASE_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
pub(crate) fn fail_commit_at(index: usize) {
    FAIL_COMMIT.with(|failure| failure.set(index));
}

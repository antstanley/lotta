//! Versioned legacy transcript upgrade on explicit non-empty persistence.

use super::{
    NoopRewriteObserver, TranscriptPaths, TranscriptRewriteObserver, manifest,
    restore_migration_backup_held, rewrite_migration_held_tracked,
};
use crate::atomic::{FileRevision, atomic_write_expected_locked, next_sequence};
use crate::confinement::{validate_existing, validate_regular_file};
use crate::{LottaStorageLock, StoreError, StoreErrorKind, WriteMode};
use lotta_domain::{TranscriptEntry, TranscriptManifest, TranscriptMessageFormat};
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

pub(crate) const MIGRATION_BACKUPS_PER_FILE_MAX: usize = 3;
const BACKUP_CREATE_RETRIES_MAX: usize = 64;

pub(crate) trait BackupObserver: Send + Sync {
    fn file_flushed(&self, _path: &Path) {}
    fn parent_flushed(&self, _path: &Path) {}
}

pub(crate) struct NoopBackupObserver;
impl BackupObserver for NoopBackupObserver {}

const COPY_BUFFER_BYTES: usize = 64 * 1_024;
static BACKUP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn persist_projection(
    paths: &TranscriptPaths,
    loaded: &super::load::LoadedTranscript,
) -> Result<(), StoreError> {
    persist_projection_observed(paths, loaded, |_| Ok(()))
}

pub(crate) fn persist_projection_observed(
    paths: &TranscriptPaths,
    loaded: &super::load::LoadedTranscript,
    before_manifest_write: impl FnOnce(&Path) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    persist_projection_inner(paths, loaded, before_manifest_write, &NoopRewriteObserver)
}

#[cfg(test)]
pub(crate) fn persist_projection_rewrite_observed(
    paths: &TranscriptPaths,
    loaded: &super::load::LoadedTranscript,
    observer: &dyn TranscriptRewriteObserver,
) -> Result<(), StoreError> {
    persist_projection_inner(paths, loaded, |_| Ok(()), observer)
}

fn persist_projection_inner(
    paths: &TranscriptPaths,
    loaded: &super::load::LoadedTranscript,
    before_manifest_write: impl FnOnce(&Path) -> Result<(), StoreError>,
    rewrite_observer: &dyn TranscriptRewriteObserver,
) -> Result<(), StoreError> {
    if loaded.messages().is_empty() {
        return Ok(());
    }
    let Some(source_manifest) = loaded.manifest() else {
        return Err(StoreError::new(
            StoreErrorKind::TranscriptMigrationRequired,
            &paths.messages,
        ));
    };
    if source_manifest.message_format != TranscriptMessageFormat::PiAiMessageJsonl {
        return Ok(());
    }
    let entries = legacy_upgrade_entries(loaded, paths)?;
    let lock = LottaStorageLock::try_acquire_confined(&paths.root)?;
    verify_loaded_authority(paths, loaded)?;
    let backup = copy_backup_durable(paths, loaded.message_revision())?;
    let manifest = upgraded_manifest(source_manifest, &backup)?;
    let manifest_bytes = manifest::encode(&manifest, &paths.manifest)?;
    let (rewrite, committed) = rewrite_migration_held_tracked(
        paths,
        &entries,
        &backup,
        loaded.message_revision(),
        &lock,
        rewrite_observer,
    );
    if let Err(error) = rewrite {
        if !committed {
            return Err(error);
        }
        let converted =
            FileRevision::sample_path_bounded(&paths.messages, super::TRANSCRIPT_BYTES_MAX)?;
        return restore_after_publication_failure(paths, &backup, &converted, &lock, error);
    }
    let converted =
        FileRevision::sample_path_bounded(&paths.messages, super::TRANSCRIPT_BYTES_MAX)?;
    let publication = before_manifest_write(&paths.manifest).and_then(|()| {
        atomic_write_expected_locked(
            &paths.manifest,
            &manifest_bytes,
            WriteMode::Standard,
            loaded.manifest_revision(),
            &lock,
        )
    });
    match publication {
        Ok(()) => Ok(()),
        Err(error) if manifest::read_current(&paths.manifest).is_ok() => Err(error),
        Err(error) => restore_after_publication_failure(paths, &backup, &converted, &lock, error),
    }
}

fn restore_after_publication_failure(
    paths: &TranscriptPaths,
    backup: &Path,
    converted: &FileRevision,
    lock: &LottaStorageLock,
    publication_error: StoreError,
) -> Result<(), StoreError> {
    match restore_migration_backup_held(paths, backup, converted, lock) {
        Ok(()) => Err(publication_error),
        Err(restore_error) => {
            let active =
                FileRevision::sample_path_bounded(&paths.messages, super::TRANSCRIPT_BYTES_MAX)?;
            let recovery = FileRevision::sample_path_bounded(backup, super::TRANSCRIPT_BYTES_MAX)?;
            if active.same_contents(&recovery) {
                Err(publication_error)
            } else {
                Err(restore_error)
            }
        }
    }
}

fn verify_loaded_authority(
    paths: &TranscriptPaths,
    loaded: &super::load::LoadedTranscript,
) -> Result<(), StoreError> {
    let messages = FileRevision::sample_path_bounded(&paths.messages, super::TRANSCRIPT_BYTES_MAX)?;
    let manifest = FileRevision::sample_path(&paths.manifest)?;
    if &messages != loaded.message_revision() || &manifest != loaded.manifest_revision() {
        return Err(StoreError::new(
            StoreErrorKind::StorageConflict,
            &paths.messages,
        ));
    }
    Ok(())
}

fn legacy_upgrade_entries(
    loaded: &super::load::LoadedTranscript,
    paths: &TranscriptPaths,
) -> Result<Vec<TranscriptEntry>, StoreError> {
    use lotta_domain::{
        MessageEntry, MessageEntryType, NonEmptyString, SessionEntry, SessionEntryType,
    };
    let mut entries = Vec::new();
    let capacity = loaded
        .messages()
        .len()
        .checked_add(1)
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, &paths.messages))?;
    entries
        .try_reserve_exact(capacity)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, &paths.messages))?;
    let conversation = loaded.conversation();
    entries.push(TranscriptEntry::Session(SessionEntry {
        entry_type: SessionEntryType::Session,
        version: 3,
        id: NonEmptyString::new(conversation.id.as_str().to_owned())
            .map_err(|_| StoreError::new(StoreErrorKind::Parse, &paths.conversation))?,
        timestamp: conversation.created_at,
        cwd: String::new(),
    }));
    let mut parent = None;
    for (index, message) in loaded.messages().iter().enumerate() {
        let id = format!("lotta-upgrade-{index}");
        entries.push(TranscriptEntry::Message(MessageEntry {
            entry_type: MessageEntryType::Message,
            id: NonEmptyString::new(id.clone())
                .map_err(|_| StoreError::new(StoreErrorKind::Parse, &paths.messages))?,
            parent_id: parent,
            timestamp: conversation.created_at,
            message: message.clone(),
        }));
        parent = Some(id);
    }
    super::session_header::validate_sequence(&entries, &paths.messages)?;
    Ok(entries)
}

pub(crate) fn copy_backup_durable(
    paths: &TranscriptPaths,
    expected: &FileRevision,
) -> Result<PathBuf, StoreError> {
    copy_backup_durable_observed(paths, expected, &NoopBackupObserver)
}

pub(crate) fn copy_backup_durable_observed(
    paths: &TranscriptPaths,
    expected: &FileRevision,
    observer: &dyn BackupObserver,
) -> Result<PathBuf, StoreError> {
    for _ in 0..BACKUP_CREATE_RETRIES_MAX {
        let backup = backup_candidate(paths)?;
        match create_backup(paths, &backup, expected, observer) {
            Err(error)
                if error.kind() == StoreErrorKind::StorageConflict && error.path() == backup => {}
            Err(error) => return Err(error),
            Ok(()) => {
                enforce_backup_limit(paths)?;
                return Ok(backup);
            }
        }
    }
    Err(StoreError::new(StoreErrorKind::Limit, &paths.messages))
}

fn backup_candidate(paths: &TranscriptPaths) -> Result<PathBuf, StoreError> {
    let sequence = next_sequence(&BACKUP_SEQUENCE, &paths.messages)?;
    let timestamp = persisted_timestamp_component(paths)?;
    Ok(paths.directory.join(format!(
        "messages.jsonl.lotta-upgrade-{timestamp}-{sequence}.bak"
    )))
}

fn persisted_timestamp_component(paths: &TranscriptPaths) -> Result<String, StoreError> {
    let created_at = match std::fs::symlink_metadata(&paths.manifest) {
        Ok(_) => manifest::read(&paths.manifest)?.created_at,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let conversation: lotta_domain::Conversation =
                crate::adapter::read_record(&paths.conversation)?;
            conversation.created_at
        }
        Err(error) => return Err(StoreError::from_io(&paths.manifest, &error)),
    };
    Ok(created_at.as_utc().format("%Y%m%d-%H%M%S").to_string())
}

fn create_backup(
    paths: &TranscriptPaths,
    backup: &Path,
    expected: &FileRevision,
    observer: &dyn BackupObserver,
) -> Result<(), StoreError> {
    validate_existing(&paths.root, &paths.directory)?;
    validate_regular_file(&paths.root, &paths.messages)?;
    let before = FileRevision::sample_path_bounded(&paths.messages, super::TRANSCRIPT_BYTES_MAX)?;
    if &before != expected {
        return Err(StoreError::new(
            StoreErrorKind::StorageConflict,
            &paths.messages,
        ));
    }
    let mut source = File::open(&paths.messages)
        .map_err(|error| StoreError::from_io(&paths.messages, &error))?;
    let mut target = match OpenOptions::new().create_new(true).write(true).open(backup) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(StoreError::new(StoreErrorKind::StorageConflict, backup));
        }
        Err(error) => return Err(StoreError::from_io(backup, &error)),
    };
    let result = copy_and_verify(paths, backup, &mut source, &mut target, expected, observer);
    drop(target);
    if result.is_err() {
        cleanup_backup(backup);
    }
    result
}

fn copy_and_verify(
    paths: &TranscriptPaths,
    backup: &Path,
    source: &mut File,
    target: &mut File,
    expected: &FileRevision,
    observer: &dyn BackupObserver,
) -> Result<(), StoreError> {
    let copied = copy_bounded(source, target, backup)?;
    if copied != expected.length() {
        return Err(StoreError::new(
            StoreErrorKind::StorageConflict,
            &paths.messages,
        ));
    }
    target
        .sync_all()
        .map_err(|error| StoreError::from_io(backup, &error))?;
    observer.file_flushed(backup);
    let after = FileRevision::sample_path_bounded(&paths.messages, super::TRANSCRIPT_BYTES_MAX)?;
    let proof = FileRevision::sample_path_bounded(backup, super::TRANSCRIPT_BYTES_MAX)?;
    if &after != expected || !proof.same_contents(expected) {
        return Err(StoreError::new(
            StoreErrorKind::StorageConflict,
            &paths.messages,
        ));
    }
    File::open(&paths.directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| StoreError::from_io(&paths.directory, &error))?;
    observer.parent_flushed(&paths.directory);
    Ok(())
}

fn copy_bounded(source: &mut File, target: &mut File, path: &Path) -> Result<u64, StoreError> {
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(COPY_BUFFER_BYTES)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    buffer.resize(COPY_BUFFER_BYTES, 0);
    let mut copied = 0_u64;
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| StoreError::from_io(path, &error))?;
        if read == 0 {
            return Ok(copied);
        }
        copied = copied
            .checked_add(
                u64::try_from(read).map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?,
            )
            .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, path))?;
        if copied > super::TRANSCRIPT_BYTES_MAX {
            return Err(StoreError::new(StoreErrorKind::Limit, path));
        }
        target
            .write_all(&buffer[..read])
            .map_err(|error| StoreError::from_io(path, &error))?;
    }
}

fn enforce_backup_limit(paths: &TranscriptPaths) -> Result<(), StoreError> {
    let mut backups = completed_backups(paths)?;
    backups.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    while backups.len() > MIGRATION_BACKUPS_PER_FILE_MAX {
        let (_, oldest) = backups.remove(0);
        validate_regular_file(&paths.root, &oldest)?;
        std::fs::remove_file(&oldest).map_err(|error| StoreError::from_io(&oldest, &error))?;
        sync_backup_parent(paths)?;
    }
    Ok(())
}

fn completed_backups(
    paths: &TranscriptPaths,
) -> Result<Vec<(std::time::SystemTime, PathBuf)>, StoreError> {
    let mut backups = Vec::new();
    for entry in std::fs::read_dir(&paths.directory)
        .map_err(|error| StoreError::from_io(&paths.directory, &error))?
    {
        let entry = entry.map_err(|error| StoreError::from_io(&paths.directory, &error))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let matching = name.starts_with("messages.jsonl.lotta-upgrade-")
            && Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("bak"));
        let regular = entry
            .file_type()
            .map_err(|error| StoreError::from_io(&entry.path(), &error))?
            .is_file();
        if matching && regular {
            let path = entry.path();
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map_err(|error| StoreError::from_io(&path, &error))?;
            backups
                .try_reserve(1)
                .map_err(|_| StoreError::new(StoreErrorKind::Limit, &paths.directory))?;
            backups.push((modified, path));
        }
    }
    Ok(backups)
}

fn sync_backup_parent(paths: &TranscriptPaths) -> Result<(), StoreError> {
    File::open(&paths.directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| StoreError::from_io(&paths.directory, &error))
}

fn upgraded_manifest(
    source: &TranscriptManifest,
    backup: &Path,
) -> Result<TranscriptManifest, StoreError> {
    let backup_name = backup
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| StoreError::new(StoreErrorKind::InvalidPath, backup))?;
    Ok(TranscriptManifest {
        schema_version: manifest::TRANSCRIPT_SCHEMA_VERSION,
        message_format: TranscriptMessageFormat::PiSessionEntryJsonl,
        provider_stack: source.provider_stack,
        created_at: source.created_at,
        migrated_from: Some("versioned-pi-ai-message-jsonl".into()),
        migrated_at: None,
        backup_path: Some(backup_name.to_owned()),
    })
}

fn cleanup_backup(path: &Path) {
    if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file()) {
        let _ignored = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::test_support::setup;

    #[test]
    fn rotates_oldest_completed_backup_at_three() {
        let (_root, store, agent, conversation) = setup("task25-backup-rotation");
        let paths = crate::transcript::transcript_paths(store.paths(), &agent, &conversation)
            .expect("paths");
        std::fs::create_dir_all(&paths.directory).expect("directory");
        let mut backups = Vec::new();
        for sequence in 1..=MIGRATION_BACKUPS_PER_FILE_MAX {
            let path = paths.directory.join(format!(
                "messages.jsonl.lotta-upgrade-20000101-000000-{sequence}.bak"
            ));
            std::fs::write(&path, format!("backup-{sequence}")).expect("backup");
            backups.push(path);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let extra = paths
            .directory
            .join("messages.jsonl.lotta-upgrade-20000101-000000-4.bak");
        std::fs::write(&extra, "backup-4").expect("extra backup");
        enforce_backup_limit(&paths).expect("rotation");
        assert!(!backups[0].exists());
        assert!(backups[1].exists());
        assert!(backups[2].exists());
        assert!(extra.exists());
        assert_eq!(completed_backups(&paths).expect("completed").len(), 3);
    }
}

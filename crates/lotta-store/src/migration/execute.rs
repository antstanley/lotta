use super::{MigrationDisposition, MigrationItem, Plan, Revisions};
use crate::atomic::{
    FileRevision, atomic_delete_expected_locked_observed, atomic_write_expected_locked_observed,
};
use crate::transcript::{self, TranscriptRewriteObserver};
use crate::{AtomicObserver, LottaStorageLock, StoreError, StoreErrorKind, WriteMode};
use std::path::Path;

pub(crate) trait MigrationObserver:
    AtomicObserver + TranscriptRewriteObserver + transcript::upgrade::BackupObserver
{
    fn atomic(&self) -> &dyn AtomicObserver;
    fn transcript(&self) -> &dyn TranscriptRewriteObserver;
    fn backup(&self) -> &dyn transcript::upgrade::BackupObserver;
}

pub(super) struct NoopMigrationObserver;
impl AtomicObserver for NoopMigrationObserver {}
impl TranscriptRewriteObserver for NoopMigrationObserver {}
impl transcript::upgrade::BackupObserver for NoopMigrationObserver {}
impl MigrationObserver for NoopMigrationObserver {
    fn atomic(&self) -> &dyn AtomicObserver {
        self
    }

    fn transcript(&self) -> &dyn TranscriptRewriteObserver {
        self
    }

    fn backup(&self) -> &dyn transcript::upgrade::BackupObserver {
        self
    }
}

pub(super) fn execute(
    plan: &Plan,
    lock: &LottaStorageLock,
    observer: &dyn MigrationObserver,
) -> Result<MigrationItem, StoreError> {
    verify_revisions(plan)?;
    if plan.disposition != MigrationDisposition::Converted {
        return item(plan, None);
    }
    let staged = stage_outputs(plan)?;
    let backup = transcript::upgrade::copy_backup_durable_observed(
        &plan.paths,
        &plan.revisions.messages,
        observer.backup(),
    )?;
    let manifest_bytes = manifest_bytes(plan, &backup)?;
    let desired_manifest = desired_revision(&manifest_bytes, &plan.paths.manifest)?;
    rewrite_transcript(plan, &backup, lock, observer)?;
    let converted = revision_messages(plan)?;
    if let Err(error) = write_conversation(plan, lock, observer) {
        rollback_conversation_failure(plan, &backup, &converted, &staged.conversation, lock)?;
        return Err(error);
    }
    let conversation_after = FileRevision::sample_path(&plan.paths.conversation)?;
    if let Err(error) = write_manifest(plan, &manifest_bytes, lock, observer) {
        let context = ManifestRollback {
            backup: &backup,
            converted: &converted,
            conversation_after: &conversation_after,
            desired_manifest: &desired_manifest,
            error,
        };
        return rollback_manifest_failure(plan, context, lock);
    }
    item(plan, Some(backup))
}

struct StagedOutputs {
    conversation: FileRevision,
}

fn stage_outputs(plan: &Plan) -> Result<StagedOutputs, StoreError> {
    let mut size = 0_u64;
    for entry in &plan.entries {
        let line = transcript::encode_line(entry, &plan.paths.messages)?;
        let added = u64::try_from(line.len())
            .map_err(|_| StoreError::new(StoreErrorKind::Limit, &plan.paths.messages))?;
        size = transcript::final_size(size, added, &plan.paths.messages)?;
    }
    let conversation = desired_revision(&plan.conversation_bytes, &plan.paths.conversation)?;
    transcript::manifest::encode(&plan.manifest, &plan.paths.manifest)?;
    Ok(StagedOutputs { conversation })
}

fn manifest_bytes(plan: &Plan, backup: &Path) -> Result<Vec<u8>, StoreError> {
    let mut manifest = plan.manifest.clone();
    manifest.backup_path = backup
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned);
    transcript::manifest::encode(&manifest, &plan.paths.manifest)
}

fn rewrite_transcript(
    plan: &Plan,
    backup: &Path,
    lock: &LottaStorageLock,
    observer: &dyn MigrationObserver,
) -> Result<(), StoreError> {
    let (result, committed) = transcript::rewrite_migration_held_tracked(
        &plan.paths,
        &plan.entries,
        backup,
        &plan.revisions.messages,
        lock,
        observer.transcript(),
    );
    match result {
        Ok(()) => Ok(()),
        Err(error) if !committed => Err(error),
        Err(error) => {
            let converted = revision_messages(plan)?;
            restore_transcript(plan, backup, &converted, lock)?;
            Err(error)
        }
    }
}

fn write_conversation(
    plan: &Plan,
    lock: &LottaStorageLock,
    observer: &dyn MigrationObserver,
) -> Result<(), StoreError> {
    atomic_write_expected_locked_observed(
        &plan.paths.conversation,
        &plan.conversation_bytes,
        WriteMode::Standard,
        &plan.revisions.conversation,
        lock,
        observer.atomic(),
    )
}

fn write_manifest(
    plan: &Plan,
    bytes: &[u8],
    lock: &LottaStorageLock,
    observer: &dyn MigrationObserver,
) -> Result<(), StoreError> {
    atomic_write_expected_locked_observed(
        &plan.paths.manifest,
        bytes,
        WriteMode::Standard,
        &plan.revisions.manifest,
        lock,
        observer.atomic(),
    )
}

struct ManifestRollback<'a> {
    backup: &'a Path,
    converted: &'a FileRevision,
    conversation_after: &'a FileRevision,
    desired_manifest: &'a FileRevision,
    error: StoreError,
}

fn rollback_manifest_failure(
    plan: &Plan,
    context: ManifestRollback<'_>,
    lock: &LottaStorageLock,
) -> Result<MigrationItem, StoreError> {
    let active = sample_active(plan)?;
    if active.manifest.same_contents(context.desired_manifest) {
        if active.messages.same_contents(context.converted)
            && active
                .conversation
                .same_contents(context.conversation_after)
        {
            return Err(context.error);
        }
        return Err(conflict(&plan.paths.directory));
    }
    preflight_rollback(plan, &active, &context)?;
    let noop = NoopMigrationObserver;
    restore_manifest(plan, &active.manifest, lock, &noop)?;
    restore_conversation(plan, &active.conversation, lock, &noop)?;
    restore_transcript(plan, context.backup, &active.messages, lock)?;
    Err(context.error)
}

fn rollback_conversation_failure(
    plan: &Plan,
    backup: &Path,
    converted: &FileRevision,
    desired: &FileRevision,
    lock: &LottaStorageLock,
) -> Result<(), StoreError> {
    let active = sample_active(plan)?;
    let conversation_owned = active.conversation == plan.revisions.conversation
        || active.conversation.same_contents(desired);
    let transcript_owned = active.messages.same_contents(converted);
    let manifest_original = active.manifest == plan.revisions.manifest;
    if !conversation_owned || !transcript_owned || !manifest_original {
        return Err(conflict(&plan.paths.directory));
    }
    if active.conversation.same_contents(desired) {
        restore_conversation(plan, &active.conversation, lock, &NoopMigrationObserver)?;
    }
    restore_transcript(plan, backup, &active.messages, lock)
}

struct ActiveRevisions {
    messages: FileRevision,
    conversation: FileRevision,
    manifest: FileRevision,
}

fn sample_active(plan: &Plan) -> Result<ActiveRevisions, StoreError> {
    Ok(ActiveRevisions {
        messages: revision_messages(plan)?,
        conversation: FileRevision::sample_path(&plan.paths.conversation)?,
        manifest: FileRevision::sample_path(&plan.paths.manifest)?,
    })
}

fn preflight_rollback(
    plan: &Plan,
    active: &ActiveRevisions,
    context: &ManifestRollback<'_>,
) -> Result<(), StoreError> {
    let transcript_owned = active.messages.same_contents(context.converted);
    let conversation_owned = active
        .conversation
        .same_contents(context.conversation_after);
    let manifest_owned = active.manifest == plan.revisions.manifest;
    if transcript_owned && conversation_owned && manifest_owned {
        Ok(())
    } else {
        Err(conflict(&plan.paths.directory))
    }
}

fn conflict(path: &Path) -> StoreError {
    StoreError::new(StoreErrorKind::StorageConflict, path)
}

pub(super) fn item(
    plan: &Plan,
    backup: Option<std::path::PathBuf>,
) -> Result<MigrationItem, StoreError> {
    let message_count = plan
        .entries
        .len()
        .checked_sub(1)
        .ok_or_else(|| StoreError::new(StoreErrorKind::Parse, &plan.paths.messages))?;
    Ok(MigrationItem {
        conversation_dir: plan.paths.directory.clone(),
        disposition: plan.disposition.clone(),
        message_count,
        backup_path: backup,
    })
}

fn restore_manifest(
    plan: &Plan,
    expected: &FileRevision,
    lock: &LottaStorageLock,
    observer: &dyn MigrationObserver,
) -> Result<(), StoreError> {
    match &plan.original_manifest_bytes {
        Some(bytes) => atomic_write_expected_locked_observed(
            &plan.paths.manifest,
            bytes,
            WriteMode::Standard,
            expected,
            lock,
            observer.atomic(),
        ),
        None if expected.exists() => atomic_delete_expected_locked_observed(
            &plan.paths.manifest,
            expected,
            lock,
            observer.atomic(),
        ),
        None => Ok(()),
    }
}

fn restore_conversation(
    plan: &Plan,
    expected: &FileRevision,
    lock: &LottaStorageLock,
    observer: &dyn MigrationObserver,
) -> Result<(), StoreError> {
    atomic_write_expected_locked_observed(
        &plan.paths.conversation,
        &plan.original_conversation_bytes,
        WriteMode::Standard,
        expected,
        lock,
        observer.atomic(),
    )
}

fn restore_transcript(
    plan: &Plan,
    backup: &Path,
    expected: &FileRevision,
    lock: &LottaStorageLock,
) -> Result<(), StoreError> {
    match transcript::restore_migration_backup_held(&plan.paths, backup, expected, lock) {
        Ok(()) => Ok(()),
        Err(error) => {
            let active = revision_messages(plan)?;
            let recovery =
                FileRevision::sample_path_bounded(backup, transcript::TRANSCRIPT_BYTES_MAX)?;
            if active.same_contents(&recovery) {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

fn revision_messages(plan: &Plan) -> Result<FileRevision, StoreError> {
    FileRevision::sample_path_bounded(&plan.paths.messages, transcript::TRANSCRIPT_BYTES_MAX)
}

fn desired_revision(bytes: &[u8], path: &Path) -> Result<FileRevision, StoreError> {
    use sha2::{Digest as _, Sha256};
    let length =
        u64::try_from(bytes.len()).map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    Ok(FileRevision::from_contents(
        length,
        Sha256::digest(bytes).into(),
    ))
}

fn verify_revisions(plan: &Plan) -> Result<(), StoreError> {
    let actual = Revisions {
        messages: revision_messages(plan)?,
        conversation: FileRevision::sample_path(&plan.paths.conversation)?,
        manifest: FileRevision::sample_path(&plan.paths.manifest)?,
    };
    if actual.messages != plan.revisions.messages
        || actual.conversation != plan.revisions.conversation
        || actual.manifest != plan.revisions.manifest
    {
        return Err(StoreError::new(
            StoreErrorKind::StorageConflict,
            &plan.paths.directory,
        ));
    }
    Ok(())
}

//! Bounded append-only schema-v2 transcript persistence.
//!
//! Lotta's root lock serializes participating Rust writers. TypeScript does not acquire it, so
//! concurrent mixed-runtime writes remain unsupported. A crash may leave an append tail; tolerant
//! prefix recovery belongs to Task 25.

pub(crate) mod append;
pub mod bounds;
pub mod load;
pub mod manifest;
pub(crate) mod projection;
mod repair;
pub(crate) mod session_header;
#[cfg(test)]
pub(crate) mod task25_test_support;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod upgrade;

use crate::adapter::run_blocking;
use crate::atomic::{ATOMIC_WRITE_RETRIES_MAX, FileRevision, atomic_write_locked, next_sequence};
use crate::confinement::validate_regular_file;
use crate::confinement::{backend_root, validate_existing};
use crate::{LocalStore, LottaStorageLock, StoreError, StoreErrorKind, WriteMode};
use lotta_domain::{AgentId, ConversationId, TranscriptEntry, TranscriptManifest};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

static REWRITE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const RESTORE_BUFFER_BYTES: usize = 64 * 1_024;

pub(crate) trait TranscriptRewriteObserver: Send + Sync {
    fn before_staged_write(&self, _target: &Path) -> Result<(), StoreError> {
        Ok(())
    }

    fn before_stage_sync(&self, _target: &Path) -> Result<(), StoreError> {
        Ok(())
    }

    fn before_compare(&self, _target: &Path, _temp: &Path) -> Result<(), StoreError> {
        Ok(())
    }

    fn before_rename(&self, _target: &Path) -> Result<(), StoreError> {
        Ok(())
    }

    fn renamed(&self, _target: &Path) {}

    fn before_parent_sync(&self, _parent: &Path, _temp: &Path) -> Result<(), StoreError> {
        Ok(())
    }

    fn parent_flushed(&self, _parent: &Path) {}
}

pub(crate) struct NoopRewriteObserver;
impl TranscriptRewriteObserver for NoopRewriteObserver {}

struct TrackingRewriteObserver<'a> {
    inner: &'a dyn TranscriptRewriteObserver,
    committed: std::sync::atomic::AtomicBool,
}

impl TranscriptRewriteObserver for TrackingRewriteObserver<'_> {
    fn before_staged_write(&self, target: &Path) -> Result<(), StoreError> {
        self.inner.before_staged_write(target)
    }

    fn before_stage_sync(&self, target: &Path) -> Result<(), StoreError> {
        self.inner.before_stage_sync(target)
    }

    fn before_compare(&self, target: &Path, temp: &Path) -> Result<(), StoreError> {
        self.inner.before_compare(target, temp)
    }

    fn before_rename(&self, target: &Path) -> Result<(), StoreError> {
        self.inner.before_rename(target)
    }

    fn renamed(&self, target: &Path) {
        self.committed
            .store(true, std::sync::atomic::Ordering::Release);
        self.inner.renamed(target);
    }

    fn before_parent_sync(&self, parent: &Path, temp: &Path) -> Result<(), StoreError> {
        self.inner.before_parent_sync(parent, temp)
    }

    fn parent_flushed(&self, parent: &Path) {
        self.inner.parent_flushed(parent);
    }
}

pub use bounds::{TRANSCRIPT_BYTES_MAX, TRANSCRIPT_LINE_BYTES_MAX};
pub(crate) use bounds::{encode_line, final_size};

impl LocalStore {
    /// Reads and validates one transcript manifest at its canonical scoped path.
    ///
    /// # Errors
    /// Returns typed identity, confinement, bound, parse, or unsupported-format failures.
    pub async fn read_transcript_manifest(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
    ) -> Result<TranscriptManifest, StoreError> {
        let path = transcript_paths(self.paths(), agent, conversation)?.manifest;
        run_blocking(self.blocking_pool(), move || manifest::read(&path)).await
    }

    /// Loads one canonical scoped transcript and repairs only its conversation projection.
    ///
    /// All filesystem work runs under the shared bounded blocking semaphore. Load accepts the
    /// baseline's graph tolerances and never mutates transcript or manifest source bytes.
    ///
    /// # Errors
    /// Returns typed confinement, manifest, migration, repair, parse, bound, lock, or conflict
    /// failures.
    pub async fn load_transcript(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
    ) -> Result<load::LoadedTranscript, StoreError> {
        let paths = transcript_paths(self.paths(), agent, conversation)?;
        run_blocking(self.blocking_pool(), move || load::load(&paths)).await
    }

    #[cfg(test)]
    pub(crate) async fn load_transcript_observed(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        observer: impl FnOnce(&Path) -> Result<(), StoreError> + Send + 'static,
    ) -> Result<load::LoadedTranscript, StoreError> {
        let paths = transcript_paths(self.paths(), agent, conversation)?;
        run_blocking(self.blocking_pool(), move || {
            load::load_observed(&paths, observer)
        })
        .await
    }

    /// Creates a manifest and exactly one version-three session row.
    ///
    /// The prepared row is published first and the manifest is the visibility marker. Both atomic
    /// writes occur while one root lock is held.
    ///
    /// # Errors
    /// Returns typed validation, conflict, confinement, lock, bound, or durability failures.
    pub async fn initialize_transcript(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        manifest: &TranscriptManifest,
        session: &TranscriptEntry,
    ) -> Result<(), StoreError> {
        let paths = transcript_paths(self.paths(), agent, conversation)?;
        let manifest_bytes = manifest::encode(manifest, &paths.manifest)?;
        let session_bytes = session_header::encode(session, &paths.messages)?;
        run_blocking(self.blocking_pool(), move || {
            initialize_locked(&paths, &manifest_bytes, &session_bytes)
        })
        .await
    }

    /// Appends exactly one durable message or compaction JSON object and LF.
    ///
    /// # Errors
    /// Returns typed validation, confinement, manifest, lock, size, or I/O failures.
    pub async fn append_transcript_entry(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        entry: &TranscriptEntry,
    ) -> Result<(), StoreError> {
        let paths = transcript_paths(self.paths(), agent, conversation)?;
        let line = encode_append_entry(entry, &paths.messages)?;
        run_blocking(self.blocking_pool(), move || append::append(&paths, &line)).await
    }

    /// Appends a transcript entry once by stable entry ID.
    ///
    /// A retry after an uncertain append returns success when the exact ID already exists.
    ///
    /// # Errors
    /// Returns typed load, validation, confinement, lock, size, or I/O failures.
    pub async fn append_transcript_entry_idempotent(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        entry: &TranscriptEntry,
    ) -> Result<(), StoreError> {
        let id = match entry {
            TranscriptEntry::Session(value) => value.id.as_str(),
            TranscriptEntry::Message(value) => value.id.as_str(),
            TranscriptEntry::Compaction(value) => value.id.as_str(),
        };
        let paths = transcript_paths(self.paths(), agent, conversation)?;
        let id = id.to_owned();
        let exists = run_blocking(self.blocking_pool(), move || {
            let rows = load::load(&paths)?;
            Ok(rows
                .messages()
                .iter()
                .any(|message| message.id.as_str() == id)
                || rows
                    .conversation()
                    .in_context_message_ids
                    .as_slice()
                    .iter()
                    .any(|value| value.as_str() == id))
        })
        .await?;
        if exists {
            Ok(())
        } else {
            self.append_transcript_entry(agent, conversation, entry)
                .await
        }
    }

    /// Persists a loaded projection, upgrading a non-empty legacy transcript on demand.
    ///
    /// Empty persistence is a no-op. A legacy non-empty persistence takes a durable confined
    /// backup before atomically rewriting messages and publishing the current manifest.
    ///
    /// # Errors
    /// Returns typed validation, confinement, lock, bound, conflict, backup, or durability errors.
    pub async fn persist_loaded_projection(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        loaded: load::LoadedTranscript,
    ) -> Result<(), StoreError> {
        let paths = transcript_paths(self.paths(), agent, conversation)?;
        run_blocking(self.blocking_pool(), move || {
            upgrade::persist_projection(&paths, &loaded)
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn persist_loaded_projection_observed(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        loaded: load::LoadedTranscript,
        observer: impl FnOnce(&Path) -> Result<(), StoreError> + Send + 'static,
    ) -> Result<(), StoreError> {
        let paths = transcript_paths(self.paths(), agent, conversation)?;
        run_blocking(self.blocking_pool(), move || {
            upgrade::persist_projection_observed(&paths, &loaded, observer)
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn persist_loaded_projection_rewrite_observed<O>(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        loaded: load::LoadedTranscript,
        observer: O,
    ) -> Result<(), StoreError>
    where
        O: TranscriptRewriteObserver + 'static,
    {
        let paths = transcript_paths(self.paths(), agent, conversation)?;
        run_blocking(self.blocking_pool(), move || {
            upgrade::persist_projection_rewrite_observed(&paths, &loaded, &observer)
        })
        .await
    }

    /// Replaces a loaded conversation transcript during explicit initial/full persistence.
    ///
    /// # Errors
    /// Returns typed validation, confinement, lock, bound, conflict, or durability failures.
    pub async fn persist_loaded_transcript(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        entries: Vec<TranscriptEntry>,
    ) -> Result<(), StoreError> {
        self.rewrite_for_authority(agent, conversation, entries, RewriteAuthority::Loaded)
            .await
    }

    /// Replaces a fork target transcript with its inherited canonical history.
    ///
    /// # Errors
    /// Returns typed validation, confinement, lock, bound, conflict, or durability failures.
    pub async fn persist_fork_transcript(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        entries: Vec<TranscriptEntry>,
    ) -> Result<(), StoreError> {
        self.rewrite_for_authority(agent, conversation, entries, RewriteAuthority::Fork)
            .await
    }

    /// Replaces a transcript only after an existing regular backup proof is supplied.
    ///
    /// # Errors
    /// Returns typed validation, confinement, backup, lock, bound, or durability failures.
    pub async fn persist_migrated_transcript(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        entries: Vec<TranscriptEntry>,
        backup: PathBuf,
    ) -> Result<(), StoreError> {
        self.rewrite_for_authority(
            agent,
            conversation,
            entries,
            RewriteAuthority::Migration(backup),
        )
        .await
    }

    async fn rewrite_for_authority(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        entries: Vec<TranscriptEntry>,
        authority: RewriteAuthority,
    ) -> Result<(), StoreError> {
        let paths = transcript_paths(self.paths(), agent, conversation)?;
        run_blocking(self.blocking_pool(), move || {
            rewrite_locked(&paths, &entries, &authority)
        })
        .await
    }
}

pub(crate) enum RewriteAuthority {
    Migration(PathBuf),
    Fork,
    Loaded,
}

pub(crate) struct TranscriptPaths {
    pub(crate) root: PathBuf,
    pub(crate) directory: PathBuf,
    pub(crate) conversation: PathBuf,
    pub(crate) manifest: PathBuf,
    pub(crate) messages: PathBuf,
}

pub(crate) fn transcript_paths(
    paths: &crate::StorePaths,
    agent: &AgentId,
    conversation: &ConversationId,
) -> Result<TranscriptPaths, StoreError> {
    let directory = paths.conversation_dir(agent, conversation)?;
    Ok(TranscriptPaths {
        root: paths.root().to_path_buf(),
        conversation: directory.join("conversation.json"),
        manifest: directory.join("manifest.json"),
        messages: directory.join("messages.jsonl"),
        directory,
    })
}

fn initialize_locked(
    paths: &TranscriptPaths,
    manifest_bytes: &[u8],
    session_bytes: &[u8],
) -> Result<(), StoreError> {
    let lock = LottaStorageLock::try_acquire_confined(&paths.root)?;
    validate_existing(&paths.root, &paths.directory)?;
    ensure_absent(&paths.manifest)?;
    ensure_absent(&paths.messages)?;
    atomic_write_locked(&paths.messages, session_bytes, WriteMode::Standard, &lock)?;
    atomic_write_locked(&paths.manifest, manifest_bytes, WriteMode::Standard, &lock)
}

fn ensure_absent(path: &Path) -> Result<(), StoreError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::from_io(path, &error)),
        Ok(_) => Err(StoreError::new(StoreErrorKind::StorageConflict, path)),
    }
}

pub(crate) fn encode_append_entry(
    entry: &TranscriptEntry,
    path: &Path,
) -> Result<Vec<u8>, StoreError> {
    if matches!(entry, TranscriptEntry::Session(_)) {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    encode_line(entry, path)
}

pub(crate) fn rewrite_locked(
    paths: &TranscriptPaths,
    entries: &[TranscriptEntry],
    authority: &RewriteAuthority,
) -> Result<(), StoreError> {
    let root = backend_root(&paths.messages)?;
    let lock = LottaStorageLock::try_acquire_confined(root)?;
    manifest::read_current(&paths.manifest)?;
    if let RewriteAuthority::Migration(backup) = authority {
        validate_migration_backup(paths, backup)?;
    }
    let expected = FileRevision::sample_path_bounded(&paths.messages, TRANSCRIPT_BYTES_MAX)?;
    rewrite_held(paths, entries, authority, &expected, &lock)
}

pub(crate) fn rewrite_held(
    paths: &TranscriptPaths,
    entries: &[TranscriptEntry],
    authority: &RewriteAuthority,
    expected: &FileRevision,
    lock: &LottaStorageLock,
) -> Result<(), StoreError> {
    rewrite_held_observed(
        paths,
        entries,
        authority,
        expected,
        lock,
        &NoopRewriteObserver,
    )
}

fn rewrite_held_observed(
    paths: &TranscriptPaths,
    entries: &[TranscriptEntry],
    authority: &RewriteAuthority,
    expected: &FileRevision,
    lock: &LottaStorageLock,
    observer: &dyn TranscriptRewriteObserver,
) -> Result<(), StoreError> {
    if let RewriteAuthority::Migration(backup) = authority {
        validate_migration_backup(paths, backup)?;
    }
    rewrite_streamed(&paths.messages, entries, expected, lock, observer)
}

pub(crate) fn rewrite_migration_held_tracked(
    paths: &TranscriptPaths,
    entries: &[TranscriptEntry],
    backup: &Path,
    expected: &FileRevision,
    lock: &LottaStorageLock,
    observer: &dyn TranscriptRewriteObserver,
) -> (Result<(), StoreError>, bool) {
    let authority = RewriteAuthority::Migration(backup.to_path_buf());
    let tracking = TrackingRewriteObserver {
        inner: observer,
        committed: std::sync::atomic::AtomicBool::new(false),
    };
    let result = rewrite_held_observed(paths, entries, &authority, expected, lock, &tracking);
    let committed = tracking
        .committed
        .load(std::sync::atomic::Ordering::Acquire);
    (result, committed)
}

pub(crate) fn restore_migration_backup_held(
    paths: &TranscriptPaths,
    backup: &Path,
    expected: &FileRevision,
    lock: &LottaStorageLock,
) -> Result<(), StoreError> {
    validate_migration_backup(paths, backup)?;
    if !lock.guards_root(&paths.root) {
        return Err(StoreError::new(StoreErrorKind::LottaLock, &paths.messages));
    }
    let backup_revision = FileRevision::sample_path_bounded(backup, TRANSCRIPT_BYTES_MAX)?;
    let (temp, mut target) = create_rewrite_temp(&paths.root, &paths.directory, &paths.messages)?;
    let result = restore_backup_precommit(paths, backup, &temp, &mut target, expected);
    drop(target);
    if let Err(error) = result {
        let _cleanup = std::fs::remove_file(&temp);
        return Err(error);
    }
    let backup_after = match FileRevision::sample_path_bounded(backup, TRANSCRIPT_BYTES_MAX) {
        Ok(revision) => revision,
        Err(error) => {
            let _cleanup = std::fs::remove_file(&temp);
            return Err(error);
        }
    };
    if backup_after != backup_revision {
        let _cleanup = std::fs::remove_file(&temp);
        return Err(StoreError::new(StoreErrorKind::StorageConflict, backup));
    }
    if let Err(error) = std::fs::rename(&temp, &paths.messages) {
        let _cleanup = std::fs::remove_file(&temp);
        return Err(StoreError::from_io(&paths.messages, &error));
    }
    flush_rewrite_parent(&paths.directory, &temp, &NoopRewriteObserver)
}

fn restore_backup_precommit(
    paths: &TranscriptPaths,
    backup: &Path,
    temp: &Path,
    target: &mut File,
    expected: &FileRevision,
) -> Result<(), StoreError> {
    let mut source = File::open(backup).map_err(|error| StoreError::from_io(backup, &error))?;
    copy_backup_bytes(&mut source, target, &paths.messages)?;
    target
        .sync_all()
        .map_err(|error| StoreError::from_io(temp, &error))?;
    validate_rewrite_target(&paths.root, &paths.messages)?;
    let actual = FileRevision::sample_path_bounded(&paths.messages, TRANSCRIPT_BYTES_MAX)?;
    if &actual != expected {
        return Err(StoreError::new(
            StoreErrorKind::StorageConflict,
            &paths.messages,
        ));
    }
    Ok(())
}

fn copy_backup_bytes(source: &mut File, target: &mut File, path: &Path) -> Result<(), StoreError> {
    use std::io::{Read as _, Write as _};
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(RESTORE_BUFFER_BYTES)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    buffer.resize(RESTORE_BUFFER_BYTES, 0);
    let mut total = 0_u64;
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| StoreError::from_io(path, &error))?;
        if read == 0 {
            return Ok(());
        }
        let added =
            u64::try_from(read).map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
        total = final_size(total, added, path)?;
        target
            .write_all(&buffer[..read])
            .map_err(|error| StoreError::from_io(path, &error))?;
    }
}

fn rewrite_streamed(
    path: &Path,
    entries: &[TranscriptEntry],
    expected: &FileRevision,
    lock: &LottaStorageLock,
    observer: &dyn TranscriptRewriteObserver,
) -> Result<(), StoreError> {
    session_header::validate_sequence(entries, path)?;
    let root = backend_root(path)?;
    if !lock.guards_root(root) {
        return Err(StoreError::new(StoreErrorKind::LottaLock, path));
    }
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::new(StoreErrorKind::InvalidPath, path))?;
    validate_existing(root, parent)?;
    validate_rewrite_target(root, path)?;
    let (temp, file) = create_rewrite_temp(root, parent, path)?;
    let context = RewriteContext {
        root,
        path,
        parent,
        temp: &temp,
        observer,
    };
    let result = rewrite_precommit(&context, file, entries, expected);
    match result {
        Ok(()) => flush_rewrite_parent(parent, &temp, observer),
        Err(error) => {
            let _cleanup = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

struct RewriteContext<'a> {
    root: &'a Path,
    path: &'a Path,
    parent: &'a Path,
    temp: &'a Path,
    observer: &'a dyn TranscriptRewriteObserver,
}

fn rewrite_precommit(
    context: &RewriteContext<'_>,
    mut file: File,
    entries: &[TranscriptEntry],
    expected: &FileRevision,
) -> Result<(), StoreError> {
    let RewriteContext {
        root,
        path,
        parent,
        temp,
        observer,
    } = context;
    write_staged(&mut file, entries, path, *observer)?;
    drop(file);
    validate_existing(root, parent)?;
    validate_rewrite_target(root, path)?;
    observer.before_compare(path, temp)?;
    let actual = FileRevision::sample_path_bounded(path, TRANSCRIPT_BYTES_MAX)?;
    if &actual != expected {
        return Err(StoreError::new(StoreErrorKind::StorageConflict, path));
    }
    observer.before_rename(path)?;
    std::fs::rename(temp, path).map_err(|error| StoreError::from_io(path, &error))?;
    observer.renamed(path);
    Ok(())
}

fn flush_rewrite_parent(
    parent: &Path,
    temp: &Path,
    observer: &dyn TranscriptRewriteObserver,
) -> Result<(), StoreError> {
    observer.before_parent_sync(parent, temp)?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| StoreError::from_io(parent, &error))?;
    observer.parent_flushed(parent);
    Ok(())
}

fn validate_rewrite_target(root: &Path, path: &Path) -> Result<(), StoreError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => validate_existing(root, path),
        Err(error) => Err(StoreError::from_io(path, &error)),
        Ok(metadata) if metadata.file_type().is_file() => validate_regular_file(root, path),
        Ok(_) => Err(StoreError::new(StoreErrorKind::InvalidPath, path)),
    }
}

fn create_rewrite_temp(
    root: &Path,
    parent: &Path,
    target: &Path,
) -> Result<(PathBuf, File), StoreError> {
    create_rewrite_temp_with(&REWRITE_SEQUENCE, root, parent, target)
}

fn create_rewrite_temp_with(
    sequence: &AtomicU64,
    root: &Path,
    parent: &Path,
    target: &Path,
) -> Result<(PathBuf, File), StoreError> {
    for _ in 0..ATOMIC_WRITE_RETRIES_MAX {
        validate_existing(root, parent)?;
        let sequence = next_sequence(sequence, target)?;
        let path = parent.join(format!(
            ".lotta-transcript-{}-{sequence}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(StoreError::from_io(target, &error)),
        }
    }
    Err(StoreError::new(StoreErrorKind::Limit, target))
}

fn write_staged(
    file: &mut File,
    entries: &[TranscriptEntry],
    target: &Path,
    observer: &dyn TranscriptRewriteObserver,
) -> Result<(), StoreError> {
    use std::io::Write as _;
    let mut total = 0_u64;
    for entry in entries {
        let line = encode_line(entry, target)?;
        let added = u64::try_from(line.len())
            .map_err(|_| StoreError::new(StoreErrorKind::Limit, target))?;
        total = final_size(total, added, target)?;
        observer.before_staged_write(target)?;
        file.write_all(&line)
            .map_err(|error| StoreError::from_io(target, &error))?;
    }
    observer.before_stage_sync(target)?;
    file.sync_all()
        .map_err(|error| StoreError::from_io(target, &error))
}

fn validate_migration_backup(paths: &TranscriptPaths, backup: &Path) -> Result<(), StoreError> {
    if backup == paths.messages || backup == paths.manifest {
        return Err(StoreError::new(StoreErrorKind::StorageConflict, backup));
    }
    validate_existing(&paths.root, backup)?;
    let metadata =
        std::fs::symlink_metadata(backup).map_err(|error| StoreError::from_io(backup, &error))?;
    if !metadata.file_type().is_file() {
        return Err(StoreError::new(StoreErrorKind::StorageConflict, backup));
    }
    Ok(())
}

#[cfg(test)]
mod rewrite_tests;

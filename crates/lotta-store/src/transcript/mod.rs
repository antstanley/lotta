//! Bounded append-only schema-v2 transcript persistence.
//!
//! Lotta's root lock serializes participating Rust writers. TypeScript does not acquire it, so
//! concurrent mixed-runtime writes remain unsupported. A crash may leave an append tail; tolerant
//! prefix recovery belongs to Task 25.

mod append;
pub mod bounds;
pub mod manifest;
mod session_header;
#[cfg(test)]
pub(crate) mod test_support;

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

trait TranscriptRewriteObserver: Send + Sync {
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

struct NoopRewriteObserver;
impl TranscriptRewriteObserver for NoopRewriteObserver {}

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
            rewrite_locked(&paths, &entries, authority)
        })
        .await
    }
}

enum RewriteAuthority {
    Migration(PathBuf),
    Fork,
    Loaded,
}

pub(crate) struct TranscriptPaths {
    pub(crate) root: PathBuf,
    pub(crate) directory: PathBuf,
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

fn encode_append_entry(entry: &TranscriptEntry, path: &Path) -> Result<Vec<u8>, StoreError> {
    if matches!(entry, TranscriptEntry::Session(_)) {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    encode_line(entry, path)
}

fn rewrite_locked(
    paths: &TranscriptPaths,
    entries: &[TranscriptEntry],
    authority: RewriteAuthority,
) -> Result<(), StoreError> {
    let root = backend_root(&paths.messages)?;
    let lock = LottaStorageLock::try_acquire_confined(root)?;
    manifest::read_current(&paths.manifest)?;
    if let RewriteAuthority::Migration(backup) = authority {
        validate_migration_backup(paths, &backup)?;
    }
    let expected = FileRevision::sample_path_bounded(&paths.messages, TRANSCRIPT_BYTES_MAX)?;
    rewrite_streamed(
        &paths.messages,
        entries,
        &expected,
        &lock,
        &NoopRewriteObserver,
    )
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
#[tokio::test]
async fn no_rewrite_on_compaction() {
    use test_support::{compaction, manifest, message, session, setup, transcript_files};
    let (_owned, store, agent, conversation) = setup("transcript-no-rewrite");
    store
        .initialize_transcript(&agent, &conversation, &manifest(), &session())
        .await
        .expect("initialize");
    store
        .append_transcript_entry(&agent, &conversation, &message("entry-1", None, "one"))
        .await
        .expect("message");
    let (manifest_path, messages) = transcript_files(&store, &agent, &conversation);
    let before = std::fs::read(&messages).expect("before");
    let manifest_before = std::fs::read(&manifest_path).expect("manifest before");
    let identity = file_identity(&messages);
    let listing = directory_names(messages.parent().expect("parent"));
    store
        .append_transcript_entry(&agent, &conversation, &compaction("summary".into()))
        .await
        .expect("compaction");
    let after = std::fs::read(&messages).expect("after");
    assert_eq!(&after[..before.len()], before);
    let suffix = &after[before.len()..];
    assert_eq!(suffix.last(), Some(&b'\n'));
    let rows = suffix
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    let appended: serde_json::Value = serde_json::from_slice(rows[0]).expect("appended row");
    assert_eq!(appended["type"], "compaction");
    assert_eq!(file_identity(&messages), identity);
    assert_eq!(directory_names(messages.parent().expect("parent")), listing);
    assert_eq!(
        std::fs::read(manifest_path).expect("manifest after"),
        manifest_before
    );
}

#[cfg(all(test, unix))]
fn file_identity(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).expect("metadata").ino()
}

#[cfg(all(test, not(unix)))]
const fn file_identity(_path: &Path) -> u64 {
    0
}

#[cfg(test)]
fn directory_names(path: &Path) -> std::collections::BTreeSet<String> {
    std::fs::read_dir(path)
        .expect("directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[cfg(test)]
mod rewrite_authority {
    use super::*;
    use crate::ATOMIC_WRITE_BYTES_MAX;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_support::{manifest, message, session, setup, transcript_files};

    #[derive(Clone, Copy)]
    enum FailurePoint {
        Write,
        Sync,
        Compare,
        Rename,
        Parent,
    }

    struct FailureObserver {
        point: FailurePoint,
        renames: AtomicUsize,
    }

    impl FailureObserver {
        const fn new(point: FailurePoint) -> Self {
            Self {
                point,
                renames: AtomicUsize::new(0),
            }
        }
    }

    impl TranscriptRewriteObserver for FailureObserver {
        fn before_staged_write(&self, target: &Path) -> Result<(), StoreError> {
            if matches!(self.point, FailurePoint::Write) {
                return Err(StoreError::new(StoreErrorKind::Io, target));
            }
            Ok(())
        }

        fn before_stage_sync(&self, target: &Path) -> Result<(), StoreError> {
            if matches!(self.point, FailurePoint::Sync) {
                return Err(StoreError::new(StoreErrorKind::Io, target));
            }
            Ok(())
        }

        fn before_compare(&self, target: &Path, _temp: &Path) -> Result<(), StoreError> {
            if matches!(self.point, FailurePoint::Compare) {
                std::fs::write(target, b"external")
                    .map_err(|error| StoreError::from_io(target, &error))?;
            }
            Ok(())
        }

        fn before_rename(&self, target: &Path) -> Result<(), StoreError> {
            if matches!(self.point, FailurePoint::Rename) {
                return Err(StoreError::new(StoreErrorKind::Io, target));
            }
            Ok(())
        }

        fn renamed(&self, _target: &Path) {
            self.renames.fetch_add(1, Ordering::Relaxed);
        }

        fn before_parent_sync(&self, parent: &Path, temp: &Path) -> Result<(), StoreError> {
            if matches!(self.point, FailurePoint::Parent) {
                std::fs::write(temp, b"unrelated")
                    .map_err(|error| StoreError::from_io(temp, &error))?;
                return Err(StoreError::new(StoreErrorKind::Io, parent));
            }
            Ok(())
        }
    }

    async fn initialized(
        label: &str,
    ) -> (
        lotta_testkit::roots::TemporaryRoot,
        LocalStore,
        AgentId,
        ConversationId,
        Vec<TranscriptEntry>,
        PathBuf,
    ) {
        let (root, store, agent, conversation) = setup(label);
        store
            .initialize_transcript(&agent, &conversation, &manifest(), &session())
            .await
            .expect("initialize");
        let (_, messages) = transcript_files(&store, &agent, &conversation);
        let entries = vec![session(), message("entry-1", None, "replacement")];
        (root, store, agent, conversation, entries, messages)
    }

    fn expected_bytes(entries: &[TranscriptEntry]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for entry in entries {
            bytes.extend_from_slice(
                &encode_line(entry, Path::new("/tmp/messages.jsonl")).expect("line"),
            );
        }
        bytes
    }

    #[tokio::test]
    async fn all_three_real_wrappers_replace_canonical_file() {
        let (_r, store, agent, conversation, entries, messages) =
            initialized("rewrite-three").await;
        store
            .persist_loaded_transcript(&agent, &conversation, entries.clone())
            .await
            .expect("loaded");
        store
            .persist_fork_transcript(&agent, &conversation, entries.clone())
            .await
            .expect("fork");
        let backup = messages.with_file_name("messages.backup.jsonl");
        std::fs::copy(&messages, &backup).expect("backup");
        store
            .persist_migrated_transcript(&agent, &conversation, entries.clone(), backup.clone())
            .await
            .expect("migration");
        assert_eq!(
            std::fs::read(messages).expect("messages"),
            expected_bytes(&entries)
        );
    }

    #[tokio::test]
    async fn loaded_persistence_creates_an_absent_messages_file() {
        let (_r, store, agent, conversation, entries, messages) =
            initialized("rewrite-absent").await;
        std::fs::remove_file(&messages).expect("remove messages");
        store
            .persist_loaded_transcript(&agent, &conversation, entries.clone())
            .await
            .expect("create absent transcript");
        assert_eq!(
            std::fs::read(messages).expect("created messages"),
            expected_bytes(&entries)
        );
    }

    #[tokio::test]
    async fn legacy_manifest_rejects_each_wrapper_before_mutation() {
        let (_r, store, agent, conversation, entries, messages) =
            initialized("rewrite-legacy").await;
        let (manifest_path, _) = transcript_files(&store, &agent, &conversation);
        let mut legacy = manifest();
        legacy.schema_version = 1;
        legacy.message_format = lotta_domain::TranscriptMessageFormat::PiAiMessageJsonl;
        std::fs::write(
            &manifest_path,
            manifest::encode_supported_for_test(&legacy, &manifest_path),
        )
        .expect("legacy manifest");
        let before = std::fs::read(&messages).expect("before");
        let backup = messages.with_file_name("backup");
        std::fs::write(&backup, b"proof").expect("backup");
        assert_eq!(
            store
                .persist_loaded_transcript(&agent, &conversation, entries.clone())
                .await
                .expect_err("loaded")
                .kind(),
            StoreErrorKind::Parse
        );
        assert_eq!(
            store
                .persist_fork_transcript(&agent, &conversation, entries.clone())
                .await
                .expect_err("fork")
                .kind(),
            StoreErrorKind::Parse
        );
        assert_eq!(
            store
                .persist_migrated_transcript(&agent, &conversation, entries.clone(), backup.clone())
                .await
                .expect_err("migration")
                .kind(),
            StoreErrorKind::Parse
        );
        assert_eq!(std::fs::read(messages).expect("after"), before);
    }

    #[tokio::test]
    async fn migration_backup_preconditions_reject() {
        let (_r, store, agent, conversation, entries, messages) =
            initialized("rewrite-backup").await;
        let before = std::fs::read(&messages).expect("before");
        let missing = messages.with_file_name("missing.backup");
        assert!(
            store
                .persist_migrated_transcript(&agent, &conversation, entries.clone(), missing)
                .await
                .is_err()
        );
        assert!(
            store
                .persist_migrated_transcript(
                    &agent,
                    &conversation,
                    entries.clone(),
                    messages.clone()
                )
                .await
                .is_err()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = messages.with_file_name("target.backup");
            let link = messages.with_file_name("link.backup");
            std::fs::write(&target, b"proof").expect("target");
            symlink(&target, &link).expect("link");
            assert!(
                store
                    .persist_migrated_transcript(&agent, &conversation, entries.clone(), link)
                    .await
                    .is_err()
            );
        }
        assert_eq!(std::fs::read(messages).expect("after"), before);
    }

    fn observed_rewrite(
        messages: &Path,
        entries: &[TranscriptEntry],
        observer: &FailureObserver,
    ) -> Result<(), StoreError> {
        let root = backend_root(messages)?;
        let lock = LottaStorageLock::try_acquire_confined(root)?;
        let expected = FileRevision::sample_path_bounded(messages, TRANSCRIPT_BYTES_MAX)?;
        rewrite_streamed(messages, entries, &expected, &lock, observer)
    }

    fn temp_names(messages: &Path) -> Vec<String> {
        directory_names(messages.parent().expect("parent"))
            .into_iter()
            .filter(|name| name.starts_with(".lotta-transcript-"))
            .collect()
    }

    #[tokio::test]
    async fn precommit_failures_preserve_target_and_clean_temp() {
        for point in [
            FailurePoint::Write,
            FailurePoint::Sync,
            FailurePoint::Rename,
        ] {
            let (_r, _s, _a, _c, entries, messages) = initialized("rewrite-fail").await;
            let before = std::fs::read(&messages).expect("before");
            let observer = FailureObserver::new(point);
            assert!(observed_rewrite(&messages, &entries, &observer).is_err());
            assert_eq!(std::fs::read(&messages).expect("after"), before);
            assert!(temp_names(&messages).is_empty());
            assert_eq!(observer.renames.load(Ordering::Relaxed), 0);
        }
    }

    #[tokio::test]
    async fn final_revision_conflict_preserves_external_target_and_cleans_temp() {
        let (_r, _s, _a, _c, entries, messages) = initialized("rewrite-conflict").await;
        let observer = FailureObserver::new(FailurePoint::Compare);
        let error = observed_rewrite(&messages, &entries, &observer).expect_err("conflict");
        assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
        assert_eq!(std::fs::read(&messages).expect("external"), b"external");
        assert!(temp_names(&messages).is_empty());
        assert_eq!(observer.renames.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn parent_sync_failure_keeps_commit_and_recreated_temp() {
        let (_r, _s, _a, _c, entries, messages) = initialized("rewrite-parent").await;
        let observer = FailureObserver::new(FailurePoint::Parent);
        let error = observed_rewrite(&messages, &entries, &observer).expect_err("parent sync");
        assert_eq!(error.kind(), StoreErrorKind::Io);
        assert_eq!(
            std::fs::read(&messages).expect("committed"),
            expected_bytes(&entries)
        );
        assert_eq!(observer.renames.load(Ordering::Relaxed), 1);
        let temps = temp_names(&messages);
        assert_eq!(temps.len(), 1);
        let recreated = messages.parent().expect("parent").join(&temps[0]);
        assert_eq!(std::fs::read(&recreated).expect("recreated"), b"unrelated");
        std::fs::remove_file(recreated).expect("cleanup recreated");
    }

    #[tokio::test]
    async fn rewrites_existing_transcript_above_atomic_limit() {
        for length in [
            ATOMIC_WRITE_BYTES_MAX as u64 - 1,
            ATOMIC_WRITE_BYTES_MAX as u64 + 1,
        ] {
            let (_r, store, agent, conversation, entries, messages) =
                initialized("rewrite-large").await;
            store
                .append_transcript_entry(&agent, &conversation, &message("entry-1", None, "source"))
                .await
                .expect("source message");
            let current = std::fs::metadata(&messages).expect("metadata").len();
            let overhead = encode_line(&test_support::compaction(String::new()), &messages)
                .expect("empty compaction")
                .len()
                - 1;
            let summary = usize::try_from(length - current)
                .expect("target length")
                .checked_sub(overhead + 1)
                .expect("summary length");
            store
                .append_transcript_entry(
                    &agent,
                    &conversation,
                    &test_support::compaction("x".repeat(summary)),
                )
                .await
                .expect("valid large source");
            assert_eq!(
                std::fs::metadata(&messages).expect("large metadata").len(),
                length
            );
            store
                .persist_loaded_transcript(&agent, &conversation, entries.clone())
                .await
                .expect("rewrite bounded transcript");
            assert_eq!(
                std::fs::read(&messages).expect("rewritten"),
                expected_bytes(&entries)
            );
        }
    }

    #[test]
    fn transcript_revision_limit_checks_metadata_before_scan() {
        let (_root, store, agent, conversation) = setup("rewrite-max-metadata");
        let paths = transcript_paths(store.paths(), &agent, &conversation).expect("paths");
        std::fs::create_dir_all(&paths.directory).expect("directory");
        assert!(
            crate::atomic::validate_revision_length(
                TRANSCRIPT_BYTES_MAX,
                TRANSCRIPT_BYTES_MAX,
                &paths.messages,
            )
            .is_ok()
        );
        assert_eq!(
            crate::atomic::validate_revision_length(
                TRANSCRIPT_BYTES_MAX + 1,
                TRANSCRIPT_BYTES_MAX,
                &paths.messages,
            )
            .expect_err("above metadata bound")
            .kind(),
            StoreErrorKind::Limit
        );
        let file = File::create(&paths.messages).expect("create");
        file.set_len(TRANSCRIPT_BYTES_MAX + 1)
            .expect("oversize sparse");
        let error = FileRevision::sample_path_bounded(&paths.messages, TRANSCRIPT_BYTES_MAX)
            .expect_err("limit before scan");
        assert_eq!(error.kind(), StoreErrorKind::Limit);
    }

    #[test]
    fn staged_temp_skips_a_bounded_create_new_collision() {
        let (_root, store, agent, conversation) = setup("rewrite-temp-collision");
        let paths = transcript_paths(store.paths(), &agent, &conversation).expect("paths");
        std::fs::create_dir_all(&paths.directory).expect("directory");
        let sequence = AtomicU64::new(40);
        let collision = paths
            .directory
            .join(format!(".lotta-transcript-{}-40.tmp", std::process::id()));
        std::fs::write(&collision, b"stale").expect("stale collision");
        let (created, file) =
            create_rewrite_temp_with(&sequence, &paths.root, &paths.directory, &paths.messages)
                .expect("second sequence");
        drop(file);
        assert!(created.ends_with(format!(".lotta-transcript-{}-41.tmp", std::process::id())));
        std::fs::remove_file(&created).expect("created cleanup");
        assert_eq!(std::fs::read(&collision).expect("stale survives"), b"stale");
        std::fs::remove_file(collision).expect("stale cleanup");
    }
}

#[cfg(test)]
mod negative_evidence {
    use super::*;
    use test_support::{manifest, message, session, setup, transcript_files};

    #[tokio::test]
    async fn invalid_initialization_and_contention_leave_no_mutation() {
        let (_root, store, agent, conversation) = setup("transcript-negative");
        let bad = message("not-session", None, "x");
        assert_eq!(
            store
                .initialize_transcript(&agent, &conversation, &manifest(), &bad)
                .await
                .expect_err("wrong initial entry")
                .kind(),
            StoreErrorKind::Parse
        );
        let (manifest_path, messages) = transcript_files(&store, &agent, &conversation);
        assert!(!manifest_path.exists());
        assert!(!messages.exists());

        let mut invalid = manifest();
        invalid.schema_version = 9;
        assert_eq!(
            store
                .initialize_transcript(&agent, &conversation, &invalid, &session())
                .await
                .expect_err("invalid manifest")
                .kind(),
            StoreErrorKind::Parse
        );
        assert!(!manifest_path.exists());
        assert!(!messages.exists());

        store
            .initialize_transcript(&agent, &conversation, &manifest(), &session())
            .await
            .expect("initialize");
        let before = std::fs::read(&messages).expect("before contention");
        let second = LocalStore::new(store.paths().clone());
        let lock = LottaStorageLock::try_acquire(store.paths().root()).expect("held lock");
        assert_eq!(
            second
                .append_transcript_entry(&agent, &conversation, &message("entry", None, "x"))
                .await
                .expect_err("contended append")
                .kind(),
            StoreErrorKind::LottaLock
        );
        drop(lock);
        assert_eq!(std::fs::read(&messages).expect("after contention"), before);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_messages_leave_outside_bytes_unchanged() {
        use std::os::unix::fs::symlink;
        let (_root, store, agent, conversation) = setup("transcript-symlink");
        let (_, messages) = transcript_files(&store, &agent, &conversation);
        std::fs::create_dir_all(messages.parent().expect("parent")).expect("directory");
        let outside =
            lotta_testkit::roots::TemporaryRoot::new("transcript-outside").expect("outside");
        let sentinel = outside.path().join("sentinel");
        std::fs::write(&sentinel, b"safe").expect("sentinel");
        symlink(&sentinel, &messages).expect("symlink");
        assert_eq!(
            store
                .initialize_transcript(&agent, &conversation, &manifest(), &session())
                .await
                .expect_err("symlink initialize")
                .kind(),
            StoreErrorKind::StorageConflict
        );
        assert_eq!(std::fs::read(sentinel).expect("sentinel after"), b"safe");
    }
}

//! Durable process-safe compaction transaction journal.

use crate::{LocalStore, LottaStorageLock, StoreError, StoreErrorKind, WriteMode};
use lotta_domain::{NonEmptyString, RuntimeScope, TranscriptEntry};
use serde::{Deserialize, Serialize};
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Maximum serialized compaction journal size.
pub const COMPACTION_JOURNAL_BYTES_MAX: usize = 8 * 1_024 * 1_024;
/// Maximum retained compaction transactions.
pub const COMPACTION_TRANSACTIONS_MAX: usize = 1_024;
/// Maximum bytes retained in one safe publication projection.
pub const COMPACTION_PROJECTION_BYTES_MAX: usize = 256 * 1_024;
const COMPACTION_LOCK_RETRIES_MAX: usize = 10_000;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
/// Durable state for one compaction request.
pub enum CompactionTransactionState {
    /// Claimed but no transcript entry is known durable yet.
    Pending,
    /// Transcript entry is durable and publication may be retried.
    Appended {
        /// Stable transcript entry identifier.
        entry_id: NonEmptyString,
    },
    /// Context publication completed.
    Published,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Outcome of claiming one scoped request.
pub enum CompactionClaim {
    /// The caller owns work for a pending or appended transaction.
    Owned,
    /// Another live caller currently owns the transaction in this process.
    Busy,
    /// The request already completed publication.
    Published,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Bounded safe information needed to resume publication after restart.
pub struct CompactionProjection {
    /// Summary text used for the transcript and context prompt.
    pub summary: String,
    /// Exact retained local message identifiers in model-visible order.
    pub retained_message_ids: Vec<NonEmptyString>,
    /// Token estimate before compaction.
    pub tokens_before: u64,
    /// Token estimate after compaction.
    pub tokens_after: u64,
    /// Message count before compaction.
    pub messages_before: usize,
    /// Message count after compaction.
    pub messages_after: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// One durable scoped compaction transaction.
pub struct CompactionTransaction {
    /// Runtime scope participating in the transaction.
    pub scope: RuntimeScope,
    /// Stable caller request identifier.
    pub request_id: NonEmptyString,
    /// Monotonic transaction revision used for compare-and-set transitions.
    pub revision: u64,
    /// Durable transaction state.
    pub state: CompactionTransactionState,
    /// Safe bounded publication projection, present after planning and summarization.
    pub projection: Option<CompactionProjection>,
}

#[derive(Default, Deserialize, Serialize)]
struct JournalFile {
    revision: u64,
    transactions: Vec<CompactionTransaction>,
}

impl LocalStore {
    fn compaction_path(&self) -> PathBuf {
        self.paths().runtime().join("compactions.json")
    }

    /// Claims a scoped request before provider summarization.
    ///
    /// Existing pending and appended records are recoverable ownership claims after restart.
    /// In-process callers are serialized by the root lock and must use the returned state to
    /// decide whether to summarize or resume publication.
    ///
    /// # Errors
    /// Returns confinement, security, bound, conflict, lock, or durability failures.
    pub fn claim_compaction(
        &self,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
    ) -> Result<(CompactionClaim, CompactionTransaction), StoreError> {
        let lock = self.compaction_lock()?;
        let mut file = self.read_compactions()?;
        if let Some(existing) = exact(&file.transactions, scope, request_id).cloned() {
            let claim = if matches!(existing.state, CompactionTransactionState::Published) {
                CompactionClaim::Published
            } else {
                CompactionClaim::Owned
            };
            return Ok((claim, existing));
        }
        if file.transactions.len() >= COMPACTION_TRANSACTIONS_MAX {
            return Err(limit(self.compaction_path()));
        }
        let transaction = CompactionTransaction {
            scope: scope.clone(),
            request_id: request_id.clone(),
            revision: 0,
            state: CompactionTransactionState::Pending,
            projection: None,
        };
        file.transactions.push(transaction.clone());
        self.write_compactions(&mut file, &lock)?;
        Ok((CompactionClaim::Owned, transaction))
    }

    /// Loads one scoped compaction transaction.
    ///
    /// # Errors
    /// Returns confinement, security, bound, or parse failures.
    pub fn compaction_transaction(
        &self,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
    ) -> Result<Option<CompactionTransaction>, StoreError> {
        self.read_compactions()
            .map(|file| exact(&file.transactions, scope, request_id).cloned())
    }

    /// Persists the safe projection while retaining `Pending` state.
    ///
    /// # Errors
    /// Returns conflict, bound, security, lock, or durability failures.
    pub fn record_compaction_projection(
        &self,
        expected_revision: u64,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
        projection: CompactionProjection,
    ) -> Result<CompactionTransaction, StoreError> {
        projection_bytes(&projection, &self.compaction_path())?;
        self.update_compaction(expected_revision, scope, request_id, |transaction| {
            if !matches!(transaction.state, CompactionTransactionState::Pending) {
                return Err(conflict(self.compaction_path()));
            }
            transaction.projection = Some(projection);
            Ok(())
        })
    }

    /// Marks a durable transcript append for a scoped request.
    ///
    /// # Errors
    /// Returns conflict, security, lock, bound, or durability failures.
    pub fn mark_compaction_appended(
        &self,
        expected_revision: u64,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
        entry_id: NonEmptyString,
    ) -> Result<CompactionTransaction, StoreError> {
        self.update_compaction(expected_revision, scope, request_id, |transaction| {
            match &transaction.state {
                CompactionTransactionState::Pending => {
                    if transaction.projection.is_none() {
                        return Err(conflict(self.compaction_path()));
                    }
                    transaction.state = CompactionTransactionState::Appended { entry_id };
                }
                CompactionTransactionState::Appended { entry_id: current }
                    if current == &entry_id => {}
                _ => return Err(conflict(self.compaction_path())),
            }
            Ok(())
        })
    }

    /// Marks publication complete for a scoped request.
    ///
    /// # Errors
    /// Returns conflict, security, lock, bound, or durability failures.
    pub fn mark_compaction_published(
        &self,
        expected_revision: u64,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
    ) -> Result<CompactionTransaction, StoreError> {
        self.update_compaction(expected_revision, scope, request_id, |transaction| {
            if matches!(
                transaction.state,
                CompactionTransactionState::Appended { .. }
            ) {
                transaction.state = CompactionTransactionState::Published;
                Ok(())
            } else {
                Err(conflict(self.compaction_path()))
            }
        })
    }

    /// Appends a compaction entry exactly once and advances its journal transaction under the same
    /// root lock. Existing transcript rows are matched by both request and entry identifier.
    ///
    /// # Errors
    /// Returns transcript, journal, conflict, security, lock, bound, or durability failures.
    pub async fn append_compaction_if_absent(
        &self,
        expected_revision: u64,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
        entry: &TranscriptEntry,
    ) -> Result<CompactionTransaction, StoreError> {
        let store = self.clone();
        let scope = scope.clone();
        let request_id = request_id.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || {
            store.append_compaction_if_absent_blocking(
                expected_revision,
                &scope,
                &request_id,
                &entry,
            )
        })
        .await
        .map_err(|_| StoreError::new(StoreErrorKind::Io, self.compaction_path()))?
    }

    fn append_compaction_if_absent_blocking(
        &self,
        expected_revision: u64,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
        entry: &TranscriptEntry,
    ) -> Result<CompactionTransaction, StoreError> {
        let TranscriptEntry::Compaction(compaction) = entry else {
            return Err(parse(self.compaction_path()));
        };
        if &compaction.id != request_id {
            return Err(conflict(self.compaction_path()));
        }
        let lock = self.compaction_lock()?;
        let mut file = self.read_compactions()?;
        let transaction = exact_mut(&mut file.transactions, scope, request_id)
            .ok_or_else(|| conflict(self.compaction_path()))?;
        if let CompactionTransactionState::Appended { entry_id } = &transaction.state {
            if entry_id == &compaction.id {
                return Ok(transaction.clone());
            }
            return Err(conflict(self.compaction_path()));
        }
        if transaction.revision != expected_revision
            || !matches!(transaction.state, CompactionTransactionState::Pending)
            || transaction.projection.is_none()
        {
            return Err(conflict(self.compaction_path()));
        }
        let paths = crate::transcript::transcript_paths(
            self.paths(),
            &scope.agent_id,
            &scope.conversation_id,
        )?;
        let line = crate::transcript::encode_append_entry(entry, &paths.messages)?;
        let existing = transcript_has_compaction(&paths, request_id)?;
        if !existing {
            crate::transcript::append::append_locked(&paths, &line, &lock)?;
        }
        transaction.state = CompactionTransactionState::Appended {
            entry_id: compaction.id.clone(),
        };
        transaction.revision = transaction
            .revision
            .checked_add(1)
            .ok_or_else(|| limit(self.compaction_path()))?;
        let result = transaction.clone();
        self.write_compactions(&mut file, &lock)?;
        Ok(result)
    }

    fn update_compaction(
        &self,
        expected_revision: u64,
        scope: &RuntimeScope,
        request_id: &NonEmptyString,
        update: impl FnOnce(&mut CompactionTransaction) -> Result<(), StoreError>,
    ) -> Result<CompactionTransaction, StoreError> {
        let lock = self.compaction_lock()?;
        let mut file = self.read_compactions()?;
        let transaction = exact_mut(&mut file.transactions, scope, request_id)
            .ok_or_else(|| conflict(self.compaction_path()))?;
        if transaction.revision != expected_revision {
            return Err(conflict(self.compaction_path()));
        }
        update(transaction)?;
        transaction.revision = transaction
            .revision
            .checked_add(1)
            .ok_or_else(|| limit(self.compaction_path()))?;
        let result = transaction.clone();
        self.write_compactions(&mut file, &lock)?;
        Ok(result)
    }

    fn compaction_lock(&self) -> Result<LottaStorageLock, StoreError> {
        let mut last = None;
        for _ in 0..COMPACTION_LOCK_RETRIES_MAX {
            match LottaStorageLock::try_acquire_confined(self.paths().root()) {
                Ok(lock) => return Ok(lock),
                Err(error) if error.kind() == StoreErrorKind::LottaLock => {
                    last = Some(error);
                    std::thread::yield_now();
                }
                Err(error) => return Err(error),
            }
        }
        Err(last.unwrap_or_else(|| StoreError::new(StoreErrorKind::LottaLock, self.paths().root())))
    }

    fn read_compactions(&self) -> Result<JournalFile, StoreError> {
        let path = self.compaction_path();
        crate::confinement::validate_existing(self.paths().root(), &path)?;
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(JournalFile::default());
            }
            Err(error) => return Err(StoreError::from_io(&path, &error)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(parse(&path));
        }
        if metadata.len() > COMPACTION_JOURNAL_BYTES_MAX as u64 {
            return Err(limit(&path));
        }
        reject_mode(&metadata, &path)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(usize::try_from(metadata.len()).map_err(|_| limit(&path))?)
            .map_err(|_| limit(&path))?;
        std::fs::File::open(&path)
            .map_err(|error| StoreError::from_io(&path, &error))?
            .take((COMPACTION_JOURNAL_BYTES_MAX + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| StoreError::from_io(&path, &error))?;
        if bytes.len() > COMPACTION_JOURNAL_BYTES_MAX {
            return Err(limit(&path));
        }
        let file: JournalFile = serde_json::from_slice(&bytes).map_err(|_| parse(&path))?;
        if file.transactions.len() > COMPACTION_TRANSACTIONS_MAX {
            return Err(limit(&path));
        }
        for transaction in &file.transactions {
            if let Some(projection) = &transaction.projection {
                projection_bytes(projection, &path)?;
            }
        }
        Ok(file)
    }

    fn write_compactions(
        &self,
        file: &mut JournalFile,
        lock: &LottaStorageLock,
    ) -> Result<(), StoreError> {
        if file.transactions.len() > COMPACTION_TRANSACTIONS_MAX {
            return Err(limit(self.compaction_path()));
        }
        file.revision = file
            .revision
            .checked_add(1)
            .ok_or_else(|| limit(self.compaction_path()))?;
        let bytes = serde_json::to_vec(file).map_err(|_| parse(self.compaction_path()))?;
        if bytes.len() > COMPACTION_JOURNAL_BYTES_MAX {
            return Err(limit(self.compaction_path()));
        }
        crate::atomic::atomic_write_locked(
            &self.compaction_path(),
            &bytes,
            WriteMode::ProviderAuth,
            lock,
        )
    }
}

fn transcript_has_compaction(
    paths: &crate::transcript::TranscriptPaths,
    request_id: &NonEmptyString,
) -> Result<bool, StoreError> {
    crate::transcript::manifest::read_current(&paths.manifest)?;
    crate::confinement::validate_regular_file(&paths.root, &paths.messages)?;
    let mut file = std::fs::File::open(&paths.messages)
        .map_err(|error| StoreError::from_io(&paths.messages, &error))?;
    let metadata = file
        .metadata()
        .map_err(|error| StoreError::from_io(&paths.messages, &error))?;
    if metadata.len() > crate::TRANSCRIPT_BYTES_MAX {
        return Err(limit(&paths.messages));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(usize::try_from(metadata.len()).map_err(|_| limit(&paths.messages))?)
        .map_err(|_| limit(&paths.messages))?;
    file.read_to_end(&mut bytes)
        .map_err(|error| StoreError::from_io(&paths.messages, &error))?;
    for row in bytes
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
    {
        let value: serde_json::Value =
            serde_json::from_slice(row).map_err(|_| parse(&paths.messages))?;
        if value.get("type").and_then(serde_json::Value::as_str) == Some("compaction")
            && value.get("id").and_then(serde_json::Value::as_str) == Some(request_id.as_str())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn exact<'a>(
    values: &'a [CompactionTransaction],
    scope: &RuntimeScope,
    request_id: &NonEmptyString,
) -> Option<&'a CompactionTransaction> {
    values
        .iter()
        .find(|value| &value.scope == scope && &value.request_id == request_id)
}

fn exact_mut<'a>(
    values: &'a mut [CompactionTransaction],
    scope: &RuntimeScope,
    request_id: &NonEmptyString,
) -> Option<&'a mut CompactionTransaction> {
    values
        .iter_mut()
        .find(|value| &value.scope == scope && &value.request_id == request_id)
}

fn projection_bytes(projection: &CompactionProjection, path: &Path) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(projection).map_err(|_| parse(path))?;
    if bytes.len() > COMPACTION_PROJECTION_BYTES_MAX {
        Err(limit(path))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn reject_mode(metadata: &std::fs::Metadata, path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    if metadata.permissions().mode() & 0o077 != 0 {
        Err(parse(path))
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
fn reject_mode(_: &std::fs::Metadata, _: &Path) -> Result<(), StoreError> {
    Ok(())
}

fn parse(path: impl AsRef<Path>) -> StoreError {
    StoreError::new(StoreErrorKind::Parse, path.as_ref().to_path_buf())
}

fn conflict(path: impl AsRef<Path>) -> StoreError {
    StoreError::new(StoreErrorKind::StorageConflict, path.as_ref().to_path_buf())
}

fn limit(path: impl AsRef<Path>) -> StoreError {
    StoreError::new(StoreErrorKind::Limit, path.as_ref().to_path_buf())
}

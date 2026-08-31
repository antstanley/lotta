use crate::adapter::{read_record, write_record_locked};
use crate::atomic::FileRevision;
use crate::refresh::{RecordCache, Snapshot};
use crate::{LottaStorageLock, StoreError, StoreErrorKind, StorePaths, WriteMode};
use lotta_domain::{
    AgentId, BoundedMap, Conversation, ConversationId, InContextMessageIds, Timestamp,
};
use std::path::Path;

/// Maximum number of persisted conversations belonging to one agent.
pub const CONVERSATIONS_PER_AGENT_MAX: usize = 100_000;

pub(crate) fn load(
    path: &Path,
    agent: &AgentId,
    conversation: &ConversationId,
    cache: &RecordCache,
) -> Result<(Conversation, FileRevision), StoreError> {
    let revision = FileRevision::sample_path(path)?;
    if let Some(Snapshot::Conversation(value)) = cache.matching(path, &revision)? {
        validate_identity(path, &value, agent, conversation)?;
        return Ok((value, revision));
    }
    let value: Conversation = read_record(path)?;
    validate_identity(path, &value, agent, conversation)?;
    cache.insert(
        path,
        revision.clone(),
        Snapshot::Conversation(value.clone()),
    )?;
    Ok((value, revision))
}

pub(crate) fn save(
    paths: &StorePaths,
    value: &Conversation,
    cache: &RecordCache,
    limit: usize,
) -> Result<(), StoreError> {
    let path = record_path(paths, &value.agent_id, &value.id)?;
    let lock = LottaStorageLock::try_acquire(paths.root())?;
    let revision = FileRevision::sample_path(&path)?;
    if revision.exists() {
        let existing: Conversation = read_record(&path)?;
        validate_identity(&path, &existing, &value.agent_id, &value.id)?;
    } else {
        ensure_create_capacity(paths, &value.agent_id, limit, &path)?;
    }
    write_record_locked(&path, value, &revision, &lock)?;
    refresh_after_write(&path, value, cache)
}

/// Creates one conversation atomically under the backend writer lock.
///
/// The per-agent cap is enforced once and a globally unused monotonic
/// identifier is allocated inside the same critical section, so concurrent
/// creators always receive distinct identifiers and can never exceed the cap.
pub(crate) fn create(
    paths: &StorePaths,
    agent: &AgentId,
    limit: usize,
    cache: &RecordCache,
    build: impl FnOnce(ConversationId) -> Result<Conversation, StoreError>,
) -> Result<Conversation, StoreError> {
    let lock = LottaStorageLock::try_acquire(paths.root())?;
    let directory = paths.conversations();
    ensure_create_capacity(paths, agent, limit, &directory)?;
    let sequence = allocate_sequence(paths, &directory, &lock)?;
    let id = ConversationId::generate(sequence)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, &directory))?;
    let value = build(id.clone())?;
    if &value.agent_id != agent || value.id != id {
        return Err(StoreError::new(StoreErrorKind::Parse, directory));
    }
    let path = record_path(paths, agent, &id)?;
    let revision = FileRevision::sample_path(&path)?;
    write_record_locked(&path, &value, &revision, &lock)?;
    refresh_after_write(&path, &value, cache)?;
    Ok(value)
}

fn allocate_sequence(
    paths: &StorePaths,
    directory: &Path,
    lock: &LottaStorageLock,
) -> Result<u64, StoreError> {
    let counter = paths.root().join(".conversation-sequence");
    let recorded = match std::fs::read_to_string(&counter) {
        Ok(text) => text
            .trim()
            .parse::<u64>()
            .map_err(|_| StoreError::new(StoreErrorKind::Parse, &counter))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(_) => return Err(StoreError::new(StoreErrorKind::Io, &counter)),
    };
    let existing = crate::adapter::max_conversation_sequence(directory)?;
    let next = recorded
        .max(existing)
        .checked_add(1)
        .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, directory))?;
    crate::atomic::atomic_write_locked(
        &counter,
        next.to_string().as_bytes(),
        WriteMode::Standard,
        lock,
    )?;
    Ok(next)
}

pub(crate) fn archive(
    paths: &StorePaths,
    agent: &AgentId,
    conversation: &ConversationId,
    archived: bool,
    now: Timestamp,
    cache: &RecordCache,
) -> Result<Conversation, StoreError> {
    update(paths, agent, conversation, cache, |value| {
        value.archived = archived;
        value.archived_at = if archived {
            match value.archived_at {
                Some(Some(first)) => Some(Some(first)),
                _ => Some(Some(now)),
            }
        } else {
            Some(None)
        };
        value.updated_at = now;
    })
}

pub(crate) fn compaction_context(
    paths: &StorePaths,
    agent: &AgentId,
    conversation: &ConversationId,
    summary: String,
    ids: InContextMessageIds,
    now: Timestamp,
    cache: &RecordCache,
) -> Result<Conversation, StoreError> {
    update(paths, agent, conversation, cache, |value| {
        value.summary = Some(Some(summary));
        value.in_context_message_ids = ids;
        value.updated_at = now;
    })
}

pub(crate) fn model_override(
    paths: &StorePaths,
    agent: &AgentId,
    conversation: &ConversationId,
    model: Option<String>,
    settings: BoundedMap<1_024>,
    now: Timestamp,
    cache: &RecordCache,
) -> Result<Conversation, StoreError> {
    update(paths, agent, conversation, cache, |value| {
        value.model = Some(model);
        value.model_settings = Some(settings);
        value.updated_at = now;
    })
}

fn update(
    paths: &StorePaths,
    agent: &AgentId,
    conversation: &ConversationId,
    cache: &RecordCache,
    mutation: impl FnOnce(&mut Conversation),
) -> Result<Conversation, StoreError> {
    let path = record_path(paths, agent, conversation)?;
    let (mut value, revision) = load(&path, agent, conversation, cache)?;
    mutation(&mut value);
    let lock = LottaStorageLock::try_acquire(paths.root())?;
    let result = write_record_locked(&path, &value, &revision, &lock);
    if result.is_err() {
        cache.invalidate(&path)?;
        return result.map(|()| value);
    }
    refresh_after_write(&path, &value, cache)?;
    Ok(value)
}

fn refresh_after_write(
    path: &Path,
    value: &Conversation,
    cache: &RecordCache,
) -> Result<(), StoreError> {
    cache.invalidate(path)?;
    let revision = FileRevision::sample_path(path)?;
    cache.insert(path, revision, Snapshot::Conversation(value.clone()))
}

fn ensure_create_capacity(
    paths: &StorePaths,
    agent: &AgentId,
    maximum: usize,
    path: &Path,
) -> Result<(), StoreError> {
    let count = super::adapter::count_conversations_for_agent(&paths.conversations(), agent)?;
    if count >= maximum {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    Ok(())
}

pub(crate) fn record_path(
    paths: &StorePaths,
    agent: &AgentId,
    conversation: &ConversationId,
) -> Result<std::path::PathBuf, StoreError> {
    Ok(paths
        .conversation_dir(agent, conversation)?
        .join("conversation.json"))
}

fn validate_identity(
    path: &Path,
    value: &Conversation,
    agent: &AgentId,
    conversation: &ConversationId,
) -> Result<(), StoreError> {
    if &value.agent_id != agent || &value.id != conversation {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    Ok(())
}

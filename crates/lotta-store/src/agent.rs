use crate::adapter::{read_record, write_record_locked};
use crate::atomic::FileRevision;
use crate::refresh::{RecordCache, Snapshot};
use crate::{LottaStorageLock, StoreError, StoreErrorKind, StorePaths};
use lotta_domain::{Agent, AgentId, NonEmptyString};
use std::path::Path;

/// Maximum number of persisted agents.
pub const AGENTS_MAX: usize = 100_000;

pub(crate) fn load(
    path: &Path,
    expected_id: &AgentId,
    cache: &RecordCache,
) -> Result<(Agent, FileRevision), StoreError> {
    let revision = FileRevision::sample_path(path)?;
    if let Some(Snapshot::Agent(value)) = cache.matching(path, &revision)? {
        validate_id(path, &value, expected_id)?;
        return Ok((value, revision));
    }
    let value: Agent = read_record(path)?;
    validate_id(path, &value, expected_id)?;
    cache.insert(path, revision.clone(), Snapshot::Agent(value.clone()))?;
    Ok((value, revision))
}

pub(crate) fn save(
    paths: &StorePaths,
    value: &Agent,
    cache: &RecordCache,
    limit: usize,
) -> Result<(), StoreError> {
    let path = paths.agent_record(&value.id)?;
    let root = paths.root();
    let lock = LottaStorageLock::try_acquire(root)?;
    let revision = FileRevision::sample_path(&path)?;
    if revision.exists() {
        let existing: Agent = read_record(&path)?;
        validate_id(&path, &existing, &value.id)?;
    } else {
        ensure_create_capacity(&paths.agents(), limit, &path)?;
    }
    write_record_locked(&path, value, &revision, &lock)?;
    refresh_after_write(&path, value, cache)
}

pub(crate) fn update_name(
    paths: &StorePaths,
    id: &AgentId,
    name: NonEmptyString,
    cache: &RecordCache,
    before_write: impl FnOnce(&Path) -> Result<(), StoreError>,
) -> Result<Agent, StoreError> {
    let path = paths.agent_record(id)?;
    let (mut value, revision) = load(&path, id, cache)?;
    value.name = name;
    before_write(&path)?;
    let lock = LottaStorageLock::try_acquire(paths.root())?;
    let result = write_record_locked(&path, &value, &revision, &lock);
    if result.is_err() {
        cache.invalidate(&path)?;
        return result.map(|()| value);
    }
    refresh_after_write(&path, &value, cache)?;
    Ok(value)
}

fn refresh_after_write(path: &Path, value: &Agent, cache: &RecordCache) -> Result<(), StoreError> {
    cache.invalidate(path)?;
    let revision = FileRevision::sample_path(path)?;
    cache.insert(path, revision, Snapshot::Agent(value.clone()))
}

fn validate_id(path: &Path, value: &Agent, expected: &AgentId) -> Result<(), StoreError> {
    if &value.id != expected {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    Ok(())
}

pub(crate) fn ensure_create_capacity(
    directory: &Path,
    maximum: usize,
    path: &Path,
) -> Result<(), StoreError> {
    let count = super::adapter::count_records(directory, super::adapter::RecordKind::Agent)?;
    if count >= maximum {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    Ok(())
}

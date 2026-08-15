use crate::StorePaths;
use crate::adapter::{push_bounded_max, read_record};
use crate::confinement::{backend_root, validate_existing};
use crate::paths::ConversationKey;
use crate::query::types::QUERY_SCAN_ENTRIES_MAX;
use crate::{StoreError, StoreErrorKind};
use lotta_domain::{Agent, AgentId, Conversation};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(crate) fn agents(paths: &StorePaths) -> Result<Vec<Agent>, StoreError> {
    let directory = paths.agents();
    let mut values = Vec::new();
    let mut ids = BTreeSet::new();
    for path in entries(&directory)? {
        if lock_file(&path) {
            continue;
        }
        let metadata = metadata(&path)?;
        if !metadata.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, path));
        }
        let value: Agent = read_record(&path)?;
        if !value.id.as_str().starts_with("agent-local-")
            || paths.agent_record(&value.id)? != path
            || !ids.insert(value.id.clone())
        {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, path));
        }
        push_bounded_max(&mut values, value, &directory, QUERY_SCAN_ENTRIES_MAX)?;
    }
    Ok(values)
}

pub(crate) fn conversations(
    directory: &Path,
    expected_agent: Option<&AgentId>,
    malformed_is_error: bool,
    include_default: bool,
) -> Result<Vec<Conversation>, StoreError> {
    let mut values = Vec::new();
    let mut ids = BTreeSet::new();
    for path in entries(directory)? {
        if lock_file(&path) {
            continue;
        }
        let Some((key, value)) = conversation_entry(&path, malformed_is_error)? else {
            continue;
        };
        let authoritative = match key {
            ConversationKey::Default(agent) => value.id.is_default() && value.agent_id == agent,
            ConversationKey::Named(id) => {
                value.id == id && expected_agent.is_none_or(|agent| &value.agent_id == agent)
            }
        };
        if !authoritative {
            if malformed_is_error {
                return Err(StoreError::new(StoreErrorKind::InvalidPath, path));
            }
            continue;
        }
        let pair = (value.agent_id.clone(), value.id.clone());
        if !ids.insert(pair) {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, path));
        }
        if expected_agent.is_none_or(|agent| &value.agent_id == agent)
            && (include_default || !value.id.is_default())
        {
            push_bounded_max(&mut values, value, directory, QUERY_SCAN_ENTRIES_MAX)?;
        }
    }
    Ok(values)
}

fn conversation_entry(
    path: &Path,
    malformed_is_error: bool,
) -> Result<Option<(ConversationKey, Conversation)>, StoreError> {
    let metadata = metadata(path)?;
    if !metadata.is_dir() {
        return Err(StoreError::new(StoreErrorKind::InvalidPath, path));
    }
    let encoded = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| StoreError::new(StoreErrorKind::InvalidPath, path))?;
    let Ok(key) = ConversationKey::decode(encoded) else {
        return if malformed_is_error {
            Err(StoreError::new(StoreErrorKind::InvalidPath, path))
        } else {
            Ok(None)
        };
    };
    let record = path.join("conversation.json");
    match std::fs::symlink_metadata(&record) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(StoreError::from_io(&record, &error)),
        Ok(value) if value.file_type().is_symlink() || !value.is_file() => {
            Err(StoreError::new(StoreErrorKind::InvalidPath, record))
        }
        Ok(_) => read_record(&record)
            .map(|value| Some((key, value)))
            .or_else(|error| {
                if malformed_is_error {
                    Err(error)
                } else {
                    Ok(None)
                }
            }),
    }
}

#[cfg(test)]
pub(crate) fn validate_scan_entries(length: usize) -> Result<(), StoreError> {
    if length > QUERY_SCAN_ENTRIES_MAX {
        Err(StoreError::new(StoreErrorKind::Limit, "query-scan"))
    } else {
        Ok(())
    }
}

fn entries(directory: &Path) -> Result<Vec<PathBuf>, StoreError> {
    let root = backend_root(directory)?;
    validate_existing(root, directory)?;
    let read = match std::fs::read_dir(directory) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(StoreError::from_io(directory, &error)),
    };
    let mut paths = Vec::new();
    for item in read {
        let item = item.map_err(|error| StoreError::from_io(directory, &error))?;
        let kind = item
            .file_type()
            .map_err(|error| StoreError::from_io(&item.path(), &error))?;
        if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, item.path()));
        }
        #[cfg(test)]
        validate_scan_entries(
            paths
                .len()
                .checked_add(1)
                .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, directory))?,
        )?;
        push_bounded_max(&mut paths, item.path(), directory, QUERY_SCAN_ENTRIES_MAX)?;
    }
    paths.sort();
    Ok(paths)
}

fn metadata(path: &Path) -> Result<std::fs::Metadata, StoreError> {
    let value =
        std::fs::symlink_metadata(path).map_err(|error| StoreError::from_io(path, &error))?;
    if value.file_type().is_symlink() {
        return Err(StoreError::new(StoreErrorKind::InvalidPath, path));
    }
    Ok(value)
}

fn lock_file(path: &Path) -> bool {
    path.file_name().and_then(|value| value.to_str()) == Some(".lotta-storage.lock")
}

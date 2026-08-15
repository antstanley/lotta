use crate::atomic::FileRevision;
use crate::{StoreError, StoreErrorKind};
use lotta_domain::{Agent, Conversation};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const RECORD_CACHE_ENTRIES_MAX: usize = 200_000;

#[derive(Clone, Debug)]
pub(crate) enum Snapshot {
    Agent(Agent),
    Conversation(Conversation),
}

#[derive(Clone, Debug)]
struct Entry {
    revision: FileRevision,
    snapshot: Snapshot,
}

#[derive(Clone, Debug)]
pub(crate) struct RecordCache {
    entries: Arc<Mutex<BTreeMap<PathBuf, Entry>>>,
    limit: usize,
}

impl RecordCache {
    pub(crate) fn new() -> Self {
        Self::with_limit(RECORD_CACHE_ENTRIES_MAX)
    }

    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            entries: Arc::new(Mutex::new(BTreeMap::new())),
            limit,
        }
    }

    pub(crate) fn matching(
        &self,
        path: &Path,
        revision: &FileRevision,
    ) -> Result<Option<Snapshot>, StoreError> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| StoreError::new(StoreErrorKind::Io, path))?;
        Ok(entries
            .get(path)
            .filter(|entry| &entry.revision == revision)
            .map(|entry| entry.snapshot.clone()))
    }

    pub(crate) fn insert(
        &self,
        path: &Path,
        revision: FileRevision,
        snapshot: Snapshot,
    ) -> Result<(), StoreError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| StoreError::new(StoreErrorKind::Io, path))?;
        if self.limit == 0 {
            return Ok(());
        }
        if !entries.contains_key(path) && entries.len() >= self.limit {
            drop(entries.pop_first());
        }
        entries.insert(path.to_path_buf(), Entry { revision, snapshot });
        Ok(())
    }

    pub(crate) fn invalidate(&self, path: &Path) -> Result<(), StoreError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| StoreError::new(StoreErrorKind::Io, path))?;
        entries.remove(path);
        Ok(())
    }
}

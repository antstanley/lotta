use crate::atomic::{FileRevision, atomic_write_expected_locked};
use crate::confinement::{backend_root, validate_existing, validate_regular_file};
use crate::refresh::RecordCache;
use crate::{LottaStorageLock, StoreError, StoreErrorKind, StorePaths, WriteMode, atomic_delete};
use lotta_domain::{Agent, AgentId, Conversation, ConversationId};
use lotta_runtime::RuntimeError;
use lotta_runtime::ports::{AgentStore, ConversationStore, PortFuture};
use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tokio::sync::{Semaphore, mpsc::Sender};
use tokio_util::sync::CancellationToken;

pub(crate) const RECORD_BYTES_MAX: usize = 8 * 1_024 * 1_024;
const LIST_ENTRIES_MAX: usize = 100_000;
/// Maximum simultaneous blocking local-store operations per shared process pool.
pub const LOCAL_STORE_BLOCKING_MAX: usize = 8;
static BLOCKING_POOL: OnceLock<Arc<Semaphore>> = OnceLock::new();

/// Real JSON record adapter using baseline paths and the shared durable replacement primitive.
#[derive(Clone, Debug)]
pub struct LocalStore {
    paths: StorePaths,
    blocking: Arc<Semaphore>,
    cache: RecordCache,
    agents_max: usize,
    conversations_per_agent_max: usize,
}

impl LocalStore {
    /// Creates an adapter for an already validated local-backend layout.
    #[must_use]
    pub fn new(paths: StorePaths) -> Self {
        let blocking = Arc::clone(
            BLOCKING_POOL.get_or_init(|| Arc::new(Semaphore::new(LOCAL_STORE_BLOCKING_MAX))),
        );
        Self {
            paths,
            blocking,
            cache: RecordCache::new(),
            agents_max: crate::agent::AGENTS_MAX,
            conversations_per_agent_max: crate::conversation::CONVERSATIONS_PER_AGENT_MAX,
        }
    }
    /// Returns the adapter's storage layout.
    #[must_use]
    pub const fn paths(&self) -> &StorePaths {
        &self.paths
    }

    #[cfg(test)]
    pub(crate) fn with_limits(paths: StorePaths, agents: usize, conversations: usize) -> Self {
        let mut store = Self::new(paths);
        store.agents_max = agents;
        store.conversations_per_agent_max = conversations;
        store
    }

    #[cfg(test)]
    pub(crate) fn with_cache_limit(paths: StorePaths, cache_entries: usize) -> Self {
        let mut store = Self::new(paths);
        store.cache = RecordCache::with_limit(cache_entries);
        store
    }

    /// Updates one known agent field while preserving compatible fields and requiring the loaded
    /// filesystem revision at commit.
    ///
    /// # Errors
    /// Returns typed read, validation, conflict, lock, limit, or durable-write failures.
    pub async fn update_agent_name(
        &self,
        id: &AgentId,
        name: lotta_domain::NonEmptyString,
    ) -> Result<Agent, StoreError> {
        self.update_agent_name_observed(id, name, |_| Ok(())).await
    }

    pub(crate) async fn update_agent_name_observed(
        &self,
        id: &AgentId,
        name: lotta_domain::NonEmptyString,
        observer: impl FnOnce(&Path) -> Result<(), StoreError> + Send + 'static,
    ) -> Result<Agent, StoreError> {
        let paths = self.paths.clone();
        let cache = self.cache.clone();
        let id = id.clone();
        run_blocking(Arc::clone(&self.blocking), move || {
            crate::agent::update_name(&paths, &id, name, &cache, observer)
        })
        .await
    }

    /// Archives or unarchives a conversation with explicit-null unarchive semantics.
    ///
    /// # Errors
    /// Returns typed read, identity, conflict, lock, limit, or durable-write failures.
    pub async fn set_conversation_archived(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        archived: bool,
        now: lotta_domain::Timestamp,
    ) -> Result<Conversation, StoreError> {
        let paths = self.paths.clone();
        let cache = self.cache.clone();
        let agent = agent.clone();
        let conversation = conversation.clone();
        run_blocking(Arc::clone(&self.blocking), move || {
            crate::conversation::archive(&paths, &agent, &conversation, archived, now, &cache)
        })
        .await
    }

    /// Sets a conversation model/settings override without touching its owning agent record.
    ///
    /// # Errors
    /// Returns typed read, identity, conflict, lock, limit, or durable-write failures.
    pub async fn set_conversation_model_override(
        &self,
        agent: &AgentId,
        conversation: &ConversationId,
        model: Option<String>,
        settings: lotta_domain::BoundedMap<1_024>,
        now: lotta_domain::Timestamp,
    ) -> Result<Conversation, StoreError> {
        let paths = self.paths.clone();
        let cache = self.cache.clone();
        let agent = agent.clone();
        let conversation = conversation.clone();
        run_blocking(Arc::clone(&self.blocking), move || {
            crate::conversation::model_override(
                &paths,
                &agent,
                &conversation,
                model,
                settings,
                now,
                &cache,
            )
        })
        .await
    }
}

impl AgentStore for LocalStore {
    fn load(&self, id: &AgentId) -> PortFuture<'_, Agent> {
        let path = self.paths.agent_record(id);
        let pool = Arc::clone(&self.blocking);
        let cache = self.cache.clone();
        let expected = id.clone();
        Box::pin(async move {
            let path = path.map_err(RuntimeError::from)?;
            run_blocking(pool, move || {
                crate::agent::load(&path, &expected, &cache).map(|v| v.0)
            })
            .await
            .map_err(Into::into)
        })
    }
    fn list(&self, items: Sender<Agent>, cancellation: CancellationToken) -> PortFuture<'_, ()> {
        let directory = self.paths.agents();
        let pool = Arc::clone(&self.blocking);
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(cancelled("agent list"));
            }
            let mut values = run_blocking(pool, move || load_agents(&directory))
                .await
                .map_err(RuntimeError::from)?;
            values.sort_by(|left, right| left.id.cmp(&right.id));
            send_all(values, items, cancellation, "agent list").await
        })
    }
    fn save(&self, agent: &Agent) -> PortFuture<'_, ()> {
        let path = self.paths.agent_record(&agent.id);
        let value = agent.clone();
        let pool = Arc::clone(&self.blocking);
        let paths = self.paths.clone();
        let cache = self.cache.clone();
        let limit = self.agents_max;
        Box::pin(async move {
            path.map_err(RuntimeError::from)?;
            run_blocking(pool, move || {
                crate::agent::save(&paths, &value, &cache, limit)
            })
            .await
            .map_err(Into::into)
        })
    }
    fn delete(&self, id: &AgentId) -> PortFuture<'_, ()> {
        let path = self.paths.agent_record(id);
        let pool = Arc::clone(&self.blocking);
        Box::pin(async move {
            let path = path.map_err(RuntimeError::from)?;
            let cache = self.cache.clone();
            run_blocking(pool, move || {
                atomic_delete(&path)?;
                cache.invalidate(&path)
            })
            .await
            .map_err(Into::into)
        })
    }
}

impl ConversationStore for LocalStore {
    fn load(&self, agent: &AgentId, conversation: &ConversationId) -> PortFuture<'_, Conversation> {
        let path = self
            .paths
            .conversation_dir(agent, conversation)
            .map(|path| path.join("conversation.json"));
        let expected_agent = agent.clone();
        let expected_conversation = conversation.clone();
        let pool = Arc::clone(&self.blocking);
        let cache = self.cache.clone();
        Box::pin(async move {
            let path = path.map_err(RuntimeError::from)?;
            run_blocking(pool, move || {
                crate::conversation::load(&path, &expected_agent, &expected_conversation, &cache)
                    .map(|v| v.0)
            })
            .await
            .map_err(Into::into)
        })
    }
    fn list_for_agent(
        &self,
        agent: &AgentId,
        items: Sender<Conversation>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ()> {
        let directory = self.paths.conversations();
        let expected_agent = agent.clone();
        let pool = Arc::clone(&self.blocking);
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(cancelled("conversation list"));
            }
            let values = run_blocking(pool, move || {
                load_conversations(&directory, &expected_agent)
            })
            .await
            .map_err(RuntimeError::from)?;
            send_all(values, items, cancellation, "conversation list").await
        })
    }
    fn save(&self, conversation: &Conversation) -> PortFuture<'_, ()> {
        let path = self
            .paths
            .conversation_dir(&conversation.agent_id, &conversation.id)
            .map(|path| path.join("conversation.json"));
        let value = conversation.clone();
        let pool = Arc::clone(&self.blocking);
        let paths = self.paths.clone();
        let cache = self.cache.clone();
        let limit = self.conversations_per_agent_max;
        Box::pin(async move {
            path.map_err(RuntimeError::from)?;
            run_blocking(pool, move || {
                crate::conversation::save(&paths, &value, &cache, limit)
            })
            .await
            .map_err(Into::into)
        })
    }
    fn delete(&self, agent: &AgentId, conversation: &ConversationId) -> PortFuture<'_, ()> {
        let path = self
            .paths
            .conversation_dir(agent, conversation)
            .map(|path| path.join("conversation.json"));
        let expected_agent = agent.clone();
        let pool = Arc::clone(&self.blocking);
        Box::pin(async move {
            let path = path.map_err(RuntimeError::from)?;
            let value: Conversation = run_blocking(Arc::clone(&pool), {
                let path = path.clone();
                move || read_record(&path)
            })
            .await
            .map_err(RuntimeError::from)?;
            if value.agent_id != expected_agent {
                return Err(RuntimeError::NotFound {
                    context: path.display().to_string(),
                });
            }
            let cache = self.cache.clone();
            run_blocking(pool, move || {
                atomic_delete(&path)?;
                cache.invalidate(&path)
            })
            .await
            .map_err(Into::into)
        })
    }
}

pub(crate) async fn run_blocking<T: Send + 'static>(
    pool: Arc<Semaphore>,
    operation: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError> {
    let permit = pool
        .acquire_owned()
        .await
        .map_err(|_| StoreError::new(StoreErrorKind::Io, "blocking-pool"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
    .await
    .map_err(|_| StoreError::new(StoreErrorKind::Io, "blocking-join"))?
}

pub(crate) fn write_record_locked<T: serde::Serialize>(
    path: &Path,
    value: &T,
    revision: &FileRevision,
    lock: &LottaStorageLock,
) -> Result<(), StoreError> {
    let mut sink = BoundedJson::new(path);
    serde_json::to_writer_pretty(&mut sink, value).map_err(|_| sink.error())?;
    sink.write_all(b"\n").map_err(|_| sink.error())?;
    atomic_write_expected_locked(path, sink.as_bytes(), WriteMode::Standard, revision, lock)
}

pub(crate) struct BoundedJson<'a> {
    path: &'a Path,
    bytes: Vec<u8>,
    failed: bool,
}
impl<'a> BoundedJson<'a> {
    pub(crate) fn new(path: &'a Path) -> Self {
        Self {
            path,
            bytes: Vec::new(),
            failed: false,
        }
    }
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    fn error(&self) -> StoreError {
        StoreError::new(
            if self.failed {
                StoreErrorKind::Limit
            } else {
                StoreErrorKind::Parse
            },
            self.path,
        )
    }
}
impl Write for BoundedJson<'_> {
    fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
        let length = self.bytes.len().checked_add(chunk.len()).ok_or_else(|| {
            self.failed = true;
            std::io::Error::other("bounded json")
        })?;
        if length > RECORD_BYTES_MAX {
            self.failed = true;
            return Err(std::io::Error::other("bounded json"));
        }
        self.bytes.try_reserve(chunk.len()).map_err(|_| {
            self.failed = true;
            std::io::Error::other("bounded json")
        })?;
        self.bytes.extend_from_slice(chunk);
        Ok(chunk.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    read_json_inner(path).map_err(StoreError::log)
}

fn reject_duplicate_json_keys(bytes: &[u8], path: &Path) -> Result<(), StoreError> {
    use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
    use std::collections::BTreeSet;
    struct Seed;
    struct JsonVisitor;
    impl<'de> DeserializeSeed<'de> for Seed {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(self, value: D) -> Result<(), D::Error> {
            value.deserialize_any(JsonVisitor)
        }
    }
    impl<'de> Visitor<'de> for JsonVisitor {
        type Value = ();
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("JSON value without duplicate keys")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            let mut keys = BTreeSet::new();
            while let Some(key) = map.next_key::<String>()? {
                if !keys.insert(key) {
                    return Err(serde::de::Error::custom("duplicate object key"));
                }
                map.next_value_seed(Seed)?;
            }
            Ok(())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
            while sequence.next_element_seed(Seed)?.is_some() {}
            Ok(())
        }
        fn visit_bool<E>(self, _: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E>(self, _: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E>(self, _: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E>(self, _: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E>(self, _: &str) -> Result<(), E> {
            Ok(())
        }
        fn visit_none<E>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E>(self) -> Result<(), E> {
            Ok(())
        }
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    Seed.deserialize(&mut deserializer)
        .and_then(|()| deserializer.end())
        .map_err(|_| StoreError::new(StoreErrorKind::Parse, path))
}

fn read_json_inner<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    let root = backend_root(path)?;
    validate_regular_file(root, path)?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| StoreError::from_io(path, &error))?;
    if metadata.len() > RECORD_BYTES_MAX as u64 {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    let length = usize::try_from(metadata.len())
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    let mut file = std::fs::File::open(path).map_err(|error| StoreError::from_io(path, &error))?;
    std::io::Read::take(&mut file, RECORD_BYTES_MAX as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| StoreError::from_io(path, &error))?;
    if bytes.len() != length {
        return Err(StoreError::new(StoreErrorKind::StorageConflict, path));
    }
    reject_duplicate_json_keys(&bytes, path)?;
    serde_json::from_slice(&bytes).map_err(|_| StoreError::new(StoreErrorKind::Parse, path))
}

pub(crate) enum RecordKind {
    Agent,
}

pub(crate) fn count_records(directory: &Path, _kind: RecordKind) -> Result<usize, StoreError> {
    Ok(sorted_files(directory)?.len())
}

pub(crate) fn count_conversations_for_agent(
    directory: &Path,
    agent: &AgentId,
) -> Result<usize, StoreError> {
    let mut count = 0_usize;
    for path in sorted_files(directory)? {
        let record = path.join("conversation.json");
        if !record.is_file() {
            continue;
        }
        let value: Conversation = read_record(&record)?;
        if &value.agent_id == agent {
            count = count
                .checked_add(1)
                .ok_or_else(|| StoreError::new(StoreErrorKind::Limit, directory))?;
        }
    }
    Ok(count)
}

fn push_bounded<T>(values: &mut Vec<T>, value: T, path: &Path) -> Result<(), StoreError> {
    push_bounded_max(values, value, path, LIST_ENTRIES_MAX)
}

pub(crate) fn push_bounded_max<T>(
    values: &mut Vec<T>,
    value: T,
    path: &Path,
    maximum: usize,
) -> Result<(), StoreError> {
    if values.len() >= maximum {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    values
        .try_reserve(1)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    values.push(value);
    Ok(())
}

fn sorted_files(directory: &Path) -> Result<Vec<PathBuf>, StoreError> {
    let root = backend_root(directory)?;
    validate_existing(root, directory)?;
    let read = match std::fs::read_dir(directory) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(StoreError::from_io(directory, &error)),
    };
    let mut paths = Vec::new();
    for entry in read {
        let entry = entry.map_err(|error| StoreError::from_io(directory, &error))?;
        let kind = entry
            .file_type()
            .map_err(|error| StoreError::from_io(&entry.path(), &error))?;
        if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, entry.path()));
        }
        push_bounded(&mut paths, entry.path(), directory)?;
    }
    paths.sort();
    Ok(paths)
}

fn load_conversations(directory: &Path, agent: &AgentId) -> Result<Vec<Conversation>, StoreError> {
    let mut values = Vec::new();
    for path in sorted_files(directory)? {
        if path.file_name().and_then(|value| value.to_str()) == Some(".lotta-storage.lock") {
            continue;
        }
        let directory_metadata =
            std::fs::symlink_metadata(&path).map_err(|error| StoreError::from_io(&path, &error))?;
        if !directory_metadata.is_dir() {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, path));
        }
        let record = path.join("conversation.json");
        match std::fs::symlink_metadata(&record) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(StoreError::from_io(&record, &error)),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(StoreError::new(StoreErrorKind::InvalidPath, record));
            }
            Ok(_) => {}
        }
        let value: Conversation = read_record(&record)?;
        if &value.agent_id == agent {
            push_bounded(&mut values, value, directory)?;
        }
    }
    values.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(values)
}

fn load_agents(directory: &Path) -> Result<Vec<Agent>, StoreError> {
    let mut values = Vec::new();
    for path in sorted_files(directory)? {
        if path.file_name().and_then(|value| value.to_str()) == Some(".lotta-storage.lock") {
            continue;
        }
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| StoreError::from_io(&path, &error))?;
        if metadata.is_dir() || path.extension().and_then(|value| value.to_str()) != Some("json") {
            return Err(StoreError::new(StoreErrorKind::InvalidPath, path));
        }
        let value = read_record(&path)?;
        push_bounded(&mut values, value, directory)?;
    }
    Ok(values)
}

async fn send_all<T>(
    values: Vec<T>,
    items: Sender<T>,
    cancellation: CancellationToken,
    context: &'static str,
) -> Result<(), RuntimeError> {
    for value in values {
        if items.is_closed() {
            return Err(channel_closed(context));
        }
        tokio::select! {
            biased;
            result = items.send(value) => {
                if result.is_err() {
                    return Err(channel_closed(context));
                }
            }
            () = cancellation.cancelled() => return Err(cancelled(context)),
        }
    }
    Ok(())
}
fn channel_closed(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "testkit_channel_closed",
        context: context.into(),
    }
}
fn cancelled(context: &'static str) -> RuntimeError {
    RuntimeError::Cancelled {
        context: context.into(),
    }
}

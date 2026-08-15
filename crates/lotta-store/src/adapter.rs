use crate::confinement::{backend_root, validate_existing, validate_regular_file};
use crate::{StoreError, StoreErrorKind, StorePaths, WriteMode, atomic_delete, atomic_write};
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
}

impl LocalStore {
    /// Creates an adapter for an already validated local-backend layout.
    #[must_use]
    pub fn new(paths: StorePaths) -> Self {
        let blocking = Arc::clone(
            BLOCKING_POOL.get_or_init(|| Arc::new(Semaphore::new(LOCAL_STORE_BLOCKING_MAX))),
        );
        Self { paths, blocking }
    }
    /// Returns the adapter's storage layout.
    #[must_use]
    pub const fn paths(&self) -> &StorePaths {
        &self.paths
    }
}

impl AgentStore for LocalStore {
    fn load(&self, id: &AgentId) -> PortFuture<'_, Agent> {
        let path = self.paths.agent_record(id);
        let pool = Arc::clone(&self.blocking);
        Box::pin(async move {
            let path = path.map_err(RuntimeError::from)?;
            run_blocking(pool, move || read_json(&path))
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
        Box::pin(async move {
            let path = path.map_err(RuntimeError::from)?;
            run_blocking(pool, move || write_json(&path, &value))
                .await
                .map_err(Into::into)
        })
    }
    fn delete(&self, id: &AgentId) -> PortFuture<'_, ()> {
        let path = self.paths.agent_record(id);
        let pool = Arc::clone(&self.blocking);
        Box::pin(async move {
            let path = path.map_err(RuntimeError::from)?;
            run_blocking(pool, move || atomic_delete(&path))
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
        let pool = Arc::clone(&self.blocking);
        Box::pin(async move {
            let path = path.map_err(RuntimeError::from)?;
            let value: Conversation = run_blocking(pool, {
                let path = path.clone();
                move || read_json(&path)
            })
            .await
            .map_err(RuntimeError::from)?;
            if value.agent_id != expected_agent {
                return Err(RuntimeError::NotFound {
                    context: path.display().to_string(),
                });
            }
            Ok(value)
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
        Box::pin(async move {
            let path = path.map_err(RuntimeError::from)?;
            run_blocking(pool, move || write_json(&path, &value))
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
                move || read_json(&path)
            })
            .await
            .map_err(RuntimeError::from)?;
            if value.agent_id != expected_agent {
                return Err(RuntimeError::NotFound {
                    context: path.display().to_string(),
                });
            }
            run_blocking(pool, move || atomic_delete(&path))
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

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let mut sink = BoundedJson::new(path);
    serde_json::to_writer_pretty(&mut sink, value).map_err(|_| sink.error())?;
    atomic_write(path, sink.as_bytes(), WriteMode::Standard)
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

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    read_json_inner(path).map_err(StoreError::log)
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
    serde_json::from_slice(&bytes).map_err(|_| StoreError::new(StoreErrorKind::Parse, path))
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
        let value: Conversation = read_json(&record)?;
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
        let value = read_json(&path)?;
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

//! Serialized asynchronous adapter operations and bounded streams.

use crate::{fs, repo, worktree};
use cap_std::ambient_authority;
use cap_std::fs::Dir;
use lotta_domain::AgentId;
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{
    CommitMessage, DiffChunk, InitialMemoryBlocks, MemoryFileContent, RepositoryPath, RevisionId,
    WorktreeId,
};
use lotta_runtime::ports::{
    MemFsCommitAuthor, MemFsHistoryEntry, MemFsMutation, MemFsPort, MemFsStatus,
    MemFsTransactionResult, MemFsTreeEntry, PortFuture,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;

fn global_lock() -> Arc<AsyncMutex<()>> {
    static LOCK: OnceLock<Arc<AsyncMutex<()>>> = OnceLock::new();
    Arc::clone(LOCK.get_or_init(|| Arc::new(AsyncMutex::new(()))))
}

fn adapter(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "memfs_adapter",
        context: context.into(),
    }
}

async fn blocking_unlocked<T, F>(function: F) -> Result<T, RuntimeError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, RuntimeError> + Send + 'static,
{
    tokio::task::spawn_blocking(function)
        .await
        .map_err(|_| adapter("memfs blocking task"))?
}

async fn locked<T, F>(function: F) -> Result<T, RuntimeError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, RuntimeError> + Send + 'static,
{
    let _guard = global_lock().lock_owned().await;
    blocking_unlocked(function).await
}

/// Cloneable Git-backed memory-filesystem adapter rooted at an explicit backend directory.
#[derive(Clone)]
pub struct GitMemFs {
    root: Arc<PathBuf>,
    backend: Arc<Dir>,
    worktrees: worktree::Worktrees,
}

impl GitMemFs {
    /// Validates and retains one absolute, existing, non-symlink backend root.
    ///
    /// # Errors
    /// Returns stable validation or adapter errors for relative, non-UTF8, missing,
    /// or symlink roots.
    pub fn new(root: PathBuf) -> Result<Self, RuntimeError> {
        if !root.is_absolute() || root.to_str().is_none() {
            return Err(RuntimeError::InvalidData {
                context: "absolute backend root".into(),
            });
        }
        let metadata = std::fs::symlink_metadata(&root).map_err(|_| RuntimeError::NotFound {
            context: "backend root".into(),
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(RuntimeError::InvalidData {
                context: "backend root".into(),
            });
        }
        let canonical = std::fs::canonicalize(root).map_err(|_| RuntimeError::NotFound {
            context: "backend root".into(),
        })?;
        let backend = Dir::open_ambient_dir(&canonical, ambient_authority()).map_err(|_| {
            RuntimeError::PermissionDenied {
                context: "backend root".into(),
            }
        })?;
        Ok(Self {
            root: Arc::new(canonical),
            backend: Arc::new(backend),
            worktrees: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    pub(crate) fn repo(&self, agent: &AgentId) -> Result<PathBuf, RuntimeError> {
        repo::repository_path(&self.root, agent)
    }

    #[cfg(test)]
    pub(crate) fn repo_dir_for_test(&self, agent: &AgentId) -> Result<Dir, RuntimeError> {
        open_repo_dir(&self.backend, agent)
    }

    #[cfg(test)]
    pub(crate) fn root_path(&self) -> &PathBuf {
        &self.root
    }

    #[cfg(test)]
    pub(crate) fn worktree_path(&self, id: &WorktreeId) -> Result<PathBuf, RuntimeError> {
        worktree::path_for_test(&self.worktrees, id)
    }
}

fn open_repo_dir(backend: &Dir, agent: &AgentId) -> Result<Dir, RuntimeError> {
    repo::validate_agent_id(agent)?;
    backend
        .open_dir(PathBuf::from("memfs").join(agent.as_str()).join("memory"))
        .map_err(|_| RuntimeError::NotFound {
            context: "memfs repository".into(),
        })
}

fn cancelled(context: &'static str) -> RuntimeError {
    RuntimeError::Cancelled {
        context: context.into(),
    }
}

fn check_cancelled(
    cancellation: &CancellationToken,
    context: &'static str,
) -> Result<(), RuntimeError> {
    if cancellation.is_cancelled() {
        return Err(cancelled(context));
    }
    Ok(())
}

async fn stream_value<T: Send>(
    sender: &Sender<T>,
    value: T,
    cancellation: &CancellationToken,
    context: &'static str,
) -> Result<(), RuntimeError> {
    check_cancelled(cancellation, context)?;
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(cancelled(context)),
        result = sender.send(value) => result.map_err(|_| adapter(context)),
    }
}

fn boxed<'a, T>(
    future: impl Future<Output = Result<T, RuntimeError>> + Send + 'a,
) -> PortFuture<'a, T> {
    Box::pin(future)
}

async fn stream_tree(
    path: Result<PathBuf, RuntimeError>,
    backend: Arc<Dir>,
    agent: AgentId,
    revision: Option<RevisionId>,
    items: Sender<MemFsTreeEntry>,
    cancellation: CancellationToken,
) -> Result<(), RuntimeError> {
    const CONTEXT: &str = "memfs tree";
    check_cancelled(&cancellation, CONTEXT)?;
    let _guard = global_lock().lock_owned().await;
    check_cancelled(&cancellation, CONTEXT)?;
    let paths = blocking_unlocked(move || {
        let directory = open_repo_dir(&backend, &agent)?;
        repo::paths(&path?, &directory, revision.as_ref())
    })
    .await?;
    for path in paths {
        stream_value(&items, MemFsTreeEntry { path }, &cancellation, CONTEXT).await?;
    }
    Ok(())
}

async fn stream_history(
    path: Result<PathBuf, RuntimeError>,
    items: Sender<MemFsHistoryEntry>,
    cancellation: CancellationToken,
) -> Result<(), RuntimeError> {
    const CONTEXT: &str = "memfs history";
    check_cancelled(&cancellation, CONTEXT)?;
    let _guard = global_lock().lock_owned().await;
    check_cancelled(&cancellation, CONTEXT)?;
    let values = blocking_unlocked(move || repo::history(&path?)).await?;
    for value in values {
        stream_value(&items, value, &cancellation, CONTEXT).await?;
    }
    Ok(())
}

struct DiffState {
    repo: PathBuf,
    directory: Arc<Dir>,
    source: repo::Snapshot,
    target: repo::Snapshot,
    source_paths: Vec<RepositoryPath>,
    target_paths: Vec<RepositoryPath>,
}

async fn prepare_diff(
    path: Result<PathBuf, RuntimeError>,
    backend: Arc<Dir>,
    agent: AgentId,
    source: Option<RevisionId>,
    target: Option<RevisionId>,
) -> Result<DiffState, RuntimeError> {
    blocking_unlocked(move || {
        let path = path?;
        let directory = Arc::new(open_repo_dir(&backend, &agent)?);
        let source = repo::snapshot_descriptor(&path, source.as_ref())?;
        let target = repo::snapshot_descriptor(&path, target.as_ref())?;
        let source_paths = repo::snapshot_paths(&path, &directory, &source)?;
        let target_paths = repo::snapshot_paths(&path, &directory, &target)?;
        Ok(DiffState {
            repo: path,
            directory,
            source,
            target,
            source_paths,
            target_paths,
        })
    })
    .await
}

fn next_path(
    source: &[RepositoryPath],
    target: &[RepositoryPath],
    source_index: &mut usize,
    target_index: &mut usize,
) -> Option<RepositoryPath> {
    match (source.get(*source_index), target.get(*target_index)) {
        (Some(old), Some(new)) if old < new => {
            *source_index += 1;
            Some(old.clone())
        }
        (Some(old), Some(new)) if old > new => {
            *target_index += 1;
            Some(new.clone())
        }
        (Some(old), Some(_)) => {
            *source_index += 1;
            *target_index += 1;
            Some(old.clone())
        }
        (Some(old), None) => {
            *source_index += 1;
            Some(old.clone())
        }
        (None, Some(new)) => {
            *target_index += 1;
            Some(new.clone())
        }
        (None, None) => None,
    }
}

async fn load_diff_file(
    state: &DiffState,
    path: &RepositoryPath,
    cancellation: &CancellationToken,
) -> Result<(Option<MemoryFileContent>, Option<MemoryFileContent>), RuntimeError> {
    check_cancelled(cancellation, "memfs diff")?;
    let repo = state.repo.clone();
    let directory = Arc::clone(&state.directory);
    let source = state.source.clone();
    let target = state.target.clone();
    let path = path.clone();
    blocking_unlocked(move || {
        let old = repo::file_in_snapshot(&repo, &directory, &source, &path)?;
        let new = repo::file_in_snapshot(&repo, &directory, &target, &path)?;
        let combined = old
            .as_ref()
            .map_or(0, |value| value.as_slice().len())
            .checked_add(new.as_ref().map_or(0, |value| value.as_slice().len()))
            .ok_or_else(|| RuntimeError::LimitExceeded {
                context: "memfs diff file bytes".into(),
            })?;
        let maximum = crate::MEMORY_FILE_BYTES_MAX
            .value
            .checked_mul(2)
            .ok_or_else(|| RuntimeError::LimitExceeded {
                context: "memfs diff file bytes".into(),
            })?;
        if combined > maximum {
            return Err(RuntimeError::LimitExceeded {
                context: "memfs diff file bytes".into(),
            });
        }
        Ok((old, new))
    })
    .await
}

async fn send_diff_file(
    sender: &Sender<DiffChunk>,
    cancellation: &CancellationToken,
    path: &RepositoryPath,
    old: Option<&MemoryFileContent>,
    new: Option<&MemoryFileContent>,
) -> Result<(), RuntimeError> {
    let chunks = diff_file(path, old, new)?;
    for chunk in chunks {
        stream_value(sender, chunk, cancellation, "memfs diff").await?;
    }
    Ok(())
}

async fn stream_diff(
    path: Result<PathBuf, RuntimeError>,
    backend: Arc<Dir>,
    agent: AgentId,
    source: Option<RevisionId>,
    target: Option<RevisionId>,
    chunks: Sender<DiffChunk>,
    cancellation: CancellationToken,
) -> Result<(), RuntimeError> {
    const CONTEXT: &str = "memfs diff";
    check_cancelled(&cancellation, CONTEXT)?;
    let _guard: OwnedMutexGuard<()> = global_lock().lock_owned().await;
    check_cancelled(&cancellation, CONTEXT)?;
    let state = prepare_diff(path, backend, agent, source, target).await?;
    let mut source_index = 0;
    let mut target_index = 0;
    while source_index < state.source_paths.len() || target_index < state.target_paths.len() {
        check_cancelled(&cancellation, CONTEXT)?;
        let path = next_path(
            &state.source_paths,
            &state.target_paths,
            &mut source_index,
            &mut target_index,
        )
        .ok_or_else(|| adapter(CONTEXT))?;
        let (old, new) = load_diff_file(&state, &path, &cancellation).await?;
        if old != new {
            send_diff_file(&chunks, &cancellation, &path, old.as_ref(), new.as_ref()).await?;
        }
    }
    Ok(())
}

impl MemFsPort for GitMemFs {
    fn initialize(
        &self,
        agent_id: &AgentId,
        blocks: &InitialMemoryBlocks,
    ) -> PortFuture<'_, RevisionId> {
        let root = Arc::clone(&self.root);
        let backend = Arc::clone(&self.backend);
        let agent = agent_id.clone();
        let blocks = blocks.clone();
        boxed(
            async move { locked(move || repo::initialize(&root, &backend, &agent, &blocks)).await },
        )
    }

    fn status(&self, agent_id: &AgentId) -> PortFuture<'_, MemFsStatus> {
        let path = self.repo(agent_id);
        boxed(async move {
            locked(move || {
                let repo = path?;
                if std::fs::symlink_metadata(&repo).is_err() {
                    return Err(RuntimeError::NotFound {
                        context: "memfs repository".into(),
                    });
                }
                let revision = repo::head(&repo)?;
                let output =
                    crate::git::checked(&repo, ["status", "--porcelain=v1", "-z"], "git status")?;
                Ok(MemFsStatus {
                    revision: Some(revision),
                    dirty: !output.is_empty(),
                })
            })
            .await
        })
    }

    fn tree(
        &self,
        agent_id: &AgentId,
        revision: Option<&RevisionId>,
        items: Sender<MemFsTreeEntry>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ()> {
        let path = self.repo(agent_id);
        let backend = Arc::clone(&self.backend);
        let agent = agent_id.clone();
        let revision = revision.cloned();
        boxed(stream_tree(
            path,
            backend,
            agent,
            revision,
            items,
            cancellation,
        ))
    }

    fn read(&self, agent_id: &AgentId, path: &RepositoryPath) -> PortFuture<'_, MemoryFileContent> {
        let backend = Arc::clone(&self.backend);
        let agent = agent_id.clone();
        let path = path.clone();
        boxed(async move {
            locked(move || fs::read_file(&open_repo_dir(&backend, &agent)?, &path)).await
        })
    }

    fn write(
        &self,
        agent_id: &AgentId,
        path: &RepositoryPath,
        contents: &MemoryFileContent,
    ) -> PortFuture<'_, ()> {
        let backend = Arc::clone(&self.backend);
        let agent = agent_id.clone();
        let path = path.clone();
        let contents = contents.clone();
        boxed(async move {
            locked(move || fs::write_file(&open_repo_dir(&backend, &agent)?, &path, &contents))
                .await
        })
    }

    fn delete(&self, agent_id: &AgentId, path: &RepositoryPath) -> PortFuture<'_, ()> {
        let backend = Arc::clone(&self.backend);
        let agent = agent_id.clone();
        let path = path.clone();
        boxed(async move {
            locked(move || fs::delete_file(&open_repo_dir(&backend, &agent)?, &path)).await
        })
    }

    fn rename(
        &self,
        agent_id: &AgentId,
        source: &RepositoryPath,
        target: &RepositoryPath,
    ) -> PortFuture<'_, ()> {
        let backend = Arc::clone(&self.backend);
        let agent = agent_id.clone();
        let source = source.clone();
        let target = target.clone();
        boxed(async move {
            locked(move || fs::rename_file(&open_repo_dir(&backend, &agent)?, &source, &target))
                .await
        })
    }

    fn history(
        &self,
        agent_id: &AgentId,
        items: Sender<MemFsHistoryEntry>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ()> {
        boxed(stream_history(self.repo(agent_id), items, cancellation))
    }

    fn file_at_revision(
        &self,
        agent_id: &AgentId,
        path: &RepositoryPath,
        revision: &RevisionId,
    ) -> PortFuture<'_, MemoryFileContent> {
        let repo = self.repo(agent_id);
        let path = path.clone();
        let revision = revision.clone();
        boxed(async move { locked(move || repo::file_at_revision(&repo?, &path, &revision)).await })
    }

    fn diff(
        &self,
        agent_id: &AgentId,
        source: Option<&RevisionId>,
        target: Option<&RevisionId>,
        chunks: Sender<DiffChunk>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ()> {
        boxed(stream_diff(
            self.repo(agent_id),
            Arc::clone(&self.backend),
            agent_id.clone(),
            source.cloned(),
            target.cloned(),
            chunks,
            cancellation,
        ))
    }

    fn commit(&self, agent_id: &AgentId, message: &CommitMessage) -> PortFuture<'_, RevisionId> {
        let path = self.repo(agent_id);
        let message = message.clone();
        boxed(async move { locked(move || repo::commit(&path?, message.as_str())).await })
    }

    fn transact(
        &self,
        agent_id: &AgentId,
        mutations: &[MemFsMutation],
        message: &CommitMessage,
        author: &MemFsCommitAuthor,
    ) -> PortFuture<'_, MemFsTransactionResult> {
        let path = self.repo(agent_id);
        let backend = Arc::clone(&self.backend);
        let agent = agent_id.clone();
        let mutations = mutations.to_vec();
        let message = message.clone();
        let author = author.clone();
        boxed(async move {
            locked(move || {
                let path = path?;
                let directory = open_repo_dir(&backend, &agent)?;
                repo::transact(&path, &directory, &mutations, message.as_str(), &author)
            })
            .await
        })
    }

    fn create_worktree(&self, agent_id: &AgentId) -> PortFuture<'_, WorktreeId> {
        let path = self.repo(agent_id);
        let records = Arc::clone(&self.worktrees);
        boxed(async move { locked(move || worktree::create(&path?, &records)).await })
    }

    fn merge_worktree(
        &self,
        agent_id: &AgentId,
        worktree_id: &WorktreeId,
        message: &CommitMessage,
    ) -> PortFuture<'_, RevisionId> {
        let path = self.repo(agent_id);
        let id = worktree_id.clone();
        let message = message.clone();
        let records = Arc::clone(&self.worktrees);
        boxed(async move { locked(move || worktree::merge(&path?, &id, &message, &records)).await })
    }
}

fn allocation_limit() -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: "memfs diff bytes".into(),
    }
}

fn diff_file(
    path: &RepositoryPath,
    old: Option<&MemoryFileContent>,
    new: Option<&MemoryFileContent>,
) -> Result<Vec<DiffChunk>, RuntimeError> {
    let status = match (old, new) {
        (None, Some(_)) => "added",
        (Some(_), None) => "deleted",
        _ => "modified",
    };
    let path = path
        .as_path()
        .to_str()
        .ok_or_else(|| RuntimeError::InvalidData {
            context: "memfs diff path".into(),
        })?;
    let header = path
        .len()
        .checked_add(status.len())
        .and_then(|value| value.checked_add(2))
        .ok_or_else(allocation_limit)?;
    let old_len = optional_line_len(old)?;
    let new_len = optional_line_len(new)?;
    let total = header
        .checked_add(old_len)
        .and_then(|value| value.checked_add(new_len))
        .ok_or_else(allocation_limit)?;
    let mut bytes = Vec::new();
    bytes.try_reserve(total).map_err(|_| allocation_limit())?;
    bytes.extend_from_slice(path.as_bytes());
    bytes.push(b' ');
    bytes.extend_from_slice(status.as_bytes());
    bytes.push(b'\n');
    append_diff_line(&mut bytes, b'-', old.map(MemoryFileContent::as_slice));
    append_diff_line(&mut bytes, b'+', new.map(MemoryFileContent::as_slice));
    chunks(&bytes)
}

fn optional_line_len(value: Option<&MemoryFileContent>) -> Result<usize, RuntimeError> {
    value.map_or(Ok(0), |value| {
        value
            .as_slice()
            .len()
            .checked_add(3)
            .ok_or_else(allocation_limit)
    })
}

fn chunks(bytes: &[u8]) -> Result<Vec<DiffChunk>, RuntimeError> {
    let maximum = lotta_runtime::bounds::MEMFS_DIFF_CHUNK_BYTES_MAX.value;
    let count = bytes
        .len()
        .checked_add(maximum - 1)
        .ok_or_else(allocation_limit)?
        / maximum;
    let mut values = Vec::new();
    values.try_reserve(count).map_err(|_| allocation_limit())?;
    for part in bytes.chunks(maximum) {
        let mut value = Vec::new();
        value
            .try_reserve(part.len())
            .map_err(|_| allocation_limit())?;
        value.extend_from_slice(part);
        values.push(DiffChunk::new(value)?);
    }
    Ok(values)
}

fn append_diff_line(output: &mut Vec<u8>, prefix: u8, value: Option<&[u8]>) {
    if let Some(value) = value {
        output.push(prefix);
        output.push(b' ');
        output.extend_from_slice(value);
        output.push(b'\n');
    }
}

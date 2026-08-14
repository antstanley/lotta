//! Store and `MemFS` fakes.

use super::{cancelled, limit, lock, not_found};
use crate::TESTKIT_ITEMS_MAX;
use lotta_domain::{
    Agent, AgentId, Conversation, ConversationId, TranscriptEntry, TranscriptManifest,
};
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{
    CommitMessage, DiffChunk, InitialMemoryBlocks, MemoryFileContent, RepositoryPath, RevisionId,
    WorktreeId,
};
use lotta_runtime::bounds::{MEMFS_DIFF_CHUNK_BYTES_MAX, MEMORY_FILES_MAX};
use lotta_runtime::ports::{
    AgentStore, ConversationStore, MemFsHistoryEntry, MemFsPort, MemFsStatus, MemFsTreeEntry,
    TranscriptItem, TranscriptStore,
};
use std::collections::BTreeMap;
use std::sync::Mutex;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

const MEMFS_RETAINED_BYTES_MAX: usize = 64 * 1024 * 1024;

/// Bounded in-memory agent store preserving exact entities and ID ordering.
#[derive(Debug, Default)]
pub struct FakeAgentStore {
    records: Mutex<BTreeMap<AgentId, Agent>>,
}

impl AgentStore for FakeAgentStore {
    fn load(&self, id: &AgentId) -> lotta_runtime::ports::PortFuture<'_, Agent> {
        let id = id.clone();
        Box::pin(async move {
            lock(&self.records)
                .get(&id)
                .cloned()
                .ok_or_else(|| not_found("agent"))
        })
    }

    fn list(
        &self,
        items: Sender<Agent>,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let values = bounded_values(
            lock(&self.records).values().cloned(),
            "fake_agent_store_items_max",
        );
        Box::pin(async move { send_all(values?, items, cancellation, "agent list").await })
    }

    fn save(&self, agent: &Agent) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let agent = agent.clone();
        Box::pin(async move {
            let mut records = lock(&self.records);
            if !records.contains_key(&agent.id) && records.len() >= TESTKIT_ITEMS_MAX {
                return Err(limit("fake_agent_store_items_max"));
            }
            records.insert(agent.id.clone(), agent);
            Ok(())
        })
    }

    fn delete(&self, id: &AgentId) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let id = id.clone();
        Box::pin(async move {
            lock(&self.records)
                .remove(&id)
                .map(|_| ())
                .ok_or_else(|| not_found("agent"))
        })
    }
}

/// Bounded in-memory conversation store preserving complete agent scope and ID order.
#[derive(Debug, Default)]
pub struct FakeConversationStore {
    records: Mutex<BTreeMap<(AgentId, ConversationId), Conversation>>,
}

impl ConversationStore for FakeConversationStore {
    fn load(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
    ) -> lotta_runtime::ports::PortFuture<'_, Conversation> {
        let key = (agent_id.clone(), conversation_id.clone());
        Box::pin(async move {
            lock(&self.records)
                .get(&key)
                .cloned()
                .ok_or_else(|| not_found("conversation"))
        })
    }

    fn list_for_agent(
        &self,
        agent_id: &AgentId,
        items: Sender<Conversation>,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let values = bounded_values(
            lock(&self.records)
                .iter()
                .filter(|((owner, _), _)| owner == agent_id)
                .map(|(_, value)| value.clone()),
            "fake_conversation_store_items_max",
        );
        Box::pin(async move { send_all(values?, items, cancellation, "conversation list").await })
    }

    fn save(&self, conversation: &Conversation) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let conversation = conversation.clone();
        Box::pin(async move {
            let key = (conversation.agent_id.clone(), conversation.id.clone());
            let mut records = lock(&self.records);
            if !records.contains_key(&key) && records.len() >= TESTKIT_ITEMS_MAX {
                return Err(limit("fake_conversation_store_items_max"));
            }
            records.insert(key, conversation);
            Ok(())
        })
    }

    fn delete(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let key = (agent_id.clone(), conversation_id.clone());
        Box::pin(async move {
            lock(&self.records)
                .remove(&key)
                .map(|_| ())
                .ok_or_else(|| not_found("conversation"))
        })
    }
}

#[derive(Debug)]
struct TranscriptRecord {
    manifest: TranscriptManifest,
    entries: Vec<TranscriptEntry>,
}

/// Bounded manifest-first append-only transcript store.
#[derive(Debug, Default)]
pub struct FakeTranscriptStore {
    records: Mutex<BTreeMap<(AgentId, ConversationId), TranscriptRecord>>,
}

impl TranscriptStore for FakeTranscriptStore {
    fn load(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
        items: Sender<TranscriptItem>,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let key = (agent_id.clone(), conversation_id.clone());
        let values = lock(&self.records).get(&key).map(|record| {
            let entries = bounded_values(
                record.entries.iter().cloned().map(TranscriptItem::Entry),
                "fake_transcript_entries_max",
            )?;
            let mut values = Vec::with_capacity(entries.len() + 1);
            values.push(TranscriptItem::Manifest(record.manifest.clone()));
            values.extend(entries);
            Ok(values)
        });
        Box::pin(async move {
            let values = values.ok_or_else(|| not_found("transcript"))??;
            send_all(values, items, cancellation, "transcript load").await
        })
    }

    fn initialize(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
        manifest: &TranscriptManifest,
        session: &TranscriptEntry,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let key = (agent_id.clone(), conversation_id.clone());
        let record = TranscriptRecord {
            manifest: manifest.clone(),
            entries: vec![session.clone()],
        };
        Box::pin(async move {
            let mut records = lock(&self.records);
            if records.contains_key(&key) {
                return Err(RuntimeError::Conflict {
                    context: "transcript".into(),
                });
            }
            if records.len() >= TESTKIT_ITEMS_MAX {
                return Err(limit("fake_transcript_store_items_max"));
            }
            records.insert(key, record);
            Ok(())
        })
    }

    fn append(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
        entry: &TranscriptEntry,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let key = (agent_id.clone(), conversation_id.clone());
        let entry = entry.clone();
        Box::pin(async move {
            let mut records = lock(&self.records);
            let record = records
                .get_mut(&key)
                .ok_or_else(|| not_found("transcript"))?;
            if record.entries.len() >= TESTKIT_ITEMS_MAX {
                return Err(limit("fake_transcript_entries_max"));
            }
            record.entries.push(entry);
            Ok(())
        })
    }
}

#[derive(Clone, Debug)]
struct Snapshot {
    revision: RevisionId,
    summary: CommitMessage,
    files: BTreeMap<RepositoryPath, MemoryFileContent>,
}

#[derive(Debug, Default)]
struct Repository {
    files: BTreeMap<RepositoryPath, MemoryFileContent>,
    history: Vec<Snapshot>,
    worktrees: BTreeMap<WorktreeId, BTreeMap<RepositoryPath, MemoryFileContent>>,
    revision_sequence: u64,
    worktree_sequence: u64,
}

/// In-memory implementation of all thirteen `MemFS` operations without disk or Git.
///
/// Worktrees are isolated immutable snapshots because the port intentionally exposes no worktree
/// mutation operation. A successful merge installs that snapshot, records a merge commit, and
/// consumes the opaque identifier; failed validation or lookup leaves it retained.
#[derive(Debug, Default)]
pub struct FakeMemFs {
    repositories: Mutex<BTreeMap<AgentId, Repository>>,
}

impl FakeMemFs {
    fn next_revision(repository: &Repository) -> Result<(u64, RevisionId), RuntimeError> {
        let sequence = repository
            .revision_sequence
            .checked_add(1)
            .ok_or_else(|| limit("fake_memfs_revision_sequence"))?;
        let revision = RevisionId::new(format!("testkit-revision-{sequence:016}"))?;
        Ok((sequence, revision))
    }

    fn snapshot<'a>(
        repository: &'a Repository,
        revision: Option<&RevisionId>,
    ) -> Result<&'a BTreeMap<RepositoryPath, MemoryFileContent>, RuntimeError> {
        match revision {
            None => Ok(&repository.files),
            Some(revision) => repository
                .history
                .iter()
                .find(|item| &item.revision == revision)
                .map(|item| &item.files)
                .ok_or_else(|| not_found("memfs revision")),
        }
    }

    fn commit_snapshot(
        repository: &mut Repository,
        summary: CommitMessage,
        files: BTreeMap<RepositoryPath, MemoryFileContent>,
    ) -> Result<RevisionId, RuntimeError> {
        Self::check_snapshot_retention(repository, &files, true, 0)?;
        let (sequence, revision) = Self::next_revision(repository)?;
        repository.history.push(Snapshot {
            revision: revision.clone(),
            summary,
            files,
        });
        repository.revision_sequence = sequence;
        Ok(revision)
    }

    fn map_bytes(
        files: &BTreeMap<RepositoryPath, MemoryFileContent>,
    ) -> Result<usize, RuntimeError> {
        if files.len() > MEMORY_FILES_MAX.value {
            return Err(limit("fake_memfs_files_max"));
        }
        files.values().try_fold(0usize, |total, value| {
            total
                .checked_add(value.as_slice().len())
                .ok_or_else(|| limit("fake_memfs_retained_bytes_max"))
        })
    }

    fn retained_bytes(repository: &Repository) -> Result<usize, RuntimeError> {
        let current = Self::map_bytes(&repository.files)?;
        let history = repository
            .history
            .iter()
            .try_fold(0usize, |total, snapshot| {
                total
                    .checked_add(Self::map_bytes(&snapshot.files)?)
                    .ok_or_else(|| limit("fake_memfs_retained_bytes_max"))
            })?;
        repository.worktrees.values().try_fold(
            current
                .checked_add(history)
                .ok_or_else(|| limit("fake_memfs_retained_bytes_max"))?,
            |total, files| {
                total
                    .checked_add(Self::map_bytes(files)?)
                    .ok_or_else(|| limit("fake_memfs_retained_bytes_max"))
            },
        )
    }

    fn check_snapshot_retention(
        repository: &Repository,
        files: &BTreeMap<RepositoryPath, MemoryFileContent>,
        add_history: bool,
        removed_bytes: usize,
    ) -> Result<(), RuntimeError> {
        if add_history && repository.history.len() >= TESTKIT_ITEMS_MAX {
            return Err(limit("fake_memfs_history_max"));
        }
        let map_bytes = Self::map_bytes(files)?;
        let retained = Self::retained_bytes(repository)?
            .checked_sub(removed_bytes)
            .and_then(|value| value.checked_add(map_bytes))
            .ok_or_else(|| limit("fake_memfs_retained_bytes_max"))?;
        if retained > MEMFS_RETAINED_BYTES_MAX {
            return Err(limit("fake_memfs_retained_bytes_max"));
        }
        Ok(())
    }
}

impl MemFsPort for FakeMemFs {
    fn initialize(
        &self,
        agent_id: &AgentId,
        blocks: &InitialMemoryBlocks,
    ) -> lotta_runtime::ports::PortFuture<'_, RevisionId> {
        let agent_id = agent_id.clone();
        let blocks = blocks.clone();
        Box::pin(async move {
            if blocks.as_slice().len() > TESTKIT_ITEMS_MAX {
                return Err(limit("fake_memfs_initial_files_max"));
            }
            let mut files = BTreeMap::new();
            for block in blocks.as_slice() {
                let path =
                    RepositoryPath::new(format!("system/{}.md", block.label().as_str()).into())?;
                if files.insert(path, block.value().clone()).is_some() {
                    return Err(RuntimeError::Conflict {
                        context: "memfs initial path".into(),
                    });
                }
            }
            let mut repository = Repository::default();
            let summary = CommitMessage::new("initialize".into())?;
            let revision = Self::commit_snapshot(&mut repository, summary, files.clone())?;
            repository.files = files;
            let mut repositories = lock(&self.repositories);
            if repositories.contains_key(&agent_id) {
                return Err(RuntimeError::Conflict {
                    context: "memfs".into(),
                });
            }
            if repositories.len() >= TESTKIT_ITEMS_MAX {
                return Err(limit("fake_memfs_repositories_max"));
            }
            repositories.insert(agent_id, repository);
            Ok(revision)
        })
    }

    fn status(&self, agent_id: &AgentId) -> lotta_runtime::ports::PortFuture<'_, MemFsStatus> {
        let result = lock(&self.repositories)
            .get(agent_id)
            .map(|repository| {
                let latest = repository.history.last();
                MemFsStatus {
                    revision: latest.map(|item| item.revision.clone()),
                    dirty: latest.is_none_or(|item| item.files != repository.files),
                }
            })
            .ok_or_else(|| not_found("memfs"));
        Box::pin(async move { result })
    }

    fn tree(
        &self,
        agent_id: &AgentId,
        revision: Option<&RevisionId>,
        items: Sender<MemFsTreeEntry>,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let values = lock(&self.repositories)
            .get(agent_id)
            .ok_or_else(|| not_found("memfs"))
            .and_then(|repository| Self::snapshot(repository, revision))
            .and_then(|files| {
                bounded_values(
                    files.keys().cloned().map(|path| MemFsTreeEntry { path }),
                    "fake_memfs_tree_items_max",
                )
            });
        Box::pin(async move { send_all(values?, items, cancellation, "memfs tree").await })
    }

    fn read(
        &self,
        agent_id: &AgentId,
        path: &RepositoryPath,
    ) -> lotta_runtime::ports::PortFuture<'_, MemoryFileContent> {
        let result = lock(&self.repositories)
            .get(agent_id)
            .ok_or_else(|| not_found("memfs"))
            .and_then(|repository| {
                repository
                    .files
                    .get(path)
                    .cloned()
                    .ok_or_else(|| not_found("memfs file"))
            });
        Box::pin(async move { result })
    }

    fn write(
        &self,
        agent_id: &AgentId,
        path: &RepositoryPath,
        contents: &MemoryFileContent,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let agent_id = agent_id.clone();
        let path = path.clone();
        let contents = contents.clone();
        Box::pin(async move {
            let mut repositories = lock(&self.repositories);
            let repository = repositories
                .get_mut(&agent_id)
                .ok_or_else(|| not_found("memfs"))?;
            if !repository.files.contains_key(&path) && repository.files.len() >= TESTKIT_ITEMS_MAX
            {
                return Err(limit("fake_memfs_files_max"));
            }
            repository.files.insert(path, contents);
            Ok(())
        })
    }

    fn delete(
        &self,
        agent_id: &AgentId,
        path: &RepositoryPath,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let agent_id = agent_id.clone();
        let path = path.clone();
        Box::pin(async move {
            lock(&self.repositories)
                .get_mut(&agent_id)
                .ok_or_else(|| not_found("memfs"))?
                .files
                .remove(&path)
                .map(|_| ())
                .ok_or_else(|| not_found("memfs file"))
        })
    }

    fn rename(
        &self,
        agent_id: &AgentId,
        source: &RepositoryPath,
        target: &RepositoryPath,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let agent_id = agent_id.clone();
        let source = source.clone();
        let target = target.clone();
        Box::pin(async move {
            let mut repositories = lock(&self.repositories);
            let repository = repositories
                .get_mut(&agent_id)
                .ok_or_else(|| not_found("memfs"))?;
            if repository.files.contains_key(&target) {
                return Err(RuntimeError::Conflict {
                    context: "memfs rename target".into(),
                });
            }
            let contents = repository
                .files
                .get(&source)
                .cloned()
                .ok_or_else(|| not_found("memfs file"))?;
            repository.files.remove(&source);
            repository.files.insert(target, contents);
            Ok(())
        })
    }

    fn history(
        &self,
        agent_id: &AgentId,
        items: Sender<MemFsHistoryEntry>,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let values = lock(&self.repositories)
            .get(agent_id)
            .ok_or_else(|| not_found("memfs"))
            .and_then(|repository| {
                bounded_values(
                    repository
                        .history
                        .iter()
                        .rev()
                        .map(|item| MemFsHistoryEntry {
                            revision: item.revision.clone(),
                            summary: item.summary.clone(),
                        }),
                    "fake_memfs_history_max",
                )
            });
        Box::pin(async move { send_all(values?, items, cancellation, "memfs history").await })
    }

    fn file_at_revision(
        &self,
        agent_id: &AgentId,
        path: &RepositoryPath,
        revision: &RevisionId,
    ) -> lotta_runtime::ports::PortFuture<'_, MemoryFileContent> {
        let result = lock(&self.repositories)
            .get(agent_id)
            .ok_or_else(|| not_found("memfs"))
            .and_then(|repository| Self::snapshot(repository, Some(revision)))
            .and_then(|files| {
                files
                    .get(path)
                    .cloned()
                    .ok_or_else(|| not_found("memfs revision file"))
            });
        Box::pin(async move { result })
    }

    fn diff(
        &self,
        agent_id: &AgentId,
        source: Option<&RevisionId>,
        target: Option<&RevisionId>,
        chunks: Sender<DiffChunk>,
        cancellation: CancellationToken,
    ) -> lotta_runtime::ports::PortFuture<'_, ()> {
        let values = lock(&self.repositories)
            .get(agent_id)
            .ok_or_else(|| not_found("memfs"))
            .and_then(|repository| {
                incremental_diff(
                    Self::snapshot(repository, source)?,
                    Self::snapshot(repository, target)?,
                )
            });
        Box::pin(async move { send_all(values?, chunks, cancellation, "memfs diff").await })
    }

    fn commit(
        &self,
        agent_id: &AgentId,
        message: &CommitMessage,
    ) -> lotta_runtime::ports::PortFuture<'_, RevisionId> {
        let agent_id = agent_id.clone();
        let message = message.clone();
        Box::pin(async move {
            let mut repositories = lock(&self.repositories);
            let repository = repositories
                .get_mut(&agent_id)
                .ok_or_else(|| not_found("memfs"))?;
            Self::check_snapshot_retention(repository, &repository.files, true, 0)?;
            let files = repository.files.clone();
            Self::commit_snapshot(repository, message, files)
        })
    }

    fn create_worktree(
        &self,
        agent_id: &AgentId,
    ) -> lotta_runtime::ports::PortFuture<'_, WorktreeId> {
        let agent_id = agent_id.clone();
        Box::pin(async move {
            let mut repositories = lock(&self.repositories);
            let repository = repositories
                .get_mut(&agent_id)
                .ok_or_else(|| not_found("memfs"))?;
            if repository.worktrees.len() >= TESTKIT_ITEMS_MAX {
                return Err(limit("fake_memfs_worktrees_max"));
            }
            Self::check_snapshot_retention(repository, &repository.files, false, 0)?;
            let sequence = repository
                .worktree_sequence
                .checked_add(1)
                .ok_or_else(|| limit("fake_memfs_worktree_sequence"))?;
            let id = WorktreeId::new(format!("testkit-worktree-{sequence:016}"))?;
            let files = repository.files.clone();
            repository.worktrees.insert(id.clone(), files);
            repository.worktree_sequence = sequence;
            Ok(id)
        })
    }

    fn merge_worktree(
        &self,
        agent_id: &AgentId,
        worktree: &WorktreeId,
        message: &CommitMessage,
    ) -> lotta_runtime::ports::PortFuture<'_, RevisionId> {
        let agent_id = agent_id.clone();
        let worktree = worktree.clone();
        let message = message.clone();
        Box::pin(async move {
            let mut repositories = lock(&self.repositories);
            let repository = repositories
                .get_mut(&agent_id)
                .ok_or_else(|| not_found("memfs"))?;
            let files = repository
                .worktrees
                .get(&worktree)
                .ok_or_else(|| not_found("memfs worktree"))?;
            let removed_bytes = Self::map_bytes(&repository.files)?
                .checked_add(Self::map_bytes(files)?)
                .ok_or_else(|| limit("fake_memfs_retained_bytes_max"))?;
            Self::check_snapshot_retention(repository, files, true, removed_bytes)?;
            let files = files.clone();
            let revision = Self::commit_snapshot(repository, message, files.clone())?;
            repository.files = files;
            repository.worktrees.remove(&worktree);
            Ok(revision)
        })
    }
}

fn incremental_diff(
    source: &BTreeMap<RepositoryPath, MemoryFileContent>,
    target: &BTreeMap<RepositoryPath, MemoryFileContent>,
) -> Result<Vec<DiffChunk>, RuntimeError> {
    use std::cmp::Ordering;
    let mut source_items = source.iter().peekable();
    let mut target_items = target.iter().peekable();
    let mut chunks = Vec::new();
    while source_items.peek().is_some() || target_items.peek().is_some() {
        if chunks.len() >= TESTKIT_ITEMS_MAX {
            return Err(limit("fake_memfs_diff_chunks_max"));
        }
        let order = match (source_items.peek(), target_items.peek()) {
            (Some((left, _)), Some((right, _))) => left.cmp(right),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => break,
        };
        let chunk = match order {
            Ordering::Less => source_items
                .next()
                .map(|(path, old)| diff_chunk(path, "deleted", Some(old), None)),
            Ordering::Greater => target_items
                .next()
                .map(|(path, new)| diff_chunk(path, "added", None, Some(new))),
            Ordering::Equal => {
                let (path, old) = source_items
                    .next()
                    .ok_or_else(|| not_found("memfs diff source"))?;
                let (_, new) = target_items
                    .next()
                    .ok_or_else(|| not_found("memfs diff target"))?;
                (old != new).then(|| diff_chunk(path, "modified", Some(old), Some(new)))
            }
        };
        if let Some(chunk) = chunk {
            chunks.push(chunk?);
        }
    }
    Ok(chunks)
}

fn diff_chunk(
    path: &RepositoryPath,
    status: &str,
    old: Option<&MemoryFileContent>,
    new: Option<&MemoryFileContent>,
) -> Result<DiffChunk, RuntimeError> {
    let path = path.as_path().as_os_str().as_encoded_bytes();
    let checked_add = |total: usize, amount: usize| {
        total
            .checked_add(amount)
            .ok_or_else(|| limit(MEMFS_DIFF_CHUNK_BYTES_MAX.name))
    };
    let mut total = checked_add(path.len(), 1)?;
    total = checked_add(total, status.len())?;
    total = checked_add(total, 1)?;
    if let Some(value) = old {
        total = checked_add(total, 2)?;
        total = checked_add(total, value.as_slice().len())?;
        total = checked_add(total, 1)?;
    }
    if let Some(value) = new {
        total = checked_add(total, 2)?;
        total = checked_add(total, value.as_slice().len())?;
        total = checked_add(total, 1)?;
    }
    if total > MEMFS_DIFF_CHUNK_BYTES_MAX.value {
        return Err(limit(MEMFS_DIFF_CHUNK_BYTES_MAX.name));
    }
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(path);
    bytes.push(b' ');
    bytes.extend_from_slice(status.as_bytes());
    bytes.push(b'\n');
    if let Some(value) = old {
        bytes.extend_from_slice(b"- ");
        bytes.extend_from_slice(value.as_slice());
        bytes.push(b'\n');
    }
    if let Some(value) = new {
        bytes.extend_from_slice(b"+ ");
        bytes.extend_from_slice(value.as_slice());
        bytes.push(b'\n');
    }
    DiffChunk::new(bytes)
}

fn bounded_values<T>(
    values: impl Iterator<Item = T>,
    context: &'static str,
) -> Result<Vec<T>, RuntimeError> {
    let mut bounded = Vec::new();
    for value in values {
        if bounded.len() >= TESTKIT_ITEMS_MAX {
            return Err(limit(context));
        }
        bounded.push(value);
    }
    Ok(bounded)
}

async fn send_all<T: Send>(
    values: Vec<T>,
    sender: Sender<T>,
    cancellation: CancellationToken,
    context: &'static str,
) -> Result<(), RuntimeError> {
    if cancellation.is_cancelled() {
        return Err(cancelled(context));
    }
    for value in values {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(cancelled(context)),
            result = sender.send(value) => {
                result.map_err(|_| RuntimeError::AdapterFailure {
                    code: "testkit_channel_closed",
                    context: context.into(),
                })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> RepositoryPath {
        RepositoryPath::new("a".into()).expect("path")
    }

    fn content(len: usize) -> MemoryFileContent {
        MemoryFileContent::new(vec![b'x'; len]).expect("content")
    }

    #[test]
    fn diff_chunk_combined_below_at_and_over_bound() {
        let path = path();
        let overhead = path.as_path().as_os_str().as_encoded_bytes().len() + "modified".len() + 5;
        let at_old = MEMFS_DIFF_CHUNK_BYTES_MAX.value - overhead;
        assert_eq!(
            diff_chunk(&path, "modified", Some(&content(at_old - 1)), None)
                .expect("below")
                .as_slice()
                .len(),
            MEMFS_DIFF_CHUNK_BYTES_MAX.value - 1
        );
        assert_eq!(
            diff_chunk(&path, "modified", Some(&content(at_old)), None)
                .expect("at")
                .as_slice()
                .len(),
            MEMFS_DIFF_CHUNK_BYTES_MAX.value
        );
        assert!(matches!(
            diff_chunk(&path, "modified", Some(&content(at_old + 1)), None),
            Err(RuntimeError::LimitExceeded { ref context })
                if context == MEMFS_DIFF_CHUNK_BYTES_MAX.name
        ));
    }

    #[test]
    fn aggregate_snapshot_bounds_are_atomic() {
        let mut repository = Repository::default();
        repository.files.insert(path(), content(1));
        let mut oversized = BTreeMap::new();
        for index in 0..8 {
            oversized.insert(
                RepositoryPath::new(format!("large-{index}").into()).expect("path"),
                content(MEMFS_RETAINED_BYTES_MAX / 8),
            );
        }
        let before = repository.history.len();
        assert!(matches!(
            FakeMemFs::commit_snapshot(
                &mut repository,
                CommitMessage::new("bounded".into()).expect("message"),
                oversized,
            ),
            Err(RuntimeError::LimitExceeded { ref context })
                if context == "fake_memfs_retained_bytes_max"
        ));
        assert_eq!(repository.history.len(), before);
    }
}

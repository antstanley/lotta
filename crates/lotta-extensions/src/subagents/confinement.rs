//! Canonical root validation and scoped conversation/history capabilities.

use super::types::{MemoryScope, ParentScope, RequestError, SubagentType};
use lotta_domain::{AgentId, ConversationId};
use lotta_runtime::boundary::{CommitMessage, RevisionId, WorktreeId};
use lotta_runtime::ports::MemFsPort;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use tokio::sync::Mutex;

/// Canonical launch roots after overlap, symlink, and profile validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfinementPlan {
    filesystem_roots: Vec<PathBuf>,
    memory_readonly_roots: Vec<PathBuf>,
    memory_writable_roots: Vec<PathBuf>,
    primary_memory_root: Option<PathBuf>,
}

impl ConfinementPlan {
    /// Validates explicit real roots and profile-specific memory posture.
    pub fn new(
        subagent_type: SubagentType,
        filesystem_roots: &[PathBuf],
        memory_scope: Option<&MemoryScope>,
        reflection_worktree: Option<&Path>,
    ) -> Result<Self, RequestError> {
        let filesystem_roots = canonical_roots(filesystem_roots)?;
        let Some(scope) = memory_scope else {
            if reflection_worktree.is_some() {
                return Err(RequestError::Combination);
            }
            return Ok(Self {
                filesystem_roots,
                memory_readonly_roots: Vec::new(),
                memory_writable_roots: Vec::new(),
                primary_memory_root: None,
            });
        };
        let primary = canonical_directory(&scope.primary_root)?;
        let readonly = canonical_roots(&scope.readonly_roots)?;
        let writable = canonical_roots(&scope.writable_roots)?;
        reject_cross_overlap(&readonly, &writable)?;
        if writable.iter().any(|root| overlaps(root, &primary)) {
            return Err(RequestError::Combination);
        }
        validate_reflection(subagent_type, reflection_worktree, &primary, &writable)?;
        Ok(Self {
            filesystem_roots,
            memory_readonly_roots: readonly,
            memory_writable_roots: writable,
            primary_memory_root: Some(primary),
        })
    }

    /// Returns canonical filesystem roots exposed to the OS sandbox.
    #[must_use]
    pub fn filesystem_roots(&self) -> &[PathBuf] {
        &self.filesystem_roots
    }

    /// Returns canonical read-only memory roots.
    #[must_use]
    pub fn memory_readonly_roots(&self) -> &[PathBuf] {
        &self.memory_readonly_roots
    }

    /// Returns canonical writable memory roots.
    #[must_use]
    pub fn memory_writable_roots(&self) -> &[PathBuf] {
        &self.memory_writable_roots
    }

    /// Returns the primary root retained only by the host for merge/rejection.
    #[must_use]
    pub fn primary_memory_root(&self) -> Option<&Path> {
        self.primary_memory_root.as_deref()
    }

    /// Resolves an existing path below one declared root without following an escape symlink.
    pub fn resolve_existing(&self, requested: &Path) -> Result<PathBuf, RequestError> {
        if has_parent_component(requested) {
            return Err(RequestError::Combination);
        }
        let canonical = requested
            .canonicalize()
            .map_err(|_| RequestError::Combination)?;
        if self
            .all_exposed_roots()
            .any(|root| within(&canonical, root))
        {
            Ok(canonical)
        } else {
            Err(RequestError::Combination)
        }
    }

    fn all_exposed_roots(&self) -> impl Iterator<Item = &PathBuf> {
        self.filesystem_roots
            .iter()
            .chain(&self.memory_readonly_roots)
            .chain(&self.memory_writable_roots)
    }
}

/// Reflection worktree lifecycle backed by the production Task39 `MemFS` port.
pub struct ReflectionWorktreePort<P: ?Sized> {
    memfs: Arc<P>,
    agent_id: AgentId,
    root: PathBuf,
    merge_lock: Arc<Mutex<()>>,
}

impl<P: MemFsPort + ?Sized> ReflectionWorktreePort<P> {
    /// Binds worktree operations to one agent and canonical Task39 worktree root.
    pub fn new(memfs: Arc<P>, agent_id: AgentId, root: PathBuf) -> Result<Self, RequestError> {
        let root = canonical_directory(&root)?;
        let merge_lock = shared_repository_lock(&root)?;
        Ok(Self {
            memfs,
            agent_id,
            root,
            merge_lock,
        })
    }

    /// Creates an isolated worktree under the repository-wide lifecycle lock.
    pub async fn create(&self) -> Result<(WorktreeId, PathBuf), RequestError> {
        let _lock = self.merge_lock.lock().await;
        let id = self
            .memfs
            .create_worktree(&self.agent_id)
            .await
            .map_err(|_| RequestError::Capability)?;
        let path = self.root.join(id.as_str());
        let path = canonical_directory(&path)?;
        if !path.starts_with(&self.root) {
            return Err(RequestError::Capability);
        }
        Ok((id, path))
    }

    /// Replaces every child-visible memory root with the exact reflection worktree.
    pub fn confine_request(
        &self,
        request: &mut super::types::SubagentRequest,
        path: &Path,
    ) -> Result<(), RequestError> {
        if request.subagent_type != SubagentType::Reflection {
            return Err(RequestError::Combination);
        }
        let path = canonical_directory(path)?;
        if !path.starts_with(&self.root) {
            return Err(RequestError::Capability);
        }
        let primary_root = request
            .memory_scope
            .as_ref()
            .ok_or(RequestError::Capability)?
            .primary_root
            .clone();
        request.memory_scope = Some(MemoryScope {
            primary_root,
            readonly_roots: Vec::new(),
            writable_roots: vec![path.clone()],
        });
        request.reflection_worktree = Some(path);
        request.validate()
    }

    /// Serializes commit and atomic merge; errors retain the worktree for diagnostics.
    pub async fn merge(
        &self,
        id: &WorktreeId,
        message: &CommitMessage,
    ) -> Result<RevisionId, ReflectionMergeError> {
        let _lock = self.merge_lock.lock().await;
        self.memfs
            .merge_worktree(&self.agent_id, id, message)
            .await
            .map_err(|error| ReflectionMergeError::Conflict {
                worktree: id.clone(),
                diagnostic: error.to_string(),
            })
    }
}

/// Typed reflection merge failure that preserves the exact diagnostic worktree.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReflectionMergeError {
    /// The worktree could not be committed or merged and remains available for diagnosis.
    #[error("reflection merge conflict in {worktree:?}: {diagnostic}")]
    Conflict {
        /// Retained worktree identity.
        worktree: WorktreeId,
        /// Stable adapter diagnostic.
        diagnostic: String,
    },
}

/// Scoped historical-message adapter used by recall only.
pub trait HistoricalMessagePort: Send + Sync {
    /// Port future.
    type Read<'a>: Future<Output = Result<Vec<u8>, RequestError>> + Send + 'a
    where
        Self: 'a;

    /// Reads only the exact capability-bound parent history.
    fn read_history(&self, agent_id: &AgentId, conversation_id: &ConversationId) -> Self::Read<'_>;
}

/// Capability wrapper that refuses every non-parent conversation before touching storage.
pub struct ScopedHistory<P> {
    parent: ParentScope,
    inner: P,
}

impl<P> ScopedHistory<P> {
    /// Binds one exact parent scope.
    #[must_use]
    pub const fn new(parent: ParentScope, inner: P) -> Self {
        Self { parent, inner }
    }
}

impl<P: HistoricalMessagePort> ScopedHistory<P> {
    /// Reads history only when both identifiers match the bound parent.
    pub fn read(
        &self,
        agent_id: &AgentId,
        conversation_id: &ConversationId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, RequestError>> + Send + '_>> {
        if agent_id != &self.parent.agent_id || conversation_id != &self.parent.conversation_id {
            return Box::pin(async { Err(RequestError::Capability) });
        }
        Box::pin(self.inner.read_history(agent_id, conversation_id))
    }
}

fn shared_repository_lock(root: &Path) -> Result<Arc<Mutex<()>>, RequestError> {
    static LOCKS: OnceLock<StdMutex<BTreeMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| StdMutex::new(BTreeMap::new()));
    let mut locks = locks.lock().map_err(|_| RequestError::Capability)?;
    if let Some(lock) = locks.get(root).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    locks.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(Mutex::new(()));
    locks.insert(root.to_path_buf(), Arc::downgrade(&lock));
    Ok(lock)
}

fn validate_reflection(
    subagent_type: SubagentType,
    worktree: Option<&Path>,
    primary: &Path,
    writable: &[PathBuf],
) -> Result<(), RequestError> {
    if subagent_type != SubagentType::Reflection {
        return if worktree.is_none() {
            Ok(())
        } else {
            Err(RequestError::Combination)
        };
    }
    let worktree = canonical_directory(worktree.ok_or(RequestError::Capability)?)?;
    if overlaps(&worktree, primary)
        || writable.len() != 1
        || writable.first().is_none_or(|root| root != &worktree)
    {
        return Err(RequestError::Combination);
    }
    Ok(())
}

fn canonical_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>, RequestError> {
    let mut canonical: Vec<PathBuf> = Vec::new();
    canonical
        .try_reserve_exact(roots.len())
        .map_err(|_| RequestError::Bound)?;
    for root in roots {
        let root = canonical_directory(root)?;
        if canonical.iter().any(|other| overlaps(other, &root)) {
            return Err(RequestError::Combination);
        }
        canonical.push(root);
    }
    Ok(canonical)
}

fn reject_cross_overlap(left: &[PathBuf], right: &[PathBuf]) -> Result<(), RequestError> {
    if left.iter().any(|a| right.iter().any(|b| overlaps(a, b))) {
        Err(RequestError::Combination)
    } else {
        Ok(())
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, RequestError> {
    if !path.is_absolute() || has_parent_component(path) {
        return Err(RequestError::Combination);
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| RequestError::Combination)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RequestError::Combination);
    }
    path.canonicalize().map_err(|_| RequestError::Combination)
}

fn has_parent_component(path: &Path) -> bool {
    path.components()
        .any(|component| component == Component::ParentDir)
}

fn overlaps(left: &Path, right: &Path) -> bool {
    within(left, right) || within(right, left)
}

fn within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

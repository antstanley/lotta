use super::PortFuture;
use crate::boundary::{
    CommitMessage, DiffChunk, InitialMemoryBlocks, MemoryFileContent, RepositoryPath, RevisionId,
    WorktreeId,
};
use lotta_domain::AgentId;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// Scalar working-tree status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemFsStatus {
    /// Current revision, absent before the first commit.
    pub revision: Option<RevisionId>,
    /// Whether uncommitted changes exist.
    pub dirty: bool,
}

/// One streamed tree entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemFsTreeEntry {
    /// Repository-relative file path.
    pub path: RepositoryPath,
}

/// One streamed history entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemFsHistoryEntry {
    /// Opaque committed revision.
    pub revision: RevisionId,
    /// Bounded commit summary.
    pub summary: CommitMessage,
}

/// Owns filesystem and revision effects for one independent memory repository per agent.
///
/// # Preconditions
/// All inputs have crossed public validation and senders are bounded. Adapters enforce the current
/// repository Markdown/frontmatter rules. Existing read/source paths are canonicalized immediately
/// before effects and checked against the retained canonical root. For absent create/write/rename
/// targets, adapters canonicalize the retained root and nearest existing parent, append and
/// validate
/// unresolved components, then use no-follow or handle-relative operations to prevent symlink and
/// time-of-check/time-of-use escape.
///
/// # Errors
/// Reports absence, confinement, validation, conflict, limit, revision, permission, cancellation,
/// channel closure, and translated filesystem failures as [`crate::RuntimeError`].
///
/// # Cancellation
/// Tree, history, and diff observe explicit tokens and may emit a valid prefix. Atomic reads,
/// mutations, commits, and merges preserve complete values under future cancellation.
///
/// # Ownership
/// Streamed and scalar results are owned bounded values. Borrowed IDs and inputs live only for each
/// returned future; opaque worktree identifiers never expose adapter filesystem paths.
pub trait MemFsPort: Send + Sync {
    /// Initializes one repository and returns its initial revision.
    fn initialize(
        &self,
        agent_id: &AgentId,
        blocks: &InitialMemoryBlocks,
    ) -> PortFuture<'_, RevisionId>;
    /// Returns working-tree status and the committed revision used for prompt freshness.
    ///
    /// Runtime compares `revision` with its compiled revision and owns recompilation decisions;
    /// adapters do not invoke speculative post-commit prompt hooks.
    fn status(&self, agent_id: &AgentId) -> PortFuture<'_, MemFsStatus>;
    /// Streams repository-relative files in deterministic order.
    fn tree(
        &self,
        agent_id: &AgentId,
        revision: Option<&RevisionId>,
        items: Sender<MemFsTreeEntry>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ()>;
    /// Reads one bounded file from the working tree.
    fn read(&self, agent_id: &AgentId, path: &RepositoryPath) -> PortFuture<'_, MemoryFileContent>;
    /// Atomically writes one bounded file.
    fn write(
        &self,
        agent_id: &AgentId,
        path: &RepositoryPath,
        contents: &MemoryFileContent,
    ) -> PortFuture<'_, ()>;
    /// Deletes one confined file.
    fn delete(&self, agent_id: &AgentId, path: &RepositoryPath) -> PortFuture<'_, ()>;
    /// Atomically renames one confined file.
    fn rename(
        &self,
        agent_id: &AgentId,
        source: &RepositoryPath,
        target: &RepositoryPath,
    ) -> PortFuture<'_, ()>;
    /// Streams commits newest-first in deterministic order.
    fn history(
        &self,
        agent_id: &AgentId,
        items: Sender<MemFsHistoryEntry>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ()>;
    /// Reads one bounded file at an opaque revision.
    fn file_at_revision(
        &self,
        agent_id: &AgentId,
        path: &RepositoryPath,
        revision: &RevisionId,
    ) -> PortFuture<'_, MemoryFileContent>;
    /// Streams an uncapped diff as independently bounded chunks with backpressure.
    fn diff(
        &self,
        agent_id: &AgentId,
        source: Option<&RevisionId>,
        target: Option<&RevisionId>,
        chunks: Sender<DiffChunk>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ()>;
    /// Validates Markdown/frontmatter, then commits and returns only the revision.
    ///
    /// The adapter must run the repository's current pre-commit validation before creating any
    /// commit and translate validation failures into [`crate::RuntimeError`].
    fn commit(&self, agent_id: &AgentId, message: &CommitMessage) -> PortFuture<'_, RevisionId>;
    /// Creates an isolated reflection worktree represented by an opaque ID.
    fn create_worktree(&self, agent_id: &AgentId) -> PortFuture<'_, WorktreeId>;
    /// Merges a completed opaque worktree and returns the merge revision.
    fn merge_worktree(
        &self,
        agent_id: &AgentId,
        worktree: &WorktreeId,
        message: &CommitMessage,
    ) -> PortFuture<'_, RevisionId>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structural_memfs_records_have_exact_field_types() {
        let MemFsStatus { revision, dirty } = MemFsStatus {
            revision: None,
            dirty: false,
        };
        let _: Option<RevisionId> = revision;
        let _: bool = dirty;
        let MemFsTreeEntry { path } = MemFsTreeEntry {
            path: RepositoryPath::new("x".into()).unwrap(),
        };
        let _: RepositoryPath = path;
        let MemFsHistoryEntry { revision, summary } = MemFsHistoryEntry {
            revision: RevisionId::new("r".into()).unwrap(),
            summary: CommitMessage::new("m".into()).unwrap(),
        };
        let _: RevisionId = revision;
        let _: CommitMessage = summary;
    }
}

//! WebSocket memory command group.
//!
//! Decodes and routes the pinned `list_memory`, `memory_history`,
//! `memory_file_at_ref`, `memory_commit_diff`, `read_memory_file`,
//! `write_memory_file`, `delete_memory_file`, and `enable_memfs` commands (the
//! §WebSocket command groups Memory row) against one Task 29
//! [`GitMemFs`](lotta_memfs::GitMemFs) repository per agent.
//!
//! Memory paths are confined identically to the Task 39 memory tools: the
//! confinement root is the agent memory repository
//! (`<backend>/memfs/<agent>/memory`, never the workspace sandbox root — the
//! bridge never receives a workspace directory) and the lexical rejection unit
//! is the same [`RepositoryPath`](lotta_runtime::boundary::RepositoryPath) the
//! tools use, yielding identical traversal and absolute-path classes. Symlink
//! escapes are enforced by the Task 29 no-follow effect layer (`regular` and
//! `open_parent` reject symlinked files and ancestors), so an escaped write
//! fails as a typed response without touching outside bytes. Two deliberate
//! lexical corners are stricter than the desktop baseline: `./x.md` is
//! rejected where the baseline normalizes it, and revision-read paths are
//! confined like every other memory path.
//!
//! Every mutating command emits a `memory_updated` snapshot after success,
//! ordered so the optional post-commit push lands strictly between the commit
//! and the notification: [`lotta_runtime::ports::MemFsPort::transact`] applies
//! mutations, commits,
//! then pushes before returning, and push failures are warn-and-continue
//! inside the adapter, so they never undo or fail the committed write. Write
//! and delete snapshot before their response frame; enable answers first and
//! then snapshots `["*"]`; a no-change write (or an idempotent absent-file
//! delete) commits nothing and emits no snapshot. Handlers run detached from
//! the listener loop like the pinned `runDetachedListenerTask`.
//!
//! Baseline degradations imposed by the Task 29 port surface, kept honest:
//! history commits carry empty timestamps and null author names (the port
//! streams revision + summary only), `memory_history.file_path` scoping
//! degrades to repository-wide history (no per-path log on the port), and a
//! root-commit diff yields an empty patch (no parent snapshot exists to
//! compare). Prompt recompilation after committed writes is Task 58; this
//! group only mutates and reports.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::Engine as _;
use lotta_domain::{AgentId, Clock};
use lotta_memfs::GitMemFs;
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{
    CommitMessage, DiffChunk, InitialMemoryBlocks, MemoryFileContent, RepositoryPath, RevisionId,
};
use lotta_runtime::ports::{
    MemFsCommitAuthor, MemFsHistoryEntry, MemFsMutation, MemFsPort, MemFsTransactionResult,
    MemFsTreeEntry,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    bounds::WS_FRAME_BYTES_MAX, error::AppServerError, errors::ProtocolErrorEnvelope,
    framing::DecodedFrame, ws::connection::ConnectionId,
};

/// History commits returned when a client omits `limit` (pinned parity).
pub const HISTORY_COMMITS_DEFAULT: usize = 50;
/// Upper bound on history commits accepted from one request.
pub const HISTORY_COMMITS_MAX: usize = 200;
/// Entries per `list_memory_response` chunk (pinned parity).
pub const LIST_CHUNK_ENTRIES: usize = 5;
/// Collected working-tree entries cap for one listing.
pub const LIST_ENTRIES_MAX: usize = lotta_runtime::bounds::MEMORY_FILES_MAX.value;
/// Total diff bytes retained for one commit-diff response.
pub const COMMIT_DIFF_TOTAL_BYTES_MAX: usize =
    lotta_runtime::bounds::MEMORY_FILE_BYTES_MAX.value * 2;
/// Revisions scanned to locate one commit's parent for its diff.
pub const HISTORY_SCAN_MAX: usize = 10_000;
/// Depth of internal streaming channels between adapter and bridge.
const STREAM_CHANNEL_DEPTH: usize = 16;

/// Scrubbed failure detail for rejected or failed listings.
const LIST_FAILURE: &str = "Failed to list memory";
/// Scrubbed failure detail for rejected or failed history fetches.
const HISTORY_FAILURE: &str = "Failed to fetch memory history";
/// Scrubbed failure detail for failed read-at-ref requests.
const AT_REF_FAILURE: &str = "Failed to read file at ref";
/// Scrubbed failure detail for failed commit diffs.
const DIFF_FAILURE: &str = "Failed to get commit diff";
/// Scrubbed failure detail for failed working-tree reads.
const READ_FAILURE: &str = "Failed to read memory file";
/// Scrubbed failure detail for failed or rejected writes.
const WRITE_FAILURE: &str = "Failed to write memory file";
/// Scrubbed failure detail for failed or rejected deletes.
const DELETE_FAILURE: &str = "Failed to delete memory file";
/// Scrubbed failure detail for failed enables.
const ENABLE_FAILURE: &str = "Failed to enable memfs";
/// Pinned confinement class shared with the memory tools.
const NOT_RELATIVE: &str = "path must be a non-empty relative path";
/// Pinned confinement class shared with the memory tools.
const ESCAPES_ROOT: &str = "path must resolve inside the memory root";
/// Pinned disabled-memfs rejection text.
const MEMFS_DISABLED: &str = "memfs is not enabled for this agent";

/// Image extensions the pinned memory viewer renders inline.
const IMAGE_MIME_BY_EXTENSION: [(&str, &str); 5] = [
    (".gif", "image/gif"),
    (".jpeg", "image/jpeg"),
    (".jpg", "image/jpeg"),
    (".png", "image/png"),
    (".webp", "image/webp"),
];

/// Pinned `list_memory` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ListMemoryCommand {
    /// Agent whose memory is listed.
    pub agent_id: String,
    /// Whether wiki-link references join each markdown entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_references: Option<bool>,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `memory_history` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MemoryHistoryCommand {
    /// Agent whose memory history is fetched.
    pub agent_id: String,
    /// Optional path scope; the port serves repository-wide history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Requested commit count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `memory_file_at_ref` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MemoryFileAtRefCommand {
    /// Agent whose memory is read.
    pub agent_id: String,
    /// Confined memory-relative path.
    pub file_path: String,
    /// Git revision to read, echoed back as `ref`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `memory_commit_diff` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MemoryCommitDiffCommand {
    /// Agent whose memory is read.
    pub agent_id: String,
    /// Commit whose introduced changes are shown.
    pub sha: String,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `read_memory_file` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReadMemoryFileCommand {
    /// Agent whose memory is read.
    pub agent_id: String,
    /// Confined memory-relative path.
    pub path: String,
    /// Requested content encoding; absence means UTF-8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<MemoryEncoding>,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `write_memory_file` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WriteMemoryFileCommand {
    /// Agent whose memory is written.
    pub agent_id: String,
    /// Confined memory-relative path.
    pub path: String,
    /// Encoded file content.
    pub content: String,
    /// Encoding of `content`; absence means UTF-8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<MemoryEncoding>,
    /// Optional commit message override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_message: Option<String>,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `delete_memory_file` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeleteMemoryFileCommand {
    /// Agent whose memory loses the file.
    pub agent_id: String,
    /// Confined memory-relative path.
    pub path: String,
    /// Optional commit message override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_message: Option<String>,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `enable_memfs` payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnableMemfsCommand {
    /// Agent gaining memfs.
    pub agent_id: String,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Content transfer encoding shared by reads and writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryEncoding {
    /// Lossily decoded UTF-8 text.
    Utf8,
    /// Standard-alphabet base64 bytes.
    Base64,
}

/// The eight concrete memory group commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum MemoryCommand {
    /// Working-tree memory listing.
    #[serde(rename = "list_memory")]
    List(ListMemoryCommand),
    /// Revision-graph log.
    #[serde(rename = "memory_history")]
    History(MemoryHistoryCommand),
    /// Read-at-revision.
    #[serde(rename = "memory_file_at_ref")]
    FileAtRef(MemoryFileAtRefCommand),
    /// One commit's introduced patch.
    #[serde(rename = "memory_commit_diff")]
    CommitDiff(MemoryCommitDiffCommand),
    /// Working-tree file read.
    #[serde(rename = "read_memory_file")]
    ReadFile(ReadMemoryFileCommand),
    /// Durable write plus commit.
    #[serde(rename = "write_memory_file")]
    WriteFile(WriteMemoryFileCommand),
    /// Durable delete plus commit.
    #[serde(rename = "delete_memory_file")]
    DeleteFile(DeleteMemoryFileCommand),
    /// Repository initialization.
    #[serde(rename = "enable_memfs")]
    EnableMemfs(EnableMemfsCommand),
}

/// One listed memory file entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryFileEntry {
    /// Repository-relative POSIX path.
    pub relative_path: String,
    /// Whether the file lives under `system/`.
    pub is_system: bool,
    /// Frontmatter description when present.
    pub description: Option<String>,
    /// Markdown body, or empty for images.
    pub content: String,
    /// Body length in bytes for markdown, whole-file length for images.
    pub size: usize,
    /// Resolved wiki-link targets when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<String>>,
    /// Entry kind.
    pub kind: MemoryEntryKind,
    /// MIME type derived from the extension.
    pub mime_type: &'static str,
}

/// Kind of one [`MemoryFileEntry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEntryKind {
    /// Frontmatter-bearing markdown file.
    Markdown,
    /// Renderable image asset.
    Image,
}

/// One history row of `memory_history_response`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryHistoryCommit {
    /// Opaque committed revision.
    pub sha: String,
    /// Bounded commit summary.
    pub message: String,
    /// Commit time; unexposed by the port, so always empty.
    pub timestamp: String,
    /// Author display name; unexposed by the port, so always null.
    pub author_name: Option<String>,
}

/// Pinned `list_memory_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct ListMemoryResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Chunk of listed entries.
    pub entries: Vec<MemoryFileEntry>,
    /// Whether this chunk completes the listing.
    pub done: bool,
    /// Total entry count across chunks.
    pub total: usize,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether memfs is available; omitted on failure like the baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memfs_enabled: Option<bool>,
    /// Whether the local checkout exists; omitted on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memfs_initialized: Option<bool>,
}

/// Pinned `memory_history_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct MemoryHistoryResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Path scope as requested, or empty for global history.
    pub file_path: String,
    /// Commits newest-first.
    pub commits: Vec<MemoryHistoryCommit>,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `memory_file_at_ref_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct MemoryFileAtRefResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// File as requested.
    pub file_path: String,
    /// Revision as requested.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Lossily decoded content, or null on failure.
    pub content: Option<String>,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `memory_commit_diff_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct MemoryCommitDiffResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Commit as requested.
    pub sha: String,
    /// Introduced patch text, or null on failure.
    pub diff: Option<String>,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `read_memory_file_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct ReadMemoryFileResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Agent as requested.
    pub agent_id: String,
    /// Validated confined path.
    pub path: String,
    /// Encoded content, or null on failure.
    pub content: Option<String>,
    /// Encoding echoed back to the client.
    pub encoding: MemoryEncoding,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Shared shape of `write_memory_file_response` and `delete_memory_file_response`.
#[derive(Clone, Debug, Serialize)]
pub struct MutationFileResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Agent as requested.
    pub agent_id: String,
    /// Validated confined path.
    pub path: String,
    /// Operation success.
    pub success: bool,
    /// Whether a commit was created; omitted on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed: Option<bool>,
    /// New head revision; omitted on failure or no-change mutations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `enable_memfs_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct EnableMemfsResponseMessage {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Absolute agent memory directory on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_directory: Option<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `memory_updated` refresh notice.
#[derive(Clone, Debug, Serialize)]
pub struct MemoryUpdatedMessage {
    /// Affected paths, or `["*"]` for whole-list refreshes.
    pub affected_paths: Vec<String>,
    /// Notification instant in epoch milliseconds.
    pub timestamp: i64,
}

/// The nine outbound memory group messages.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum MemoryMessage {
    /// Listing chunk.
    #[serde(rename = "list_memory_response")]
    ListResponse(ListMemoryResponseMessage),
    /// History result.
    #[serde(rename = "memory_history_response")]
    HistoryResponse(MemoryHistoryResponseMessage),
    /// Read-at-ref result.
    #[serde(rename = "memory_file_at_ref_response")]
    FileAtRefResponse(MemoryFileAtRefResponseMessage),
    /// Commit diff result.
    #[serde(rename = "memory_commit_diff_response")]
    CommitDiffResponse(MemoryCommitDiffResponseMessage),
    /// Working-tree read result.
    #[serde(rename = "read_memory_file_response")]
    ReadFileResponse(ReadMemoryFileResponseMessage),
    /// Write result.
    #[serde(rename = "write_memory_file_response")]
    WriteFileResponse(MutationFileResponseMessage),
    /// Delete result.
    #[serde(rename = "delete_memory_file_response")]
    DeleteFileResponse(MutationFileResponseMessage),
    /// Enable result.
    #[serde(rename = "enable_memfs_response")]
    EnableMemfsResponse(EnableMemfsResponseMessage),
    /// Refresh notice emitted by every mutating command.
    #[serde(rename = "memory_updated")]
    Updated(MemoryUpdatedMessage),
}

/// Push callback delivering one outbound message to one connection.
pub type MemoryForwarder =
    Arc<dyn Fn(ConnectionId, MemoryMessage) -> Result<(), AppServerError> + Send + Sync>;

#[cfg(test)]
pub(crate) fn inert_forwarder() -> MemoryForwarder {
    Arc::new(|_, _| Ok(()))
}

/// Lexical confinement classification shared with the memory tools.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathRejection {
    /// Empty or absolute input.
    NotRelative,
    /// Traversal escaping the agent memory root.
    EscapesRoot,
}

impl PathRejection {
    fn text(self, prefix: &str) -> String {
        match self {
            Self::NotRelative => format!("{prefix}{NOT_RELATIVE}"),
            Self::EscapesRoot => format!("{prefix}{ESCAPES_ROOT}"),
        }
    }
}

/// Internal listing classification before entry assembly.
enum EntryKind {
    Markdown,
    Image(&'static str),
}

/// Which mutation response frame wraps the shared mutation shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationResponse {
    Write,
    Delete,
}

fn mutation_response(
    which: MutationResponse,
    message: MutationFileResponseMessage,
) -> MemoryMessage {
    match which {
        MutationResponse::Write => MemoryMessage::WriteFileResponse(message),
        MutationResponse::Delete => MemoryMessage::DeleteFileResponse(message),
    }
}

/// Everything one write/delete needs, bundled to keep the handler flat.
struct MutationRequest<'a> {
    which: MutationResponse,
    request_id: &'a str,
    agent_id: &'a str,
    path: &'a str,
    prefix: &'static str,
    fallback_verb: &'static str,
    /// Encoded write payload; `None` marks a delete.
    content: Option<(&'a str, MemoryEncoding)>,
    commit_message: Option<&'a str>,
}

/// Committed-mutation outcome mirrored into the response frame.
struct MutationOutcome {
    committed: bool,
    revision: Option<String>,
}

struct PreparedBackend {
    root: PathBuf,
    memfs: GitMemFs,
}

/// Applies wire memory commands to one agent's Task 29 memory repository.
pub struct MemoryBridge {
    memfs: GitMemFs,
    backend_root: PathBuf,
    clock: Arc<dyn Clock + Send + Sync>,
    forward: MemoryForwarder,
}

impl MemoryBridge {
    /// Creates a bridge over an existing-or-creatable canonical backend root.
    ///
    /// # Errors
    /// Returns [`AppServerError::Config`] when the backend cannot be prepared
    /// as a canonical ordinary directory for [`GitMemFs::new`].
    pub fn new(
        forward: MemoryForwarder,
        backend_dir: &Path,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, AppServerError> {
        let prepared = prepare_backend(backend_dir)?;
        Ok(Self {
            memfs: prepared.memfs,
            backend_root: prepared.root,
            clock,
            forward,
        })
    }

    /// Routes one decoded command in a detached task, like the baseline.
    pub fn handle(self: &Arc<Self>, connection: ConnectionId, command: &MemoryCommand) {
        let this = Arc::clone(self);
        let command = command.clone();
        tokio::spawn(async move { this.apply(connection, &command).await });
    }

    /// Applies one command inline, emitting responses through the forwarder.
    pub async fn apply(&self, connection: ConnectionId, command: &MemoryCommand) {
        match command {
            MemoryCommand::List(payload) => self.list(connection, payload).await,
            MemoryCommand::History(payload) => self.history(connection, payload).await,
            MemoryCommand::FileAtRef(payload) => self.file_at_ref(connection, payload).await,
            MemoryCommand::CommitDiff(payload) => self.commit_diff(connection, payload).await,
            MemoryCommand::ReadFile(payload) => self.read_file(connection, payload).await,
            MemoryCommand::WriteFile(payload) => self.write_file(connection, payload).await,
            MemoryCommand::DeleteFile(payload) => self.delete_file(connection, payload).await,
            MemoryCommand::EnableMemfs(payload) => self.enable_memfs(connection, payload).await,
        }
    }

    async fn list(&self, connection: ConnectionId, command: &ListMemoryCommand) {
        let Ok(agent) = AgentId::accept(command.agent_id.clone()) else {
            self.emit(connection, failed_list(&command.request_id, LIST_FAILURE));
            return;
        };
        if !self.initialized(&agent).await {
            self.emit(connection, disabled_list(&command.request_id));
            return;
        }
        let collected = self
            .collect_entries(&agent, command.include_references.unwrap_or(false))
            .await;
        match collected {
            Ok(entries) => self.emit_chunks(connection, &command.request_id, &entries),
            Err(_) => self.emit(connection, failed_list(&command.request_id, LIST_FAILURE)),
        }
    }

    fn emit_chunks(&self, connection: ConnectionId, request_id: &str, entries: &[MemoryFileEntry]) {
        let total = entries.len();
        let mut sent = 0;
        while sent < total {
            let end = (sent + LIST_CHUNK_ENTRIES).min(total);
            self.emit(
                connection,
                MemoryMessage::ListResponse(ListMemoryResponseMessage {
                    request_id: request_id.to_owned(),
                    entries: entries[sent..end].to_vec(),
                    done: end == total,
                    total,
                    success: true,
                    error: None,
                    memfs_enabled: Some(true),
                    memfs_initialized: Some(true),
                }),
            );
            sent = end;
        }
        if total == 0 {
            self.emit(connection, empty_list(request_id));
        }
    }

    async fn history(&self, connection: ConnectionId, command: &MemoryHistoryCommand) {
        let Ok(agent) = AgentId::accept(command.agent_id.clone()) else {
            self.emit(connection, failed_history(command, HISTORY_FAILURE));
            return;
        };
        let limit = command
            .limit
            .unwrap_or(HISTORY_COMMITS_DEFAULT)
            .min(HISTORY_COMMITS_MAX);
        match self.collect_history(&agent, limit).await {
            Ok(commits) => {
                let rows = commits
                    .into_iter()
                    .map(|entry| MemoryHistoryCommit {
                        sha: entry.revision.into_string(),
                        message: entry.summary.into_string(),
                        timestamp: String::new(),
                        author_name: None,
                    })
                    .collect();
                self.emit(
                    connection,
                    MemoryMessage::HistoryResponse(MemoryHistoryResponseMessage {
                        request_id: command.request_id.clone(),
                        file_path: command.file_path.clone().unwrap_or_default(),
                        commits: rows,
                        success: true,
                        error: None,
                    }),
                );
            }
            Err(_) => self.emit(connection, failed_history(command, HISTORY_FAILURE)),
        }
    }

    async fn file_at_ref(&self, connection: ConnectionId, command: &MemoryFileAtRefCommand) {
        let at_ref_failure = |error: Option<String>| {
            MemoryMessage::FileAtRefResponse(MemoryFileAtRefResponseMessage {
                request_id: command.request_id.clone(),
                file_path: command.file_path.clone(),
                reference: command.reference.clone(),
                content: None,
                success: false,
                error,
            })
        };
        let Ok(agent) = AgentId::accept(command.agent_id.clone()) else {
            self.emit(connection, at_ref_failure(Some(AT_REF_FAILURE.to_owned())));
            return;
        };
        let path = match confine(&command.file_path) {
            Ok(path) => path,
            Err(rejection) => {
                self.emit(connection, at_ref_failure(Some(rejection.text(""))));
                return;
            }
        };
        let Ok(revision) = RevisionId::new(command.reference.clone()) else {
            self.emit(connection, at_ref_failure(Some(AT_REF_FAILURE.to_owned())));
            return;
        };
        match self.memfs.file_at_revision(&agent, &path, &revision).await {
            Ok(content) => {
                let text = String::from_utf8_lossy(content.as_slice()).into_owned();
                self.emit(
                    connection,
                    MemoryMessage::FileAtRefResponse(MemoryFileAtRefResponseMessage {
                        request_id: command.request_id.clone(),
                        file_path: command.file_path.clone(),
                        reference: command.reference.clone(),
                        content: Some(text),
                        success: true,
                        error: None,
                    }),
                );
            }
            // Unknown revisions resolve through the Git graph into this arm.
            Err(_) => self.emit(connection, at_ref_failure(None)),
        }
    }

    async fn commit_diff(&self, connection: ConnectionId, command: &MemoryCommitDiffCommand) {
        let diff_failure = |error: Option<String>| {
            MemoryMessage::CommitDiffResponse(MemoryCommitDiffResponseMessage {
                request_id: command.request_id.clone(),
                sha: command.sha.clone(),
                diff: None,
                success: false,
                error,
            })
        };
        let Ok(agent) = AgentId::accept(command.agent_id.clone()) else {
            self.emit(connection, diff_failure(Some(DIFF_FAILURE.to_owned())));
            return;
        };
        let Ok(revision) = RevisionId::new(command.sha.clone()) else {
            self.emit(connection, diff_failure(Some(DIFF_FAILURE.to_owned())));
            return;
        };
        let Ok(parent) = self.parent_revision(&agent, &revision).await else {
            self.emit(connection, diff_failure(None));
            return;
        };
        // A root commit has no parent snapshot; the pinned `git show` likewise
        // prints only headers there, so an empty patch stays faithful.
        let text = match parent.as_ref() {
            None => Ok(String::new()),
            Some(parent) => self.diff_text(&agent, Some(parent), &revision).await,
        };
        match text {
            Ok(diff) => {
                self.emit(
                    connection,
                    MemoryMessage::CommitDiffResponse(MemoryCommitDiffResponseMessage {
                        request_id: command.request_id.clone(),
                        sha: command.sha.clone(),
                        diff: Some(diff),
                        success: true,
                        error: None,
                    }),
                );
            }
            Err(_) => self.emit(connection, diff_failure(None)),
        }
    }

    async fn read_file(&self, connection: ConnectionId, command: &ReadMemoryFileCommand) {
        let encoding = command.encoding.unwrap_or(MemoryEncoding::Utf8);
        let read_failure = |error: Option<String>| {
            MemoryMessage::ReadFileResponse(ReadMemoryFileResponseMessage {
                request_id: command.request_id.clone(),
                agent_id: command.agent_id.clone(),
                path: command.path.clone(),
                content: None,
                encoding,
                success: false,
                error,
            })
        };
        let Ok(agent) = AgentId::accept(command.agent_id.clone()) else {
            self.emit(connection, read_failure(Some(READ_FAILURE.to_owned())));
            return;
        };
        let path = match confine(&command.path) {
            Ok(path) => path,
            Err(rejection) => {
                self.emit(connection, read_failure(Some(rejection.text(""))));
                return;
            }
        };
        if !self.initialized(&agent).await {
            self.emit(connection, read_failure(Some(MEMFS_DISABLED.to_owned())));
            return;
        }
        match self.memfs.read(&agent, &path).await {
            Ok(content) => {
                self.emit(
                    connection,
                    MemoryMessage::ReadFileResponse(ReadMemoryFileResponseMessage {
                        request_id: command.request_id.clone(),
                        agent_id: command.agent_id.clone(),
                        path: display(&path),
                        content: Some(encode_content(content.as_slice(), encoding)),
                        encoding,
                        success: true,
                        error: None,
                    }),
                );
            }
            Err(_) => self.emit(connection, read_failure(None)),
        }
    }

    async fn write_file(&self, connection: ConnectionId, command: &WriteMemoryFileCommand) {
        let encoding = command.encoding.unwrap_or(MemoryEncoding::Utf8);
        self.run_mutation(
            connection,
            MutationRequest {
                which: MutationResponse::Write,
                request_id: &command.request_id,
                agent_id: &command.agent_id,
                path: &command.path,
                prefix: "write_memory_file: ",
                fallback_verb: "Update",
                content: Some((&command.content, encoding)),
                commit_message: command.commit_message.as_deref(),
            },
        )
        .await;
    }

    async fn delete_file(&self, connection: ConnectionId, command: &DeleteMemoryFileCommand) {
        self.run_mutation(
            connection,
            MutationRequest {
                which: MutationResponse::Delete,
                request_id: &command.request_id,
                agent_id: &command.agent_id,
                path: &command.path,
                prefix: "delete_memory_file: ",
                fallback_verb: "Delete",
                content: None,
                commit_message: command.commit_message.as_deref(),
            },
        )
        .await;
    }

    async fn run_mutation(&self, connection: ConnectionId, request: MutationRequest<'_>) {
        let prefix = request.prefix;
        let emit_failure = |error: String| {
            self.emit(
                connection,
                mutation_response(
                    request.which,
                    mutation_frame(&request, false, None, None, Some(error)),
                ),
            );
        };
        let Ok(agent) = AgentId::accept(request.agent_id.to_owned()) else {
            return emit_failure(format!("{prefix}invalid agent identifier"));
        };
        let path = match confine(request.path) {
            Ok(path) => path,
            Err(rejection) => return emit_failure(rejection.text(prefix)),
        };
        if !self.initialized(&agent).await {
            return emit_failure(format!("{prefix}{MEMFS_DISABLED}"));
        }
        let contents = match request.content {
            Some((raw, encoding)) => match decoded_bounded(raw, encoding) {
                Ok(contents) => Some(contents),
                Err(reason) => return emit_failure(format!("{prefix}{reason}")),
            },
            None => None,
        };
        // Idempotent delete: an absent file answers success without a commit,
        // exactly like the pinned removeIfPresent fast path.
        if contents.is_none()
            && matches!(
                self.memfs.read(&agent, &path).await,
                Err(RuntimeError::NotFound { .. })
            )
        {
            self.emit(
                connection,
                mutation_response(
                    request.which,
                    mutation_frame(&request, true, Some(false), None, None)
                        .with_path(display(&path)),
                ),
            );
            return;
        }
        let mutation = match contents {
            Some(contents) => MemFsMutation::Write {
                path: path.clone(),
                contents,
            },
            None => MemFsMutation::Delete { path: path.clone() },
        };
        let pathspec = display(&path);
        let fallback = format!("{} memory file {pathspec}", request.fallback_verb);
        let outcome = self
            .commit_mutation(&agent, mutation, request.commit_message, &fallback)
            .await;
        self.respond_to_mutation(connection, &request, outcome, pathspec);
    }

    /// Emits the terminal mutation frames: snapshot plus correlated success
    /// response, or a scrubbed failure detail.
    fn respond_to_mutation(
        &self,
        connection: ConnectionId,
        request: &MutationRequest<'_>,
        outcome: Result<MutationOutcome, ()>,
        pathspec: String,
    ) {
        let Ok(outcome) = outcome else {
            let scrubbed = match request.which {
                MutationResponse::Write => WRITE_FAILURE,
                MutationResponse::Delete => DELETE_FAILURE,
            };
            self.emit(
                connection,
                mutation_response(
                    request.which,
                    mutation_frame(request, false, None, None, Some(scrubbed.to_owned())),
                ),
            );
            return;
        };
        if outcome.committed {
            self.emit_snapshot(connection, &[pathspec.as_str()]);
        }
        self.emit(
            connection,
            mutation_response(
                request.which,
                mutation_frame(
                    request,
                    true,
                    Some(outcome.committed),
                    outcome.revision,
                    None,
                )
                .with_path(pathspec),
            ),
        );
    }

    async fn commit_mutation(
        &self,
        agent: &AgentId,
        mutation: MemFsMutation,
        custom: Option<&str>,
        fallback: &str,
    ) -> Result<MutationOutcome, ()> {
        let summary = custom
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback);
        let Some(message) = CommitMessage::new(summary.to_owned()).ok() else {
            return Err(());
        };
        let Some(author) = author_for(agent) else {
            return Err(());
        };
        // Transact applies, commits, and pushes before returning; push failures
        // are swallowed inside the adapter so they cannot fail this write.
        match self
            .memfs
            .transact(agent, std::slice::from_ref(&mutation), &message, &author)
            .await
        {
            Ok(MemFsTransactionResult::Committed(revision)) => Ok(MutationOutcome {
                committed: true,
                revision: Some(revision.into_string()),
            }),
            Ok(MemFsTransactionResult::NoChange) => Ok(MutationOutcome {
                committed: false,
                revision: None,
            }),
            Err(_) => Err(()),
        }
    }

    async fn enable_memfs(&self, connection: ConnectionId, command: &EnableMemfsCommand) {
        let enable_failure = |error: String| {
            MemoryMessage::EnableMemfsResponse(EnableMemfsResponseMessage {
                request_id: command.request_id.clone(),
                success: false,
                memory_directory: None,
                error: Some(error),
            })
        };
        let Ok(agent) = AgentId::accept(command.agent_id.clone()) else {
            return self.emit(connection, enable_failure(ENABLE_FAILURE.to_owned()));
        };
        if !self.initialized(&agent).await {
            let fresh = match InitialMemoryBlocks::new(Vec::new()) {
                Ok(blocks) => self.memfs.initialize(&agent, &blocks).await.is_ok(),
                Err(_) => false,
            };
            if !fresh {
                return self.emit(connection, enable_failure(ENABLE_FAILURE.to_owned()));
            }
        }
        self.emit(
            connection,
            MemoryMessage::EnableMemfsResponse(EnableMemfsResponseMessage {
                request_id: command.request_id.clone(),
                success: true,
                memory_directory: Some(self.memory_root(&agent).to_string_lossy().into_owned()),
                error: None,
            }),
        );
        self.emit_snapshot(connection, &["*"]);
    }

    fn memory_root(&self, agent: &AgentId) -> PathBuf {
        self.backend_root
            .join("memfs")
            .join(agent.as_str())
            .join("memory")
    }

    async fn initialized(&self, agent: &AgentId) -> bool {
        self.memfs.status(agent).await.is_ok()
    }

    async fn collect_entries(
        &self,
        agent: &AgentId,
        references: bool,
    ) -> Result<Vec<MemoryFileEntry>, RuntimeError> {
        let paths = self.working_paths(agent).await?;
        let known: HashSet<String> = paths
            .iter()
            .filter(|path| is_markdown(path.as_path()))
            .map(display)
            .collect();
        let mut entries = Vec::new();
        for path in paths {
            let Some(kind) = classify_entry(path.as_path()) else {
                continue;
            };
            entries.push(
                self.build_entry(agent, &path, kind, references, &known)
                    .await?,
            );
        }
        Ok(entries)
    }

    async fn build_entry(
        &self,
        agent: &AgentId,
        path: &RepositoryPath,
        kind: EntryKind,
        references: bool,
        known: &HashSet<String>,
    ) -> Result<MemoryFileEntry, RuntimeError> {
        let relative = display(path);
        let is_system = relative.starts_with("system/");
        let bytes = self.memfs.read(agent, path).await?;
        match kind {
            EntryKind::Image(mime_type) => Ok(MemoryFileEntry {
                relative_path: relative,
                is_system,
                description: None,
                content: String::new(),
                size: bytes.as_slice().len(),
                references: None,
                kind: MemoryEntryKind::Image,
                mime_type,
            }),
            EntryKind::Markdown => {
                let (description, body) = split_frontmatter(bytes.as_slice());
                let links = references.then(|| wiki_references(&body, &relative, known));
                Ok(MemoryFileEntry {
                    relative_path: relative,
                    is_system,
                    description,
                    size: body.len(),
                    content: body,
                    references: links,
                    kind: MemoryEntryKind::Markdown,
                    mime_type: "text/markdown",
                })
            }
        }
    }

    async fn working_paths(&self, agent: &AgentId) -> Result<Vec<RepositoryPath>, RuntimeError> {
        let (sender, mut receiver) = mpsc::channel::<MemFsTreeEntry>(STREAM_CHANNEL_DEPTH);
        let stream = self
            .memfs
            .tree(agent, None, sender, CancellationToken::new());
        tokio::pin!(stream);
        let mut paths = Vec::new();
        loop {
            tokio::select! {
                result = &mut stream => {
                    result?;
                    while let Some(entry) = receiver.recv().await && paths.len() < LIST_ENTRIES_MAX
                    {
                        paths.push(entry.path);
                    }
                    break;
                }
                entry = receiver.recv() => if let Some(entry) = entry
                    && paths.len() < LIST_ENTRIES_MAX
                {
                    paths.push(entry.path);
                }
            }
        }
        Ok(paths)
    }

    async fn collect_history(
        &self,
        agent: &AgentId,
        limit: usize,
    ) -> Result<Vec<MemFsHistoryEntry>, RuntimeError> {
        let (sender, mut receiver) = mpsc::channel::<MemFsHistoryEntry>(STREAM_CHANNEL_DEPTH);
        let stream = self.memfs.history(agent, sender, CancellationToken::new());
        tokio::pin!(stream);
        let mut entries = Vec::new();
        loop {
            tokio::select! {
                result = &mut stream => {
                    result?;
                    while let Some(entry) = receiver.recv().await && entries.len() < limit {
                        entries.push(entry);
                    }
                    break;
                }
                entry = receiver.recv() => if let Some(entry) = entry
                    && entries.len() < limit
                {
                    entries.push(entry);
                }
            }
        }
        Ok(entries)
    }

    async fn parent_revision(
        &self,
        agent: &AgentId,
        revision: &RevisionId,
    ) -> Result<Option<RevisionId>, ()> {
        let history = self
            .collect_history(agent, HISTORY_SCAN_MAX)
            .await
            .map_err(|_| ())?;
        let position = history
            .iter()
            .position(|entry| entry.revision == *revision)
            .ok_or(())?;
        Ok(history
            .get(position + 1)
            .map(|entry| entry.revision.clone()))
    }

    async fn diff_text(
        &self,
        agent: &AgentId,
        source: Option<&RevisionId>,
        target: &RevisionId,
    ) -> Result<String, RuntimeError> {
        let (sender, mut receiver) = mpsc::channel::<DiffChunk>(STREAM_CHANNEL_DEPTH);
        let stream = self.memfs.diff(
            agent,
            source,
            Some(target),
            sender,
            CancellationToken::new(),
        );
        tokio::pin!(stream);
        let mut bytes = Vec::new();
        loop {
            tokio::select! {
                result = &mut stream => {
                    result?;
                    while let Some(chunk) = receiver.recv().await {
                        if bytes.len() < COMMIT_DIFF_TOTAL_BYTES_MAX {
                            bytes.extend_from_slice(chunk.as_slice());
                        }
                    }
                    return Ok(String::from_utf8_lossy(&bytes).into_owned());
                }
                chunk = receiver.recv() => if let Some(chunk) = chunk
                    && bytes.len() < COMMIT_DIFF_TOTAL_BYTES_MAX
                {
                    bytes.extend_from_slice(chunk.as_slice());
                }
            }
        }
    }

    fn emit_snapshot(&self, connection: ConnectionId, paths: &[&str]) {
        self.emit(
            connection,
            MemoryMessage::Updated(MemoryUpdatedMessage {
                affected_paths: paths.iter().map(|path| (*path).to_owned()).collect(),
                timestamp: self.clock.now().as_utc().timestamp_millis(),
            }),
        );
    }

    fn emit(&self, connection: ConnectionId, message: MemoryMessage) {
        match serde_json::to_string(&message) {
            Ok(body) if body.len() <= WS_FRAME_BYTES_MAX => {
                let _ = (self.forward)(connection, message);
            }
            Ok(_) => tracing::warn!("memory response exceeded the frame bound and was dropped"),
            Err(_) => tracing::warn!("memory response failed to encode"),
        }
    }
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// # Errors
/// Returns a correlated protocol error for malformed known memory commands,
/// including unknown `encoding` values or missing required fields.
pub fn decode(frame: &DecodedFrame) -> Result<Option<MemoryCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    let parsed = match tag {
        Tag::ListMemory => typed::<ListMemoryCommand>(frame).map(MemoryCommand::List),
        Tag::MemoryHistory => typed::<MemoryHistoryCommand>(frame).map(MemoryCommand::History),
        Tag::MemoryFileAtRef => {
            typed::<MemoryFileAtRefCommand>(frame).map(MemoryCommand::FileAtRef)
        }
        Tag::MemoryCommitDiff => {
            typed::<MemoryCommitDiffCommand>(frame).map(MemoryCommand::CommitDiff)
        }
        Tag::ReadMemoryFile => typed::<ReadMemoryFileCommand>(frame).map(MemoryCommand::ReadFile),
        Tag::WriteMemoryFile => {
            typed::<WriteMemoryFileCommand>(frame).map(MemoryCommand::WriteFile)
        }
        Tag::DeleteMemoryFile => {
            typed::<DeleteMemoryFileCommand>(frame).map(MemoryCommand::DeleteFile)
        }
        Tag::EnableMemfs => typed::<EnableMemfsCommand>(frame).map(MemoryCommand::EnableMemfs),
        _ => return Ok(None),
    }?;
    Ok(Some(parsed))
}

fn typed<T: DeserializeOwned>(frame: &DecodedFrame) -> Result<T, ProtocolErrorEnvelope> {
    serde_json::from_value(frame.value.clone()).map_err(|_| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "memory_command_invalid",
        "invalid memory command",
        frame.request_id.clone(),
    )
}

fn prepare_backend(backend_dir: &Path) -> Result<PreparedBackend, AppServerError> {
    std::fs::create_dir_all(backend_dir)
        .map_err(|_| AppServerError::Config("memory backend unavailable"))?;
    let root = backend_dir
        .canonicalize()
        .map_err(|_| AppServerError::Config("memory backend unavailable"))?;
    let memfs = GitMemFs::new(root.clone())
        .map_err(|_| AppServerError::Config("memory backend unavailable"))?;
    Ok(PreparedBackend { root, memfs })
}

fn author_for(agent: &AgentId) -> Option<MemFsCommitAuthor> {
    Some(MemFsCommitAuthor {
        name: CommitMessage::new(agent.as_str().to_owned()).ok()?,
        email: CommitMessage::new(format!("{}@letta.com", agent.as_str())).ok()?,
    })
}

fn mutation_frame(
    request: &MutationRequest<'_>,
    success: bool,
    committed: Option<bool>,
    commit_sha: Option<String>,
    error: Option<String>,
) -> MutationFileResponseMessage {
    MutationFileResponseMessage {
        request_id: request.request_id.to_owned(),
        agent_id: request.agent_id.to_owned(),
        path: request.path.to_owned(),
        success,
        committed,
        commit_sha,
        error,
    }
}

impl MutationFileResponseMessage {
    fn with_path(mut self, path: String) -> Self {
        self.path = path;
        self
    }
}

fn failed_list(request_id: &str, error: &str) -> MemoryMessage {
    MemoryMessage::ListResponse(ListMemoryResponseMessage {
        request_id: request_id.to_owned(),
        entries: Vec::new(),
        done: true,
        total: 0,
        success: false,
        error: Some(error.to_owned()),
        memfs_enabled: None,
        memfs_initialized: None,
    })
}

/// Success-shaped terminal frame for disabled or empty listings.
fn disabled_list(request_id: &str) -> MemoryMessage {
    MemoryMessage::ListResponse(ListMemoryResponseMessage {
        request_id: request_id.to_owned(),
        entries: Vec::new(),
        done: true,
        total: 0,
        success: true,
        error: None,
        memfs_enabled: Some(false),
        memfs_initialized: Some(false),
    })
}

/// Success-shaped terminal frame for an exhausted-but-enabled listing.
fn empty_list(request_id: &str) -> MemoryMessage {
    MemoryMessage::ListResponse(ListMemoryResponseMessage {
        request_id: request_id.to_owned(),
        entries: Vec::new(),
        done: true,
        total: 0,
        success: true,
        error: None,
        memfs_enabled: Some(true),
        memfs_initialized: Some(true),
    })
}

fn failed_history(command: &MemoryHistoryCommand, error: &str) -> MemoryMessage {
    MemoryMessage::HistoryResponse(MemoryHistoryResponseMessage {
        request_id: command.request_id.clone(),
        file_path: command.file_path.clone().unwrap_or_default(),
        commits: Vec::new(),
        success: false,
        error: Some(error.to_owned()),
    })
}

fn confine(value: &str) -> Result<RepositoryPath, PathRejection> {
    if value.is_empty() || Path::new(value).is_absolute() {
        return Err(PathRejection::NotRelative);
    }
    RepositoryPath::new(PathBuf::from(value)).map_err(|_| PathRejection::EscapesRoot)
}

fn display(path: &RepositoryPath) -> String {
    path.as_path().to_string_lossy().into_owned()
}

fn decoded_bounded(raw: &str, encoding: MemoryEncoding) -> Result<MemoryFileContent, &'static str> {
    let bytes = decoded_content(raw, encoding)?;
    MemoryFileContent::new(bytes).map_err(|_| "content exceeds the memory file limit")
}

fn decoded_content(raw: &str, encoding: MemoryEncoding) -> Result<Vec<u8>, &'static str> {
    use base64::engine::general_purpose::STANDARD;
    match encoding {
        MemoryEncoding::Utf8 => Ok(raw.as_bytes().to_vec()),
        MemoryEncoding::Base64 => STANDARD
            .decode(raw)
            .map_err(|_| "content is not valid base64"),
    }
}

fn encode_content(bytes: &[u8], encoding: MemoryEncoding) -> String {
    use base64::engine::general_purpose::STANDARD;
    match encoding {
        MemoryEncoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        MemoryEncoding::Base64 => STANDARD.encode(bytes),
    }
}

fn classify_entry(path: &Path) -> Option<EntryKind> {
    match image_mime(path) {
        Some(mime_type) => Some(EntryKind::Image(mime_type)),
        None => is_markdown(path).then_some(EntryKind::Markdown),
    }
}

fn image_mime(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let dotted = format!(".{extension}");
    IMAGE_MIME_BY_EXTENSION
        .iter()
        .find_map(|(name, mime)| (*name == dotted).then_some(*mime))
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

/// Splits pinned-style frontmatter into `(description, body)`.
///
/// ponytail: lines-based reconstruction loses CRLF endings and the trailing
/// newline inside listed bodies only; reads stay byte-exact. Switch to
/// offset-based splitting if the viewer needs exact sizes.
fn split_frontmatter(raw: &[u8]) -> (Option<String>, String) {
    let text = String::from_utf8_lossy(raw).into_owned();
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"));
    let Some(rest) = rest else {
        return (None, text);
    };
    let lines = rest.lines();
    let mut description = None;
    let mut closed = false;
    let mut body_lines: Vec<&str> = Vec::new();
    for line in lines {
        if closed {
            body_lines.push(line);
            continue;
        }
        if line.trim_end_matches('\r') == "---" {
            closed = true;
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(frontmatter_description(value));
        }
    }
    if !closed {
        return (None, text);
    }
    (description, body_lines.join("\n"))
}

fn frontmatter_description(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        serde_json::from_str::<String>(trimmed)
            .unwrap_or_else(|_| trimmed.trim_matches('"').to_owned())
    } else {
        trimmed.to_owned()
    }
}

fn wiki_references(body: &str, source: &str, known: &HashSet<String>) -> Vec<String> {
    let mut references: Vec<String> = Vec::new();
    for raw in wiki_targets(body) {
        let Some(resolved) = resolve_reference(&raw, source, known) else {
            continue;
        };
        if resolved != source && !references.contains(&resolved) {
            references.push(resolved);
        }
    }
    references
}

fn wiki_targets(body: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            break;
        };
        targets.push(after[..end].trim().to_owned());
        rest = &after[end + 2..];
    }
    targets
}

fn resolve_reference(raw: &str, source: &str, known: &HashSet<String>) -> Option<String> {
    let target = raw.split('|').next()?.trim();
    if target.is_empty() || external_reference(target) || target.starts_with('#') {
        return None;
    }
    let target = target.split(['#', '?']).next()?.replace('\\', "/");
    let normalized = if target.starts_with("./") || target.starts_with("../") {
        posix_normalize(&posix_join(&parent_directory(source), &target))
    } else {
        posix_normalize(target.trim_start_matches('/'))
    };
    if normalized.is_empty() {
        return None;
    }
    let with_extension = if is_markdown(Path::new(&normalized)) {
        normalized
    } else {
        format!("{normalized}.md")
    };
    let mut candidates = vec![with_extension.clone()];
    let directory = parent_directory(source);
    if !directory.is_empty() {
        candidates.push(posix_join(&directory, &with_extension));
    }
    if !with_extension.starts_with("system/") {
        candidates.push(posix_join("system", &with_extension));
    }
    candidates
        .into_iter()
        .find(|candidate| known.contains(candidate))
}

fn external_reference(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://") || target.starts_with("mailto:")
}

fn parent_directory(path: &str) -> String {
    path.rsplit_once('/')
        .map_or(String::new(), |(parent, _)| parent.to_owned())
}

fn posix_join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}/{child}")
    }
}

fn posix_normalize(value: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[cfg(test)]
#[path = "memory_commands_tests.rs"]
mod commands;
#[cfg(test)]
#[path = "memory_confinement_tests.rs"]
mod confinement;
#[cfg(test)]
#[path = "memory_revisions_tests.rs"]
mod revisions;
#[cfg(test)]
#[path = "memory_snapshots_tests.rs"]
mod snapshots;
#[cfg(test)]
#[path = "memory_support.rs"]
mod support;
#[cfg(test)]
#[path = "memory_write_push_ordering_tests.rs"]
mod write_push_ordering;

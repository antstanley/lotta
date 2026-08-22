//! WebSocket files command group.
//!
//! Decodes and routes the pinned `search_files`, `grep_in_files`,
//! `list_in_directory`, `get_tree`, `read_file`, `write_file`, `watch_file`,
//! `unwatch_file`, `edit_file`, and `file_ops` commands (the §WebSocket command
//! groups Files row) against one confined workspace root.
//!
//! Confinement reuses the exact shared Task 34/35 units the tool pipeline's
//! [`WorkspaceSandboxGate`](lotta_tools::sandbox::WorkspaceSandboxGate) uses —
//! `canonicalize_invocation_path` plus component-aware `path_within` — so a
//! traversal, symlink escape, or out-of-root absolute path is rejected by the
//! same code that rejects it on the tool path. Raw read, write, and edit
//! effects route through the Task 37 [`FileToolBundle`](lotta_tools::builtin::file::FileToolBundle)
//! listener seam, inheriting its cap-std no-follow effects, atomic writes,
//! CRLF-normalized edits, and text-size caps; the pinned baseline likewise
//! routes `write_file`/`edit_file` through the tool implementations.
//!
//! Watchers are bounded per connection
//! ([`FILE_WATCHERS_PER_CONNECTION_MAX`](crate::ws::files::FILE_WATCHERS_PER_CONNECTION_MAX))
//! and polled every
//! [`FILE_WATCH_POLL_INTERVAL_MS`](crate::ws::files::FILE_WATCH_POLL_INTERVAL_MS);
//! polling collapses rapid save bursts like the pinned 150 ms debounce.
//! Unwatch removes one watcher; connection close removes all of that
//! connection's watchers and cancels their tasks. Every response is
//! serialized once before forwarding and dropped with a warning when its
//! encoded size exceeds
//! [`WS_FRAME_BYTES_MAX`](crate::bounds::WS_FRAME_BYTES_MAX); tree, grep,
//! listing, search, and read bounds below keep every well-formed response far
//! under that transport ceiling.
//!
//! Deliberate simplifications versus the desktop baseline: directory walks
//! never list or follow symlinks (the confined listener has no legitimate
//! use for them), `.gitignore` rules are not parsed (only the pinned
//! ignored-name sets apply), glob filters use a translated wildcard match,
//! and sort order is byte-wise rather than locale-aware.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use base64::Engine as _;
use lotta_protocol::{DecodeOutcome, WsProtocolCommand as Tag};
use lotta_tools::{
    builtin::file::FileToolBundle,
    permissions::{canonicalize_invocation_path, path_within},
};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    bounds::WS_FRAME_BYTES_MAX, error::AppServerError, errors::ProtocolErrorEnvelope,
    framing::DecodedFrame, ws::connection::ConnectionId,
};

/// Maximum entries in one `get_tree` response (pinned parity).
pub const TREE_ENTRIES_MAX: usize = 5_000;
/// Maximum `get_tree` recursion depth accepted from a client.
pub const TREE_DEPTH_MAX: usize = 64;
/// Maximum filename search results returned.
pub const SEARCH_RESULTS_MAX: usize = 200;
/// Filename search results when the client omits `max_results` (pinned parity).
pub const SEARCH_RESULTS_DEFAULT: usize = 5;
/// Maximum filesystem entries visited by one filename search (pinned parity).
pub const SEARCH_VISITS_MAX: usize = 50_000;
/// Maximum grep match rows returned (also the pinned default).
pub const GREP_MATCHES_MAX: usize = 500;
/// Maximum context lines requested around each grep match.
pub const GREP_CONTEXT_LINES_MAX: usize = 20;
/// Context lines around each grep match when the client omits them (pinned parity).
pub const GREP_CONTEXT_LINES_DEFAULT: usize = 2;
/// Maximum bytes retained for any single grep line or context line.
pub const GREP_LINE_BYTES_MAX: usize = 4_096;
/// Maximum bytes read from one grep candidate file.
pub const GREP_FILE_BYTES_MAX: usize = 8 * 1024 * 1024;
/// Hard ceiling on counted matches while scanning, keeping totals bounded.
pub const GREP_TOTAL_MATCHES_SCAN_MAX: usize = 10_000;
/// Maximum compiled regex size for grep queries and glob filters.
pub const GREP_PATTERN_COMPILED_BYTES_MAX: usize = 256 * 1024;
/// Maximum raw bytes accepted for one base64 `read_file` response (pinned parity).
pub const READ_BASE64_BYTES_MAX: usize = 25 * 1024 * 1024;
/// Utf8 read ceiling, reused verbatim from the Task 37 text tooling.
pub const READ_UTF8_BYTES_MAX: usize = lotta_tools::builtin::file::TEXT_FILE_BYTES_MAX;
/// Maximum collected entries for one directory listing before truncation.
pub const LIST_ENTRIES_MAX: usize = 10_000;
/// Maximum live file watchers per connection.
pub const FILE_WATCHERS_PER_CONNECTION_MAX: usize = 64;
/// Watch poll interval; collapses save bursts like the pinned debounce.
pub const FILE_WATCH_POLL_INTERVAL_MS: u64 = 150;

/// Names filtered from every directory listing (pinned parity).
const DIR_IGNORED_NAMES: [&str; 3] = [".DS_Store", ".git", "Thumbs.db"];
/// Scrubbed failure detail for rejected or failed filename searches.
const SEARCH_FAILURE: &str = "Failed to search files";
/// Scrubbed failure detail for rejected or failed content searches.
const GREP_FAILURE: &str = "Failed to search file contents";
/// Scrubbed failure detail for rejected or failed directory listings.
const LIST_FAILURE: &str = "Failed to list directory";
/// Scrubbed failure detail for rejected or failed subtree fetches.
const TREE_FAILURE: &str = "Failed to get tree";
/// Extra directory names skipped by recursive walks (pinned parity).
const RECURSIVE_IGNORED_NAMES: [&str; 16] = [
    ".cache",
    ".letta",
    ".next",
    ".nuxt",
    ".tox",
    ".venv",
    "bower_components",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "out",
    "target",
    "vendor",
    "venv",
    "__pycache__",
];

fn ignored_name(name: &str) -> bool {
    DIR_IGNORED_NAMES.contains(&name)
}

fn recursive_ignored(name: &str) -> bool {
    ignored_name(name) || RECURSIVE_IGNORED_NAMES.contains(&name)
}

fn worktree_blocked(relative: &str) -> bool {
    relative == ".letta/worktrees" || relative.starts_with(".letta/worktrees/")
}

/// Pinned `search_files` command payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchFilesCommand {
    /// Optional search root; absence resolves to the workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Case-insensitive filename substring query.
    pub query: String,
    /// Requested result count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `grep_in_files` command payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GrepInFilesCommand {
    /// Optional search root; absence resolves to the workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Literal or regular-expression content query.
    pub query: String,
    /// Whether `query` is a regular expression.
    #[serde(default)]
    pub is_regex: bool,
    /// Whether the query is case-sensitive.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Whether only whole-word matches count.
    #[serde(default)]
    pub whole_word: bool,
    /// Optional wildcard filename filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// Requested maximum match rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
    /// Requested context lines around each match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_lines: Option<usize>,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `list_in_directory` command payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ListInDirectoryCommand {
    /// Directory to list.
    pub path: String,
    /// Whether the response includes files as well as folders.
    #[serde(default)]
    pub include_files: bool,
    /// Pagination start offset into the combined sorted listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// Pagination page size; absence returns everything collected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Optional response correlation identifier (pinned optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Pinned `get_tree` command payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GetTreeCommand {
    /// Subtree root.
    pub path: String,
    /// Requested recursion depth; zero yields no entries.
    pub depth: i64,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `read_file` command payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReadFileCommand {
    /// File to read.
    pub path: String,
    /// Requested encoding; only `base64` changes decoding of the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `write_file` command payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WriteFileCommand {
    /// File to create or fully replace.
    pub path: String,
    /// Full replacement text content.
    pub content: String,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `watch_file` command payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WatchFileCommand {
    /// File whose modifications push `file_changed` notices.
    pub path: String,
    /// Response correlation identifier (the pinned guard requires it).
    pub request_id: String,
}

/// Pinned `unwatch_file` command payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UnwatchFileCommand {
    /// Previously watched file to stop watching.
    pub path: String,
    /// Response correlation identifier (the pinned guard requires it).
    pub request_id: String,
}

/// Pinned `edit_file` command payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EditFileCommand {
    /// File to edit.
    pub file_path: String,
    /// Exact text replaced by `new_string`.
    pub old_string: String,
    /// Replacement text.
    pub new_string: String,
    /// Whether every occurrence is replaced instead of exactly one.
    #[serde(default)]
    pub replace_all: bool,
    /// Expected replacement count; mismatch rejects the edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_replacements: Option<usize>,
    /// Response correlation identifier.
    pub request_id: String,
}

/// Pinned `file_ops` record shared by the inbound command and outbound notice.
///
/// The pinned guard requires `cg_entries` and `ops` arrays plus a `source`
/// string; `document_content` stays optional. Structural CRDT entries are
/// tolerated for wire compatibility while the listener consumes only
/// `document_content`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileOpsRecord {
    /// Workspace file the operations target.
    pub path: String,
    /// Structural CRDT entries (ignored).
    pub cg_entries: Vec<Value>,
    /// Structural CRDT ops (ignored).
    pub ops: Vec<Value>,
    /// Attribution source echoed by the notice.
    pub source: String,
    /// Full document content applied when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_content: Option<String>,
}

/// The ten concrete files group commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum FilesCommand {
    /// Filename search under one root.
    #[serde(rename = "search_files")]
    Search(SearchFilesCommand),
    /// Content search under one root.
    #[serde(rename = "grep_in_files")]
    Grep(GrepInFilesCommand),
    /// Single-level paginated directory listing.
    #[serde(rename = "list_in_directory")]
    List(ListInDirectoryCommand),
    /// Depth-limited subtree fetch.
    #[serde(rename = "get_tree")]
    Tree(GetTreeCommand),
    /// Whole-file read.
    #[serde(rename = "read_file")]
    Read(ReadFileCommand),
    /// Create-or-replace whole-file write.
    #[serde(rename = "write_file")]
    Write(WriteFileCommand),
    /// Begin watching one file.
    #[serde(rename = "watch_file")]
    Watch(WatchFileCommand),
    /// Stop watching one file.
    #[serde(rename = "unwatch_file")]
    Unwatch(UnwatchFileCommand),
    /// Exact-string replacement edit.
    #[serde(rename = "edit_file")]
    Edit(EditFileCommand),
    /// CRDT document application.
    #[serde(rename = "file_ops")]
    Ops(FileOpsRecord),
}

/// One `path`/`type` entry shared by search and tree responses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TreeEntry {
    /// Workspace-relative POSIX path.
    pub path: String,
    /// Entry kind.
    #[serde(rename = "type")]
    pub kind: TreeEntryKind,
}

/// Kind of one [`TreeEntry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeEntryKind {
    /// Regular file entry.
    File,
    /// Directory entry.
    Dir,
}

/// One content-match row of `grep_in_files_response`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GrepMatch {
    /// Workspace-relative POSIX path of the matching file.
    pub path: String,
    /// One-based line number.
    pub line: usize,
    /// One-based column of the first match on the line.
    pub column: usize,
    /// Exclusive end column of the first match on the line.
    pub column_end: usize,
    /// Matched line text without its newline.
    pub text: String,
    /// Preceding context lines when context was requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Vec<String>>,
    /// Following context lines when context was requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Vec<String>>,
}

/// Pinned `search_files_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct SearchFilesResponse {
    /// Correlation identifier.
    pub request_id: String,
    /// Matching entries in walk order.
    pub files: Vec<TreeEntry>,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `grep_in_files_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct GrepInFilesResponse {
    /// Correlation identifier.
    pub request_id: String,
    /// Operation success.
    pub success: bool,
    /// Returned match rows.
    pub matches: Vec<GrepMatch>,
    /// Total matches seen across the scanned corpus.
    pub total_matches: usize,
    /// Distinct files containing at least one counted match.
    pub total_files: usize,
    /// Whether `matches` is shorter than `total_matches`.
    pub truncated: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `list_in_directory_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct ListInDirectoryResponse {
    /// Listed directory as requested.
    pub path: String,
    /// Page of folder names.
    pub folders: Vec<String>,
    /// Page of file names when `include_files` was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    /// Whether more pages exist beyond this one.
    #[serde(rename = "hasMore")]
    pub has_more: bool,
    /// Total collected entries across both name kinds; omitted on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    /// Operation success.
    pub success: bool,
    /// Correlation identifier when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `get_tree_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct GetTreeResponse {
    /// Subtree root as requested.
    pub path: String,
    /// Correlation identifier.
    pub request_id: String,
    /// Collected entries in breadth-first order.
    pub entries: Vec<TreeEntry>,
    /// Whether deeper or further entries exist beyond the response.
    pub has_more_depth: bool,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `read_file_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct ReadFileResponse {
    /// Correlation identifier.
    pub request_id: String,
    /// File as requested.
    pub path: String,
    /// Decoded content, or null on failure.
    pub content: Option<String>,
    /// Encoding echoed back to the client.
    pub encoding: String,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `write_file_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct WriteFileResponse {
    /// Correlation identifier.
    pub request_id: String,
    /// File as requested.
    pub path: String,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `edit_file_response` payload.
#[derive(Clone, Debug, Serialize)]
pub struct EditFileResponse {
    /// Correlation identifier.
    pub request_id: String,
    /// File as requested.
    pub file_path: String,
    /// Human summary, or null on failure.
    pub message: Option<String>,
    /// Replacements applied.
    pub replacements: usize,
    /// One-based line of the first replacement on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    /// Operation success.
    pub success: bool,
    /// Scrubbed failure detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pinned `file_changed` notice emitted by watchers.
#[derive(Clone, Debug, Serialize)]
pub struct FileChangedNotice {
    /// Watched file as requested.
    pub path: String,
    /// Modification time in epoch milliseconds.
    #[serde(rename = "lastModified")]
    pub last_modified: u64,
}

/// The nine outbound files group messages.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum FilesMessage {
    /// Filename search result.
    #[serde(rename = "search_files_response")]
    SearchFiles(SearchFilesResponse),
    /// Content search result.
    #[serde(rename = "grep_in_files_response")]
    GrepInFiles(GrepInFilesResponse),
    /// Directory listing result.
    #[serde(rename = "list_in_directory_response")]
    ListInDirectory(ListInDirectoryResponse),
    /// Subtree fetch result.
    #[serde(rename = "get_tree_response")]
    GetTree(GetTreeResponse),
    /// Whole-file read result.
    #[serde(rename = "read_file_response")]
    ReadFile(ReadFileResponse),
    /// Whole-file write result.
    #[serde(rename = "write_file_response")]
    WriteFile(WriteFileResponse),
    /// Applied document content notice.
    #[serde(rename = "file_ops")]
    FileOps(FileOpsRecord),
    /// Exact-string edit result.
    #[serde(rename = "edit_file_response")]
    EditFile(EditFileResponse),
    /// Watched-file modification notice.
    #[serde(rename = "file_changed")]
    Changed(FileChangedNotice),
}

/// Push callback delivering one encoded outbound message to one connection.
pub type FilesForwarder =
    Arc<dyn Fn(ConnectionId, FilesMessage) -> Result<(), AppServerError> + Send + Sync>;

#[cfg(test)]
pub(crate) fn inert_forwarder() -> FilesForwarder {
    Arc::new(|_, _| Ok(()))
}

struct WatchRecord {
    cancel: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

type Watches = HashMap<(ConnectionId, PathBuf), WatchRecord>;

/// Applies wire files commands to one confined workspace root.
pub struct FilesBridge {
    workspace_root: PathBuf,
    bundle: Arc<FileToolBundle>,
    forward: FilesForwarder,
    watches: Arc<Mutex<Watches>>,
    poll_interval_ms: u64,
}

impl FilesBridge {
    /// Creates a bridge over an existing canonical workspace root.
    ///
    /// The artifact root is created when absent because Task 37 bundle
    /// construction requires both canonical ordinary directories.
    ///
    /// # Errors
    /// Returns [`AppServerError::Config`] when either root cannot be prepared
    /// as a canonical ordinary directory.
    pub fn new(
        forward: FilesForwarder,
        workspace_dir: &Path,
        artifacts_dir: &Path,
    ) -> Result<Self, AppServerError> {
        Self::with_poll_interval(
            forward,
            workspace_dir,
            artifacts_dir,
            FILE_WATCH_POLL_INTERVAL_MS,
        )
    }

    /// Creates a bridge with an explicit watch poll interval for tests.
    ///
    /// # Errors
    /// Returns [`AppServerError::Config`] when either root cannot be prepared.
    pub fn with_poll_interval(
        forward: FilesForwarder,
        workspace_dir: &Path,
        artifacts_dir: &Path,
        poll_interval_ms: u64,
    ) -> Result<Self, AppServerError> {
        std::fs::create_dir_all(artifacts_dir)
            .map_err(|_| AppServerError::Config("files artifact root unavailable"))?;
        let bundle = FileToolBundle::new(workspace_dir, artifacts_dir)
            .map_err(|_| AppServerError::Config("files workspace unavailable"))?;
        let workspace_root = workspace_dir
            .canonicalize()
            .map_err(|_| AppServerError::Config("files workspace unavailable"))?;
        Ok(Self {
            workspace_root,
            bundle: Arc::new(bundle),
            forward,
            watches: Arc::new(Mutex::new(Watches::new())),
            poll_interval_ms,
        })
    }

    /// Routes one decoded command; silent commands produce no response frame.
    pub fn handle(&self, connection: ConnectionId, command: &FilesCommand) {
        match command {
            FilesCommand::Search(payload) => self.search(connection, payload),
            FilesCommand::Grep(payload) => self.grep(connection, payload),
            FilesCommand::List(payload) => self.list(connection, payload),
            FilesCommand::Tree(payload) => self.tree(connection, payload),
            FilesCommand::Read(payload) => self.read(connection, payload),
            FilesCommand::Write(payload) => self.write(connection, payload),
            FilesCommand::Watch(payload) => self.watch(connection, payload),
            FilesCommand::Unwatch(payload) => self.unwatch(connection, payload),
            FilesCommand::Edit(payload) => self.edit(connection, payload),
            FilesCommand::Ops(payload) => self.ops(payload),
        }
    }

    /// Cancels and removes every watcher owned by the closing connection.
    pub fn disconnect(&self, connection: ConnectionId) {
        let removed: Vec<WatchRecord> = {
            let mut guard = lock(&self.watches);
            let keys: Vec<_> = guard
                .keys()
                .filter(|(owner, _)| *owner == connection)
                .cloned()
                .collect();
            keys.iter().filter_map(|key| guard.remove(key)).collect()
        };
        for record in removed {
            record.cancel.cancel();
            record.task.abort();
        }
    }

    /// Live watcher count owned by one connection; test introspection only.
    #[cfg(test)]
    pub(crate) fn watcher_count(&self, connection: ConnectionId) -> usize {
        lock(&self.watches)
            .keys()
            .filter(|(owner, _)| *owner == connection)
            .count()
    }

    fn confine(&self, value: &str) -> Result<PathBuf, ()> {
        let canonical =
            canonicalize_invocation_path(&self.workspace_root, value).map_err(|_| ())?;
        path_within(&canonical, &self.workspace_root)
            .then_some(canonical)
            .ok_or(())
    }

    fn emit(&self, connection: ConnectionId, message: FilesMessage) {
        match serde_json::to_string(&message) {
            Ok(body) if body.len() <= WS_FRAME_BYTES_MAX => {
                let _ = (self.forward)(connection, message);
            }
            Ok(_) => tracing::warn!("files response exceeded the frame bound and was dropped"),
            Err(_) => tracing::warn!("files response failed to encode"),
        }
    }

    fn search(&self, connection: ConnectionId, command: &SearchFilesCommand) {
        let Ok(root) = self.confine(command.cwd.as_deref().unwrap_or(".")) else {
            self.emit(connection, failed_search(&command.request_id));
            return;
        };
        let max = command
            .max_results
            .unwrap_or(SEARCH_RESULTS_DEFAULT)
            .clamp(1, SEARCH_RESULTS_MAX);
        let query = command.query.trim().to_lowercase();
        let files = walk_search(&root, &query, max);
        self.emit(
            connection,
            FilesMessage::SearchFiles(SearchFilesResponse {
                request_id: command.request_id.clone(),
                files,
                success: true,
                error: None,
            }),
        );
    }

    fn grep(&self, connection: ConnectionId, command: &GrepInFilesCommand) {
        let Ok(root) = self.confine(command.cwd.as_deref().unwrap_or(".")) else {
            self.emit(connection, failed_grep(&command.request_id));
            return;
        };
        if command.query.is_empty() {
            self.emit(connection, empty_grep(&command.request_id));
            return;
        }
        let Some(pattern) = compile_query(command) else {
            self.emit(connection, failed_grep(&command.request_id));
            return;
        };
        let filter = command.glob.as_deref().and_then(compile_glob);
        let context = command
            .context_lines
            .unwrap_or(GREP_CONTEXT_LINES_DEFAULT)
            .min(GREP_CONTEXT_LINES_MAX);
        let max = command
            .max_results
            .unwrap_or(GREP_MATCHES_MAX)
            .clamp(1, GREP_MATCHES_MAX);
        let outcome = run_grep(
            &self.bundle,
            &self.workspace_root,
            &root,
            &pattern,
            filter.as_ref(),
            context,
            max,
        );
        let truncated = outcome.overflow || outcome.total > outcome.records.len();
        self.emit(
            connection,
            FilesMessage::GrepInFiles(GrepInFilesResponse {
                request_id: command.request_id.clone(),
                success: true,
                matches: outcome.records,
                total_matches: outcome.total,
                total_files: outcome.files,
                truncated,
                error: None,
            }),
        );
    }

    fn list(&self, connection: ConnectionId, command: &ListInDirectoryCommand) {
        let Ok(root) = self.confine(&command.path) else {
            self.emit(connection, failed_list(command));
            return;
        };
        let Some((folders, files, capped)) = collect_listing(&root, command.include_files) else {
            self.emit(connection, failed_list(command));
            return;
        };
        let mut combined = folders.clone();
        combined.extend(files.clone());
        let total = combined.len();
        let offset = command.offset.unwrap_or(0).min(total);
        let limit = command.limit.unwrap_or(total - offset).min(total - offset);
        let page: Vec<String> = combined[offset..offset + limit].to_vec();
        let folder_set: HashSet<&String> = folders.iter().collect();
        let page_folders: Vec<String> = page
            .iter()
            .filter(|name| folder_set.contains(name))
            .cloned()
            .collect();
        let page_files: Vec<String> = page
            .iter()
            .filter(|name| !folder_set.contains(name))
            .cloned()
            .collect();
        self.emit(
            connection,
            FilesMessage::ListInDirectory(ListInDirectoryResponse {
                path: command.path.clone(),
                folders: page_folders,
                files: command.include_files.then_some(page_files),
                has_more: capped || offset + limit < total,
                total: Some(total),
                success: true,
                request_id: command.request_id.clone(),
                error: None,
            }),
        );
    }

    fn tree(&self, connection: ConnectionId, command: &GetTreeCommand) {
        let Ok(root) = self.confine(&command.path) else {
            self.emit(connection, failed_tree(&command.path, &command.request_id));
            return;
        };
        let depth = if command.depth < 0 {
            0
        } else {
            usize::try_from(command.depth)
                .unwrap_or(TREE_DEPTH_MAX)
                .min(TREE_DEPTH_MAX)
        };
        let (entries, has_more_depth) = walk_tree(&root, depth);
        self.emit(
            connection,
            FilesMessage::GetTree(GetTreeResponse {
                path: command.path.clone(),
                request_id: command.request_id.clone(),
                entries,
                has_more_depth,
                success: true,
                error: None,
            }),
        );
    }

    fn read(&self, connection: ConnectionId, command: &ReadFileCommand) {
        let encoding = command.encoding.clone().unwrap_or_else(|| "utf8".into());
        let failure = |error: String| {
            self.emit(
                connection,
                FilesMessage::ReadFile(ReadFileResponse {
                    request_id: command.request_id.clone(),
                    path: command.path.clone(),
                    content: None,
                    encoding: encoding.clone(),
                    success: false,
                    error: Some(error),
                }),
            );
        };
        let Ok(canonical) = self.confine(&command.path) else {
            return failure("Failed to read file.".into());
        };
        if encoding == "base64" {
            self.read_base64(connection, command, &encoding, &canonical, failure);
        } else {
            let content = self
                .bundle
                .listener_read_bytes(&command.path, READ_UTF8_BYTES_MAX)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok());
            match content {
                Some(text) => self.emit_read(connection, command, &encoding, Some(text)),
                None => failure("Failed to read file.".into()),
            }
        }
    }

    fn read_base64(
        &self,
        connection: ConnectionId,
        command: &ReadFileCommand,
        encoding: &str,
        canonical: &Path,
        failure: impl Fn(String),
    ) {
        let oversized = std::fs::symlink_metadata(canonical).is_ok_and(|meta| {
            meta.len() > u64::try_from(READ_BASE64_BYTES_MAX).unwrap_or(u64::MAX)
        });
        if oversized {
            failure(format!(
                "File too large for base64 read (max {}MB)",
                READ_BASE64_BYTES_MAX / (1024 * 1024)
            ));
            return;
        }
        match self
            .bundle
            .listener_read_bytes(&command.path, READ_BASE64_BYTES_MAX)
        {
            Ok(bytes) => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                self.emit_read(connection, command, encoding, Some(encoded));
            }
            Err(_) => failure("Failed to read file.".into()),
        }
    }

    fn emit_read(
        &self,
        connection: ConnectionId,
        command: &ReadFileCommand,
        encoding: &str,
        content: Option<String>,
    ) {
        self.emit(
            connection,
            FilesMessage::ReadFile(ReadFileResponse {
                request_id: command.request_id.clone(),
                path: command.path.clone(),
                content,
                encoding: encoding.to_owned(),
                success: true,
                error: None,
            }),
        );
    }

    fn write(&self, connection: ConnectionId, command: &WriteFileCommand) {
        if command.content.len() > READ_UTF8_BYTES_MAX || self.confine(&command.path).is_err() {
            self.emit(
                connection,
                FilesMessage::WriteFile(WriteFileResponse {
                    request_id: command.request_id.clone(),
                    path: command.path.clone(),
                    success: false,
                    error: Some("Failed to write file.".into()),
                }),
            );
            return;
        }
        let result = self
            .bundle
            .listener_write_text(&command.path, &command.content);
        self.emit(
            connection,
            FilesMessage::WriteFile(WriteFileResponse {
                request_id: command.request_id.clone(),
                path: command.path.clone(),
                success: result.is_ok(),
                error: result.err().map(|_| "Failed to write file.".into()),
            }),
        );
    }

    fn watch(&self, connection: ConnectionId, command: &WatchFileCommand) {
        let Ok(canonical) = self.confine(&command.path) else {
            tracing::debug!(path = %command.path, "watch path rejected by confinement");
            return;
        };
        let key = (connection, canonical);
        {
            let guard = lock(&self.watches);
            if guard.contains_key(&key)
                || guard
                    .keys()
                    .filter(|(owner, _)| *owner == connection)
                    .count()
                    >= FILE_WATCHERS_PER_CONNECTION_MAX
            {
                tracing::debug!(connection, "watcher set full or already watching");
                return;
            }
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        let initial = modified_ms(&key.1);
        let task = self.spawn_watcher(
            connection,
            key.clone(),
            command.path.clone(),
            cancel.clone(),
            initial,
        );
        lock(&self.watches).insert(key, WatchRecord { cancel, task });
    }

    fn spawn_watcher(
        &self,
        connection: ConnectionId,
        key: (ConnectionId, PathBuf),
        echo_path: String,
        cancel: tokio_util::sync::CancellationToken,
        initial: Option<u64>,
    ) -> tokio::task::JoinHandle<()> {
        let forward = Arc::clone(&self.forward);
        let watches = Arc::clone(&self.watches);
        let poll = self.poll_interval_ms;
        tokio::spawn(async move {
            let mut last = initial;
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = tokio::time::sleep(Duration::from_millis(poll)) => {
                        match modified_ms(&key.1) {
                            Some(modified) if Some(modified) != last => {
                                last = Some(modified);
                                let _ = forward(
                                    connection,
                                    FilesMessage::Changed(FileChangedNotice {
                                        path: echo_path.clone(),
                                        last_modified: modified,
                                    }),
                                );
                            }
                            Some(_) => {}
                            None => {
                                lock(&watches).remove(&key);
                                break;
                            }
                        }
                    }
                }
            }
        })
    }

    fn unwatch(&self, connection: ConnectionId, command: &UnwatchFileCommand) {
        let Ok(canonical) = self.confine(&command.path) else {
            return;
        };
        if let Some(record) = lock(&self.watches).remove(&(connection, canonical)) {
            record.cancel.cancel();
            record.task.abort();
        }
    }

    fn edit(&self, connection: ConnectionId, command: &EditFileCommand) {
        let failure = |error: &str| {
            self.emit(
                connection,
                FilesMessage::EditFile(EditFileResponse {
                    request_id: command.request_id.clone(),
                    file_path: command.file_path.clone(),
                    message: None,
                    replacements: 0,
                    start_line: None,
                    success: false,
                    error: Some(error.to_owned()),
                }),
            );
        };
        if self.confine(&command.file_path).is_err() {
            failure("Failed to edit file.");
            return;
        }
        match self.bundle.listener_edit(
            &command.file_path,
            &command.old_string,
            &command.new_string,
            command.replace_all,
            command.expected_replacements,
        ) {
            Ok(report) => {
                if report.replacements > 0 {
                    self.emit_ops_notice(connection, command);
                }
                self.emit(
                    connection,
                    FilesMessage::EditFile(EditFileResponse {
                        request_id: command.request_id.clone(),
                        file_path: command.file_path.clone(),
                        message: Some("File edited successfully.".into()),
                        replacements: report.replacements,
                        start_line: Some(report.start_line),
                        success: true,
                        error: None,
                    }),
                );
            }
            Err(_) => failure("Failed to edit file."),
        }
    }

    /// Best-effort `file_ops` content notice to the editing connection, per
    /// the pinned baseline which pushes new document content before the
    /// `edit_file_response` frame.
    fn emit_ops_notice(&self, connection: ConnectionId, command: &EditFileCommand) {
        if let Ok(bytes) = self
            .bundle
            .listener_read_bytes(&command.file_path, READ_UTF8_BYTES_MAX)
            && let Ok(document_content) = String::from_utf8(bytes)
        {
            self.emit(
                connection,
                FilesMessage::FileOps(FileOpsRecord {
                    path: command.file_path.clone(),
                    cg_entries: Vec::new(),
                    ops: Vec::new(),
                    source: "agent".into(),
                    document_content: Some(document_content),
                }),
            );
        }
    }

    fn ops(&self, command: &FileOpsRecord) {
        let Some(content) = command.document_content.as_ref() else {
            return;
        };
        if content.len() > READ_UTF8_BYTES_MAX || self.confine(&command.path).is_err() {
            tracing::warn!(path = %command.path, "file_ops write rejected");
            return;
        }
        if let Err(error) = self.bundle.listener_write_text(&command.path, content) {
            tracing::warn!(path = %command.path, ?error, "file_ops write failed");
        }
    }
}

/// Decodes an already bounded and classified frame without parsing text again.
///
/// Payload validation mirrors the pinned protocol guards: `read_file`
/// accepts only the `utf8`/`base64` encodings and `edit_file` rejects a
/// non-positive `expected_replacements`.
///
/// # Errors
/// Returns a correlated protocol error for malformed known files commands.
pub fn decode(frame: &DecodedFrame) -> Result<Option<FilesCommand>, ProtocolErrorEnvelope> {
    let DecodeOutcome::Accepted(tag) = &frame.effects.outcome else {
        return Ok(None);
    };
    let decoded = match tag {
        Tag::SearchFiles => from_payload::<SearchFilesCommand>(frame).map(FilesCommand::Search),
        Tag::GrepInFiles => from_payload::<GrepInFilesCommand>(frame).map(FilesCommand::Grep),
        Tag::ListInDirectory => {
            from_payload::<ListInDirectoryCommand>(frame).map(FilesCommand::List)
        }
        Tag::GetTree => from_payload::<GetTreeCommand>(frame).map(FilesCommand::Tree),
        Tag::ReadFile => from_payload::<ReadFileCommand>(frame)
            .and_then(|command| valid_encoding(&command).map(|()| FilesCommand::Read(command))),
        Tag::WriteFile => from_payload::<WriteFileCommand>(frame).map(FilesCommand::Write),
        Tag::WatchFile => from_payload::<WatchFileCommand>(frame).map(FilesCommand::Watch),
        Tag::UnwatchFile => from_payload::<UnwatchFileCommand>(frame).map(FilesCommand::Unwatch),
        Tag::EditFile => from_payload::<EditFileCommand>(frame)
            .and_then(|command| valid_expectations(&command).map(|()| FilesCommand::Edit(command))),
        Tag::FileOps => from_payload::<FileOpsRecord>(frame).map(FilesCommand::Ops),
        _ => return Ok(None),
    };
    decoded.map(Some).map_err(|()| invalid(frame))
}

fn invalid(frame: &DecodedFrame) -> ProtocolErrorEnvelope {
    ProtocolErrorEnvelope::new(
        "files_command_invalid",
        "invalid files command",
        frame.request_id.clone(),
    )
}

fn from_payload<T: DeserializeOwned>(frame: &DecodedFrame) -> Result<T, ()> {
    serde_json::from_value(frame.value.clone()).map_err(|_| ())
}

fn valid_encoding(command: &ReadFileCommand) -> Result<(), ()> {
    match command.encoding.as_deref() {
        None | Some("utf8" | "base64") => Ok(()),
        _ => Err(()),
    }
}

fn valid_expectations(command: &EditFileCommand) -> Result<(), ()> {
    if command
        .expected_replacements
        .is_some_and(|count| count == 0)
    {
        Err(())
    } else {
        Ok(())
    }
}

fn lock(watches: &Mutex<Watches>) -> MutexGuard<'_, Watches> {
    watches.lock().unwrap_or_else(PoisonError::into_inner)
}

fn modified_ms(path: &Path) -> Option<u64> {
    let modified = std::fs::symlink_metadata(path).ok()?.modified().ok()?;
    let elapsed = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    u64::try_from(elapsed.as_millis()).ok()
}

/// Renders a workspace-relative path with POSIX separators.
fn posix(path: &Path) -> String {
    let mut output = String::new();
    for component in path.components() {
        if !output.is_empty() {
            output.push('/');
        }
        output.push_str(component.as_os_str().to_string_lossy().as_ref());
    }
    output
}

fn matches_search(query: &str, relative: &str) -> bool {
    query.is_empty() || relative.to_lowercase().contains(query)
}

/// Breadth-first filename search over one confined root.
///
/// Directory entries come from `read_dir`, so symlink entries are seen as
/// neither files nor directories and are never followed; visited entries are
/// capped at [`SEARCH_VISITS_MAX`] like the pinned walk.
fn walk_search(root: &Path, query: &str, max: usize) -> Vec<TreeEntry> {
    let mut results = Vec::new();
    let mut queue: VecDeque<(PathBuf, PathBuf)> = VecDeque::new();
    queue.push_back((root.to_owned(), PathBuf::new()));
    let mut visited = 0usize;
    while visited < SEARCH_VISITS_MAX
        && let Some((directory, relative)) = queue.pop_front()
    {
        let Ok(children) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut children: Vec<std::fs::DirEntry> = children.flatten().collect();
        children.sort_by_key(std::fs::DirEntry::file_name);
        for child in children {
            if visited >= SEARCH_VISITS_MAX {
                break;
            }
            let Ok(kind) = child.file_type() else {
                continue;
            };
            let Some(name) = child.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if ignored_name(&name) {
                continue;
            }
            let child_relative = relative.join(&name);
            let display = posix(&child_relative);
            if kind.is_dir() {
                if recursive_ignored(&name) || worktree_blocked(&display) {
                    continue;
                }
                visited += 1;
                if matches_search(query, &display) {
                    results.push(TreeEntry {
                        path: display.clone(),
                        kind: TreeEntryKind::Dir,
                    });
                    if results.len() >= max {
                        return results;
                    }
                }
                queue.push_back((directory.join(&name), child_relative));
            } else if kind.is_file() && !worktree_blocked(&display) {
                visited += 1;
                if matches_search(query, &display) {
                    results.push(TreeEntry {
                        path: display,
                        kind: TreeEntryKind::File,
                    });
                    if results.len() >= max {
                        return results;
                    }
                }
            }
        }
    }
    results
}

/// Collects one single-level listing sorted bytewise, capped at
/// [`LIST_ENTRIES_MAX`] with a truncation flag. A failed read signals
/// [`None`] so the handler answers with a scrubbed failure like the pinned
/// catch block.
fn collect_listing(root: &Path, include_files: bool) -> Option<(Vec<String>, Vec<String>, bool)> {
    let mut folders = Vec::new();
    let mut files = Vec::new();
    let children = std::fs::read_dir(root).ok()?;
    for child in children.flatten() {
        let Ok(kind) = child.file_type() else {
            continue;
        };
        let Some(name) = child.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        if ignored_name(&name) {
            continue;
        }
        if kind.is_dir() {
            folders.push(name);
        } else if kind.is_file() && include_files {
            files.push(name);
        }
    }
    folders.sort();
    files.sort();
    let mut capped = false;
    if folders.len() + files.len() > LIST_ENTRIES_MAX {
        capped = true;
        folders.truncate(LIST_ENTRIES_MAX);
        files.truncate(LIST_ENTRIES_MAX - folders.len());
    }
    Some((folders, files, capped))
}

/// Breadth-first depth-limited subtree fetch mirroring the pinned walk,
/// including its [`TREE_ENTRIES_MAX`] early stop and `has_more_depth` flag.
fn walk_tree(root: &Path, depth_max: usize) -> (Vec<TreeEntry>, bool) {
    let mut entries = Vec::new();
    if depth_max == 0 {
        return (entries, false);
    }
    let mut has_more = false;
    let mut queue: VecDeque<(PathBuf, String, usize)> = VecDeque::new();
    queue.push_back((root.to_owned(), String::new(), 0));
    while let Some((directory, relative, depth)) = queue.pop_front() {
        if depth >= depth_max {
            has_more |= !relative.is_empty();
            continue;
        }
        let Ok(children) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut children: Vec<std::fs::DirEntry> = children.flatten().collect();
        children.sort_by_key(std::fs::DirEntry::file_name);
        for child in children {
            let Ok(kind) = child.file_type() else {
                continue;
            };
            let Some(name) = child.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if ignored_name(&name) {
                continue;
            }
            let display = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if kind.is_dir() {
                if recursive_ignored(&name) || worktree_blocked(&display) {
                    continue;
                }
                entries.push(TreeEntry {
                    path: display.clone(),
                    kind: TreeEntryKind::Dir,
                });
                if entries.len() >= TREE_ENTRIES_MAX {
                    return (entries, true);
                }
                queue.push_back((directory.join(&name), display, depth + 1));
            } else if kind.is_file() && !worktree_blocked(&display) {
                entries.push(TreeEntry {
                    path: display,
                    kind: TreeEntryKind::File,
                });
                if entries.len() >= TREE_ENTRIES_MAX {
                    return (entries, true);
                }
            }
        }
    }
    (entries, has_more)
}

/// One file's grep contribution before response assembly.
struct FileGrepOutcome {
    rows: Vec<GrepMatch>,
    counted: usize,
}

fn compile_query(command: &GrepInFilesCommand) -> Option<Regex> {
    let source = if command.is_regex {
        command.query.clone()
    } else {
        regex::escape(&command.query)
    };
    let wrapped = if command.whole_word {
        format!(r"\b(?:{source})\b")
    } else {
        source
    };
    RegexBuilder::new(&wrapped)
        .case_insensitive(!command.case_sensitive)
        .size_limit(GREP_PATTERN_COMPILED_BYTES_MAX)
        .build()
        .ok()
}

/// Translates a pinned ripgrep-style glob into an anchored regex. Globs
/// without `/` match any file name component at any depth, like `--glob`.
fn compile_glob(pattern: &str) -> Option<Regex> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut source = String::from("^");
    let mut chars = trimmed.chars().peekable();
    while let Some(current) = chars.next() {
        match current {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                source.push_str(".*");
            }
            '*' => source.push_str("[^/]*"),
            '?' => source.push_str("[^/]"),
            other => source.push_str(&regex::escape(&other.to_string())),
        }
        if source.len() > GREP_PATTERN_COMPILED_BYTES_MAX {
            return None;
        }
    }
    source.push('$');
    RegexBuilder::new(&source)
        .size_limit(GREP_PATTERN_COMPILED_BYTES_MAX)
        .build()
        .ok()
}

fn grep_candidate(
    relative_display: &str,
    content: &str,
    pattern: &Regex,
    context_lines: usize,
    remaining: usize,
) -> FileGrepOutcome {
    let lines: Vec<&str> = content.lines().collect();
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| pattern.is_match(line))
        .map(|(index, _)| index)
        .collect();
    let counted = hits.len();
    let mut rows = Vec::new();
    for index in hits {
        if rows.len() >= remaining {
            break;
        }
        let line = bounded_line(lines[index]);
        let found = pattern.find(line);
        let column = found.map_or(1, |match_start| {
            line[..match_start.start()].chars().count() + 1
        });
        let column_end =
            column + found.map_or(0, |match_start| match_start.as_str().chars().count());
        let before = (context_lines > 0).then(|| {
            let start = index.saturating_sub(context_lines);
            lines[start..index]
                .iter()
                .map(|text| bounded_line(text).to_owned())
                .collect()
        });
        let after = (context_lines > 0).then(|| {
            let end = (index + 1 + context_lines).min(lines.len());
            lines[index + 1..end]
                .iter()
                .map(|text| bounded_line(text).to_owned())
                .collect()
        });
        rows.push(GrepMatch {
            path: relative_display.to_owned(),
            line: index + 1,
            column,
            column_end,
            text: line.to_owned(),
            before,
            after,
        });
    }
    FileGrepOutcome { rows, counted }
}

fn bounded_line(line: &str) -> &str {
    if line.len() <= GREP_LINE_BYTES_MAX {
        return line;
    }
    let mut end = GREP_LINE_BYTES_MAX;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

/// Collects candidate files under one confined root in breadth-first order,
/// pruning ignored names and applying the optional glob filter, mirroring the
/// pinned walk's ordering and skip rules.
fn grep_candidates(root: &Path, filter: Option<&Regex>) -> Vec<(String, PathBuf)> {
    let mut candidates = Vec::new();
    let mut queue: VecDeque<(PathBuf, PathBuf)> = VecDeque::new();
    queue.push_back((root.to_owned(), PathBuf::new()));
    while let Some((directory, relative)) = queue.pop_front() {
        let Ok(children) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut children: Vec<std::fs::DirEntry> = children.flatten().collect();
        children.sort_by_key(std::fs::DirEntry::file_name);
        for child in children {
            let Ok(kind) = child.file_type() else {
                continue;
            };
            let Some(name) = child.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if ignored_name(&name) {
                continue;
            }
            let child_relative = relative.join(&name);
            let display = posix(&child_relative);
            if kind.is_dir() {
                if recursive_ignored(&name) || worktree_blocked(&display) {
                    continue;
                }
                queue.push_back((directory.join(&name), child_relative));
            } else if kind.is_file()
                && !worktree_blocked(&display)
                && filter.is_none_or(|glob| glob.is_match(&display))
            {
                candidates.push((display, child_relative));
            }
        }
    }
    candidates
}

/// Sequential content scan under one confined root routed through the Task 37
/// no-follow bounded read seam. Counted matches stop at
/// [`GREP_TOTAL_MATCHES_SCAN_MAX`]; returned rows stop at the client maximum.
fn run_grep(
    bundle: &FileToolBundle,
    workspace_root: &Path,
    root: &Path,
    pattern: &Regex,
    filter: Option<&Regex>,
    context_lines: usize,
    max: usize,
) -> GrepOutcome {
    let mut records = Vec::new();
    let mut total = 0usize;
    let mut files_hit = 0usize;
    let mut overflow = false;
    for (display, relative) in grep_candidates(root, filter) {
        if total >= GREP_TOTAL_MATCHES_SCAN_MAX {
            overflow = true;
            break;
        }
        let Some(content) = read_grep_candidate(bundle, workspace_root, root, &relative) else {
            continue;
        };
        let outcome = grep_candidate(
            &display,
            &content,
            pattern,
            context_lines,
            max - records.len(),
        );
        files_hit += usize::from(outcome.counted > 0);
        total += outcome.counted;
        records.extend(outcome.rows);
    }
    if total > GREP_TOTAL_MATCHES_SCAN_MAX {
        overflow = true;
    }
    GrepOutcome {
        records,
        total,
        files: files_hit,
        overflow,
    }
}

fn read_grep_candidate(
    bundle: &FileToolBundle,
    workspace_root: &Path,
    root: &Path,
    relative_to_root: &Path,
) -> Option<String> {
    let absolute = root.join(relative_to_root);
    let workspace_relative = absolute.strip_prefix(workspace_root).ok()?;
    let value = posix(workspace_relative);
    let bytes = bundle
        .listener_read_bytes(&value, GREP_FILE_BYTES_MAX)
        .ok()?;
    String::from_utf8(bytes).ok()
}

struct GrepOutcome {
    records: Vec<GrepMatch>,
    total: usize,
    files: usize,
    overflow: bool,
}

fn failed_search(request_id: &str) -> FilesMessage {
    FilesMessage::SearchFiles(SearchFilesResponse {
        request_id: request_id.to_owned(),
        files: Vec::new(),
        success: false,
        error: Some(SEARCH_FAILURE.into()),
    })
}

fn failed_grep(request_id: &str) -> FilesMessage {
    FilesMessage::GrepInFiles(GrepInFilesResponse {
        request_id: request_id.to_owned(),
        success: false,
        matches: Vec::new(),
        total_matches: 0,
        total_files: 0,
        truncated: false,
        error: Some(GREP_FAILURE.into()),
    })
}

fn empty_grep(request_id: &str) -> FilesMessage {
    FilesMessage::GrepInFiles(GrepInFilesResponse {
        request_id: request_id.to_owned(),
        success: true,
        matches: Vec::new(),
        total_matches: 0,
        total_files: 0,
        truncated: false,
        error: None,
    })
}

fn failed_list(command: &ListInDirectoryCommand) -> FilesMessage {
    FilesMessage::ListInDirectory(ListInDirectoryResponse {
        path: command.path.clone(),
        folders: Vec::new(),
        files: None,
        has_more: false,
        total: None,
        success: false,
        request_id: command.request_id.clone(),
        error: Some(LIST_FAILURE.into()),
    })
}

fn failed_tree(path: &str, request_id: &str) -> FilesMessage {
    FilesMessage::GetTree(GetTreeResponse {
        path: path.to_owned(),
        request_id: request_id.to_owned(),
        entries: Vec::new(),
        has_more_depth: false,
        success: false,
        error: Some(TREE_FAILURE.into()),
    })
}

#[cfg(test)]
#[path = "files_commands_tests.rs"]
mod commands;
#[cfg(test)]
#[path = "files_confinement_tests.rs"]
mod confinement;
#[cfg(test)]
#[path = "files_result_bounds_tests.rs"]
mod result_bounds;
#[cfg(test)]
#[path = "files_support.rs"]
mod support;
#[cfg(test)]
#[path = "files_watchers_tests.rs"]
mod watchers;

use super::compile::{PromptCompiler, collect_paths_with_limits, retain_with_limit};
use super::test_support::inputs;
use lotta_domain::AgentId;
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{
    CommitMessage, DiffChunk, InitialMemoryBlocks, MemoryFileContent, RepositoryPath, RevisionId,
    WorktreeId,
};
use lotta_runtime::ports::{MemFsHistoryEntry, MemFsPort, MemFsStatus, MemFsTreeEntry, PortFuture};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct EvidenceMemFs {
    state: Arc<State>,
}

struct State {
    revision: RevisionId,
    later_revision: RevisionId,
    status_calls: AtomicUsize,
    tree_calls: AtomicUsize,
    file_calls: AtomicUsize,
    tree_revisions: Mutex<Vec<RevisionId>>,
    file_revisions: Mutex<Vec<RevisionId>>,
    files: Mutex<BTreeMap<RepositoryPath, FileMode>>,
    tree_cancel: Mutex<Option<CancellationToken>>,
}

#[derive(Clone)]
enum FileMode {
    Value(MemoryFileContent),
    Error(ErrorMode),
    Cancel(MemoryFileContent, CancellationToken),
}

#[derive(Clone, Copy)]
enum ErrorMode {
    Permission,
    Limit,
    Adapter,
    Conflict,
    NotFound,
}

impl EvidenceMemFs {
    fn new(files: BTreeMap<RepositoryPath, FileMode>) -> Self {
        Self {
            state: Arc::new(State {
                revision: revision("revision-a"),
                later_revision: revision("revision-b"),
                status_calls: AtomicUsize::new(0),
                tree_calls: AtomicUsize::new(0),
                file_calls: AtomicUsize::new(0),
                tree_revisions: Mutex::new(Vec::new()),
                file_revisions: Mutex::new(Vec::new()),
                files: Mutex::new(files),
                tree_cancel: Mutex::new(None),
            }),
        }
    }

    fn counters(&self) -> (usize, usize, usize) {
        (
            self.state.status_calls.load(Ordering::SeqCst),
            self.state.tree_calls.load(Ordering::SeqCst),
            self.state.file_calls.load(Ordering::SeqCst),
        )
    }
}

fn boxed<'a, T: Send + 'a>(
    value: impl std::future::Future<Output = Result<T, RuntimeError>> + Send + 'a,
) -> PortFuture<'a, T> {
    Box::pin(value)
}

fn unsupported<T: Send + 'static>() -> PortFuture<'static, T> {
    boxed(async { Err(adapter()) })
}

impl MemFsPort for EvidenceMemFs {
    fn initialize(&self, _: &AgentId, _: &InitialMemoryBlocks) -> PortFuture<'_, RevisionId> {
        unsupported()
    }

    fn status(&self, _: &AgentId) -> PortFuture<'_, MemFsStatus> {
        let call = self.state.status_calls.fetch_add(1, Ordering::SeqCst);
        let revision = if call == 0 {
            self.state.revision.clone()
        } else {
            self.state.later_revision.clone()
        };
        boxed(async move {
            Ok(MemFsStatus {
                revision: Some(revision),
                dirty: false,
            })
        })
    }

    fn tree(
        &self,
        _: &AgentId,
        revision: Option<&RevisionId>,
        items: Sender<MemFsTreeEntry>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ()> {
        self.state.tree_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(value) = revision {
            self.state
                .tree_revisions
                .lock()
                .expect("tree revisions")
                .push(value.clone());
        }
        let paths: Vec<_> = self
            .state
            .files
            .lock()
            .expect("files")
            .keys()
            .cloned()
            .collect();
        let blocked = self.state.tree_cancel.lock().expect("tree cancel").clone();
        boxed(async move {
            for path in paths {
                tokio::select! {
                    () = cancellation.cancelled() => return Err(cancelled()),
                    result = items.send(MemFsTreeEntry { path }) => {
                        result.map_err(|_| adapter())?;
                    }
                }
                if let Some(token) = &blocked {
                    token.cancel();
                }
            }
            Ok(())
        })
    }

    fn read(&self, _: &AgentId, _: &RepositoryPath) -> PortFuture<'_, MemoryFileContent> {
        unsupported()
    }
    fn write(&self, _: &AgentId, _: &RepositoryPath, _: &MemoryFileContent) -> PortFuture<'_, ()> {
        unsupported()
    }
    fn delete(&self, _: &AgentId, _: &RepositoryPath) -> PortFuture<'_, ()> {
        unsupported()
    }
    fn rename(&self, _: &AgentId, _: &RepositoryPath, _: &RepositoryPath) -> PortFuture<'_, ()> {
        unsupported()
    }
    fn history(
        &self,
        _: &AgentId,
        _: Sender<MemFsHistoryEntry>,
        _: CancellationToken,
    ) -> PortFuture<'_, ()> {
        unsupported()
    }

    fn file_at_revision(
        &self,
        _: &AgentId,
        path: &RepositoryPath,
        revision: &RevisionId,
    ) -> PortFuture<'_, MemoryFileContent> {
        self.state.file_calls.fetch_add(1, Ordering::SeqCst);
        self.state
            .file_revisions
            .lock()
            .expect("file revisions")
            .push(revision.clone());
        let mode = self.state.files.lock().expect("files").get(path).cloned();
        boxed(async move {
            match mode {
                Some(FileMode::Value(value)) => Ok(value),
                Some(FileMode::Cancel(value, token)) => {
                    token.cancel();
                    Ok(value)
                }
                Some(FileMode::Error(mode)) => Err(error(mode)),
                None => Err(error(ErrorMode::NotFound)),
            }
        })
    }

    fn diff(
        &self,
        _: &AgentId,
        _: Option<&RevisionId>,
        _: Option<&RevisionId>,
        _: Sender<DiffChunk>,
        _: CancellationToken,
    ) -> PortFuture<'_, ()> {
        unsupported()
    }
    fn commit(&self, _: &AgentId, _: &CommitMessage) -> PortFuture<'_, RevisionId> {
        unsupported()
    }
    fn create_worktree(&self, _: &AgentId) -> PortFuture<'_, WorktreeId> {
        unsupported()
    }
    fn merge_worktree(
        &self,
        _: &AgentId,
        _: &WorktreeId,
        _: &CommitMessage,
    ) -> PortFuture<'_, RevisionId> {
        unsupported()
    }
}

fn revision(value: &str) -> RevisionId {
    RevisionId::new(value.to_owned()).expect("revision")
}
fn path(value: &str) -> RepositoryPath {
    RepositoryPath::new(value.into()).expect("path")
}
fn content(value: &str) -> MemoryFileContent {
    MemoryFileContent::new(value.as_bytes().to_vec()).expect("content")
}
fn valid(value: &str) -> MemoryFileContent {
    content(&format!("---\ndescription: evidence\n---\n{value}\n"))
}
fn cancelled() -> RuntimeError {
    RuntimeError::Cancelled {
        context: "evidence".into(),
    }
}
fn adapter() -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "evidence",
        context: "evidence".into(),
    }
}
fn error(mode: ErrorMode) -> RuntimeError {
    let context = "evidence".into();
    match mode {
        ErrorMode::Permission => RuntimeError::PermissionDenied { context },
        ErrorMode::Limit => RuntimeError::LimitExceeded { context },
        ErrorMode::Adapter => RuntimeError::AdapterFailure {
            code: "evidence",
            context,
        },
        ErrorMode::Conflict => RuntimeError::Conflict { context },
        ErrorMode::NotFound => RuntimeError::NotFound { context },
    }
}

#[tokio::test]
async fn exact_revision_is_selected_once_and_used_everywhere() {
    let files = BTreeMap::from([
        (path("system/a.md"), FileMode::Value(valid("a"))),
        (path("system/b.md"), FileMode::Value(valid("b"))),
    ]);
    let port = EvidenceMemFs::new(files);
    let record = PromptCompiler::new(&port)
        .compile(
            &inputs("raw", "2000-01-01T00:00:00Z"),
            CancellationToken::new(),
        )
        .await
        .expect("compile");
    assert_eq!(port.counters(), (1, 1, 2));
    assert_eq!(record.memfs_revision.as_deref(), Some("revision-a"));
    assert_eq!(
        *port.state.tree_revisions.lock().expect("tree"),
        vec![revision("revision-a")]
    );
    assert_eq!(
        *port.state.file_revisions.lock().expect("files"),
        vec![revision("revision-a"), revision("revision-a")]
    );
}

#[tokio::test]
async fn pre_cancel_compile_has_zero_memfs_or_render_calls() {
    let port = EvidenceMemFs::new(BTreeMap::new());
    let renders = AtomicUsize::new(0);
    let token = CancellationToken::new();
    token.cancel();
    let result = PromptCompiler::with_render_counter(&port, &renders)
        .compile(&inputs("raw", "2000-01-01T00:00:00Z"), token)
        .await;
    assert!(matches!(result, Err(RuntimeError::Cancelled { .. })));
    assert_eq!(port.counters(), (0, 0, 0));
    assert_eq!(renders.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_between_files_stops_before_second_read() {
    let token = CancellationToken::new();
    let files = BTreeMap::from([
        (
            path("system/a.md"),
            FileMode::Cancel(valid("a"), token.clone()),
        ),
        (path("system/b.md"), FileMode::Value(valid("b"))),
    ]);
    let port = EvidenceMemFs::new(files);
    let result = PromptCompiler::new(&port)
        .compile(&inputs("raw", "2000-01-01T00:00:00Z"), token)
        .await;
    assert!(matches!(result, Err(RuntimeError::Cancelled { .. })));
    assert_eq!(port.counters(), (1, 1, 1));
}

#[tokio::test]
async fn exact_file_errors_propagate() {
    for mode in [
        ErrorMode::Permission,
        ErrorMode::Limit,
        ErrorMode::Adapter,
        ErrorMode::Conflict,
    ] {
        let port = EvidenceMemFs::new(BTreeMap::from([(
            path("system/a.md"),
            FileMode::Error(mode),
        )]));
        let result = PromptCompiler::new(&port)
            .compile(
                &inputs("raw", "2000-01-01T00:00:00Z"),
                CancellationToken::new(),
            )
            .await;
        assert!(
            std::mem::discriminant(&result.expect_err("error"))
                == std::mem::discriminant(&error(mode))
        );
    }
}

#[tokio::test]
async fn not_found_and_invalid_markdown_skip_before_later_valid_file() {
    let files = BTreeMap::from([
        (path("system/a.md"), FileMode::Error(ErrorMode::NotFound)),
        (path("system/b.md"), FileMode::Value(content("invalid"))),
        (path("system/c.md"), FileMode::Value(valid("retained"))),
    ]);
    let port = EvidenceMemFs::new(files);
    let record = PromptCompiler::new(&port)
        .compile(
            &inputs("raw", "2000-01-01T00:00:00Z"),
            CancellationToken::new(),
        )
        .await
        .expect("compile");
    assert!(record.core_memory.contains("retained"));
    assert_eq!(port.counters(), (1, 1, 3));
}

#[tokio::test]
async fn path_count_and_byte_aggregates_use_production_core() {
    let files = BTreeMap::from([
        (path("a.md"), FileMode::Value(valid("a"))),
        (path("bb.md"), FileMode::Value(valid("b"))),
    ]);
    for (items, bytes, succeeds) in [(2, 11, true), (1, 11, false), (2, 10, false)] {
        let port = EvidenceMemFs::new(files.clone());
        let result = collect_paths_with_limits(
            &port,
            &super::test_support::agent(),
            &revision("revision-a"),
            CancellationToken::new(),
            items,
            bytes,
        )
        .await;
        assert_eq!(result.is_ok(), succeeds);
    }
}

#[test]
fn retained_source_budget_below_at_and_above() {
    assert_eq!(retain_with_limit(3, 1, 5).expect("below"), 4);
    assert_eq!(retain_with_limit(3, 2, 5).expect("at"), 5);
    assert!(matches!(
        retain_with_limit(3, 3, 5),
        Err(RuntimeError::LimitExceeded { .. })
    ));
}

#[tokio::test]
async fn cancellation_unblocks_bounded_tree_producer() {
    let token = CancellationToken::new();
    let files = BTreeMap::from([
        (path("system/a.md"), FileMode::Value(valid("a"))),
        (path("system/b.md"), FileMode::Value(valid("b"))),
        (path("system/c.md"), FileMode::Value(valid("c"))),
    ]);
    let port = EvidenceMemFs::new(files);
    *port.state.tree_cancel.lock().expect("cancel") = Some(token.clone());
    let result = PromptCompiler::new(&port)
        .compile(&inputs("raw", "2000-01-01T00:00:00Z"), token)
        .await;
    assert!(matches!(result, Err(RuntimeError::Cancelled { .. })));
    assert_eq!(port.state.file_calls.load(Ordering::SeqCst), 0);
}

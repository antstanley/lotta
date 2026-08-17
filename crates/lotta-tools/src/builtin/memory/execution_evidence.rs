use super::super::{MemoryAuthor, MemoryToolBundle};
use crate::registry::ToolRegistry;
use lotta_domain::{AgentId, BoundedJsonValue};
use lotta_runtime::RuntimeError;
use lotta_runtime::boundary::{
    CommitMessage, DiffChunk, InitialMemoryBlocks, MemoryFileContent, RepositoryPath, RevisionId,
    WorktreeId,
};
use lotta_runtime::ports::{
    MemFsCommitAuthor, MemFsHistoryEntry, MemFsMutation, MemFsPort, MemFsStatus,
    MemFsTransactionResult, MemFsTreeEntry, PortFuture, ToolOutcome,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct InstrumentedMemFs {
    reads: AtomicUsize,
    transacts: AtomicUsize,
}

fn unsupported<T>() -> PortFuture<'static, T> {
    Box::pin(async {
        Err(RuntimeError::AdapterFailure {
            code: "test_memfs_unsupported",
            context: "memory execution evidence".into(),
        })
    })
}

impl MemFsPort for InstrumentedMemFs {
    fn initialize(&self, _: &AgentId, _: &InitialMemoryBlocks) -> PortFuture<'_, RevisionId> {
        unsupported()
    }
    fn status(&self, _: &AgentId) -> PortFuture<'_, MemFsStatus> {
        unsupported()
    }
    fn tree(
        &self,
        _: &AgentId,
        _: Option<&RevisionId>,
        _: Sender<MemFsTreeEntry>,
        _: CancellationToken,
    ) -> PortFuture<'_, ()> {
        unsupported()
    }
    fn read(&self, _: &AgentId, _: &RepositoryPath) -> PortFuture<'_, MemoryFileContent> {
        self.reads.fetch_add(1, Ordering::SeqCst);
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
        _: &RepositoryPath,
        _: &RevisionId,
    ) -> PortFuture<'_, MemoryFileContent> {
        unsupported()
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
    fn transact(
        &self,
        _: &AgentId,
        _: &[MemFsMutation],
        _: &CommitMessage,
        _: &MemFsCommitAuthor,
    ) -> PortFuture<'_, MemFsTransactionResult> {
        self.transacts.fetch_add(1, Ordering::SeqCst);
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

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lotta-task39-memory-evidence-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("memfs/agent-evidence/memory/system"))
            .expect("memory root");
        Self(path)
    }
    fn memory(&self) -> PathBuf {
        self.0
            .join("memfs/agent-evidence/memory")
            .canonicalize()
            .expect("canonical memory")
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn invoke(bundle: &MemoryToolBundle, input: serde_json::Value) -> ToolOutcome {
    use crate::{AllowAllPermissions, AllowAllSandbox, PipelineError, PipelineRequest};
    use crate::{OutcomeSink, SecretResolver, TraceEvent, TraceSink};
    struct NoTrace;
    impl TraceSink for NoTrace {
        fn record(&self, _: TraceEvent) {}
    }
    struct Sink;
    impl OutcomeSink for Sink {
        fn record(&self, _: &str, _: &ToolOutcome) -> Result<(), PipelineError> {
            Ok(())
        }
    }
    struct Secrets;
    impl SecretResolver for Secrets {
        fn resolve(&self, _: &str) -> Result<Option<String>, PipelineError> {
            Ok(None)
        }
    }
    struct Overflow;
    impl crate::clamp::OverflowWriter for Overflow {
        fn write(&self, _: &str, _: &str) -> Result<String, crate::clamp::ClampError> {
            Err(crate::clamp::ClampError::OverflowWrite)
        }
    }
    let registration = bundle.registrations()[0].clone();
    let registry = ToolRegistry::new(bundle.registrations().to_vec()).expect("registry");
    let snapshot = registry
        .update(crate::toolset::ToolsetId::None, &[registration], None)
        .expect("snapshot");
    crate::execute(PipelineRequest {
        tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
            lotta_runtime::boundary::ProviderName::new("pipeline-call".to_owned()).unwrap(),
        ),
        registry: snapshot,
        model_name: "memory",
        input: BoundedJsonValue::new(input).expect("input"),
        cancellation: CancellationToken::new(),
        hook_runtime: &lotta_runtime::hooks::NoopHookRuntime,
        permissions: &AllowAllPermissions,
        sandbox: &AllowAllSandbox,
        secrets: &Secrets,
        trace: &NoTrace,
        overflow: &Overflow,
        persistence: &Sink,
        emit: &Sink,
    })
    .await
    .expect("pipeline")
}

#[tokio::test]
async fn traversal_absolute_and_symlink_escape_reach_zero_transacts() {
    let root = Root::new();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&root.0, root.memory().join("system/link.md")).expect("symlink");
    let port = Arc::new(InstrumentedMemFs::default());
    let agent = AgentId::accept("agent-evidence").expect("agent");
    let bundle = MemoryToolBundle::new(
        &root.memory(),
        agent,
        port.clone(),
        MemoryAuthor::new("Evidence".into(), "evidence@letta.com".into()).expect("author"),
    )
    .expect("bundle");
    for path in ["../peer.md", "/tmp/peer.md", "system/link.md"] {
        let outcome = invoke(
            &bundle,
            serde_json::json!({
                "command":"str_replace","reason":"reject","file_path":path,
                "old_string":"old","new_string":"new"
            }),
        )
        .await;
        assert!(matches!(outcome, ToolOutcome::ToolDefinedError { .. }));
    }
    assert_eq!(port.transacts.load(Ordering::SeqCst), 0);
    assert!(port.reads.load(Ordering::SeqCst) <= 1);
}

#[tokio::test]
async fn injected_transact_failure_is_reported_without_partial_effect() {
    let root = Root::new();
    let port = Arc::new(InstrumentedMemFs::default());
    let bundle = MemoryToolBundle::new(
        &root.memory(),
        AgentId::accept("agent-evidence").expect("agent"),
        port.clone(),
        MemoryAuthor::new("Evidence".into(), "evidence@letta.com".into()).expect("author"),
    )
    .expect("bundle");
    let outcome = invoke(
        &bundle,
        serde_json::json!({
            "command":"create","reason":"fail transaction","file_path":"system/new.md",
            "description":"New","file_text":"never committed"
        }),
    )
    .await;
    assert!(matches!(
        outcome,
        ToolOutcome::ToolDefinedError { ref code, .. } if code.as_str() == "infrastructure_error"
    ));
    assert_eq!(port.transacts.load(Ordering::SeqCst), 1);
    assert!(!root.memory().join("system/new.md").exists());
}

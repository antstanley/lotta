use super::{MemoryAuthor, MemoryToolBundle};
use crate::registry::ToolRegistry;
use lotta_domain::{AgentId, BoundedJsonValue};
use lotta_memfs::GitMemFs;
use lotta_runtime::boundary::InitialMemoryBlocks;
use lotta_runtime::ports::{MemFsPort, ToolOutcome};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lotta-task39-memory-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("root");
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn fixture() -> (Root, Arc<GitMemFs>, AgentId, MemoryToolBundle) {
    let root = Root::new();
    let agent = AgentId::accept("agent-task39").expect("agent");
    let port = Arc::new(GitMemFs::new(root.0.clone()).expect("port"));
    port.initialize(
        &agent,
        &InitialMemoryBlocks::new(Vec::new()).expect("blocks"),
    )
    .await
    .expect("initialize");
    let memory_root = root
        .0
        .join("memfs/agent-task39/memory")
        .canonicalize()
        .expect("memory root");
    let bundle = MemoryToolBundle::new(
        &memory_root,
        agent.clone(),
        port.clone(),
        MemoryAuthor::new("Agent Task39".into(), "agent-task39@letta.com".into()).expect("author"),
    )
    .expect("bundle");
    (root, port, agent, bundle)
}

#[tokio::test]
async fn edit_and_patch_each_create_one_clean_commit() {
    let (_root, port, agent, bundle) = fixture().await;
    invoke(
        &bundle,
        "memory",
        serde_json::json!({
            "command":"create","reason":"create profile","file_path":"system/profile.md",
            "description":"Profile","file_text":"old"
        }),
    )
    .await;
    assert!(!port.status(&agent).await.expect("status").dirty);
    invoke(&bundle, "memory_apply_patch", serde_json::json!({
        "reason":"update profile","input":"*** Begin Patch\n*** Update File: system/profile.md\n@@\n-old\n+new\n*** End Patch"
    })).await;
    assert!(!port.status(&agent).await.expect("status").dirty);
    let value = port
        .read(
            &agent,
            &lotta_runtime::boundary::RepositoryPath::new("system/profile.md".into())
                .expect("path"),
        )
        .await
        .expect("read");
    assert!(
        std::str::from_utf8(value.as_slice())
            .expect("utf8")
            .ends_with("new")
    );
    let history = history(port.as_ref(), &agent).await;
    assert_eq!(history.len(), 3);
    assert_eq!(history[0], "update profile");
    assert_eq!(history[1], "create profile");
}

#[tokio::test]
async fn no_op_creates_no_commit() {
    let (_root, port, agent, bundle) = fixture().await;
    invoke(
        &bundle,
        "memory",
        serde_json::json!({
            "command":"create","reason":"create","file_path":"system/x.md",
            "description":"X","file_text":"same"
        }),
    )
    .await;
    let before = port.status(&agent).await.expect("status").revision;
    invoke(
        &bundle,
        "memory",
        serde_json::json!({
            "command":"str_replace","reason":"noop","file_path":"system/x.md",
            "old_string":"same","new_string":"same"
        }),
    )
    .await;
    assert_eq!(before, port.status(&agent).await.expect("status").revision);
    assert!(!port.status(&agent).await.expect("status").dirty);
}

async fn invoke(bundle: &MemoryToolBundle, model: &str, input: serde_json::Value) {
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
    let registry = ToolRegistry::new(bundle.registrations().to_vec()).expect("registry");
    let registration = bundle
        .registrations()
        .iter()
        .find(|item| item.definition.internal_name.as_str() == model)
        .expect("registration")
        .clone();
    let snapshot = registry
        .update(crate::toolset::ToolsetId::None, &[registration], None)
        .expect("snapshot");
    let outcome = crate::execute(PipelineRequest {
        approval_grant: lotta_runtime::ports::ToolApprovalGrant::None,
        tool_call_id: lotta_runtime::ports::ToolCallId::from_name(
            lotta_runtime::boundary::ProviderName::new("pipeline-call".to_owned()).unwrap(),
        ),
        registry: snapshot,
        model_name: model,
        input: BoundedJsonValue::new(input).expect("input"),
        cancellation: tokio_util::sync::CancellationToken::new(),
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
    .expect("pipeline");
    assert!(matches!(outcome, ToolOutcome::Success { .. }));
}

async fn history(port: &dyn MemFsPort, agent: &AgentId) -> Vec<String> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
    let future = port.history(agent, sender, tokio_util::sync::CancellationToken::new());
    let drain = async {
        let mut values = Vec::new();
        while let Some(item) = receiver.recv().await {
            values.push(item.summary.as_str().to_owned());
        }
        values
    };
    let (result, values) = tokio::join!(future, drain);
    result.expect("history");
    values
}

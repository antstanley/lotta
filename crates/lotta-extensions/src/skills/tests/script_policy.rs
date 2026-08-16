use super::*;
use lotta_domain::{AgentId, BoundedJsonValue, ConversationId, RuntimeScope};
use lotta_runtime::boundary::{
    ConfinedPath, ProcessArguments, ProcessEnvironment, ProcessOutputBytesMax, Program,
};
use lotta_runtime::ports::*;
use lotta_tools::*;
use std::future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Permission(
    PermissionDecision,
    &'static str,
    Arc<Mutex<Vec<&'static str>>>,
);
impl PermissionGate for Permission {
    fn check(
        &self,
        _: PermissionInvocation<'_>,
    ) -> Result<PermissionDecision, lotta_tools::permissions::matcher::PermissionError> {
        self.2.lock().unwrap().push(self.1);
        Ok(self.0)
    }
}
struct Sandbox(SandboxDecision, Arc<Mutex<Vec<&'static str>>>);
impl SandboxGate for Sandbox {
    fn check(&self, _: SandboxInvocation<'_>) -> Result<SandboxDecision, SandboxError> {
        self.1.lock().unwrap().push("sandbox");
        Ok(self.0)
    }
}
struct Port {
    trace: Arc<Mutex<Vec<&'static str>>>,
    fail: bool,
}
impl SandboxPort for Port {
    fn execute(
        &self,
        _: ProcessRequest,
        _: mpsc::Sender<ProcessEvent>,
        _: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        self.trace.lock().unwrap().push("process");
        let result = if self.fail {
            Err(lotta_runtime::RuntimeError::AdapterFailure {
                code: "unsupported",
                context: "sandbox".into(),
            })
        } else {
            Ok(ProcessOutcome {
                exit_code: Some(0),
                timed_out: false,
            })
        };
        Box::pin(future::ready(result))
    }
}

fn definition() -> ToolDefinition {
    let schema = serde_json::from_value::<ToolInputSchema>(
        serde_json::json!({"type":"object","properties":{"file_path":{"type":"string"}}}),
    )
    .unwrap();
    let secrets = serde_json::from_value::<SecretRedactionSpec>(
        serde_json::json!({"fields":[],"policy":"redact"}),
    )
    .unwrap();
    ToolDefinition::new(
        InternalToolName::new("Read".into()).unwrap(),
        ModelFacingToolName::new("Read".into()).unwrap(),
        schema,
        ToolDescriptionAsset::new(String::new()).unwrap(),
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Never,
        PermissionAction::new("read".into()).unwrap(),
        ToolTimeout::new(Duration::from_secs(1)).unwrap(),
        ToolOutputLimit::new(1, 1).unwrap(),
        secrets,
    )
}
fn input(path: &str) -> ValidatedToolInput {
    ValidatedToolInput::new(BoundedJsonValue::new(serde_json::json!({"file_path":path})).unwrap())
        .unwrap()
}
fn request() -> ProcessRequest {
    ProcessRequest::new(
        RuntimeScope::new(
            AgentId::accept("agent").unwrap(),
            ConversationId::accept("conversation").unwrap(),
            None,
        ),
        Program::new("program".into()).unwrap(),
        ProcessArguments::new(vec![]).unwrap(),
        ConfinedPath::new("/sandbox".into(), "/sandbox".into()).unwrap(),
        ProcessEnvironment::new(vec![]).unwrap(),
        None,
        ProcessOutputBytesMax::new(1).unwrap(),
        Duration::from_secs(1),
    )
    .unwrap()
}
async fn run(
    permission: PermissionDecision,
    sandbox: SandboxDecision,
    fail: bool,
) -> (Result<ProcessOutcome, SkillScriptError>, Vec<&'static str>) {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let permission = Permission(permission, "permission", Arc::clone(&trace));
    let sandbox = Sandbox(sandbox, Arc::clone(&trace));
    let port = Port {
        trace: Arc::clone(&trace),
        fail,
    };
    let runner = SkillScriptRunner::new(&permission, &sandbox, &port);
    let (sender, _) = mpsc::channel(1);
    let result = runner
        .run(
            &definition(),
            &input("/sandbox/file"),
            request(),
            sender,
            CancellationToken::new(),
        )
        .await;
    let values = trace.lock().unwrap().clone();
    (result, values)
}
#[tokio::test]
async fn permission_denial_stops_before_process() {
    let (result, trace) = run(PermissionDecision::Deny, SandboxDecision::Allow, false).await;
    assert_eq!(result, Err(SkillScriptError::PermissionDenied));
    assert_eq!(trace, ["permission"]);
}
#[tokio::test]
async fn approval_required_stops_before_process() {
    let (result, trace) = run(PermissionDecision::Ask, SandboxDecision::Allow, false).await;
    assert_eq!(result, Err(SkillScriptError::ApprovalRequired));
    assert_eq!(trace, ["permission"]);
}
#[tokio::test]
async fn sandbox_denial_stops_before_process() {
    let (result, trace) = run(PermissionDecision::Allow, SandboxDecision::Deny, false).await;
    assert_eq!(result, Err(SkillScriptError::SandboxDenied));
    assert_eq!(trace, ["permission", "sandbox"]);
}
#[tokio::test]
async fn allowed_calls_sandbox_port_once_in_order() {
    let (result, trace) = run(PermissionDecision::Allow, SandboxDecision::Allow, false).await;
    assert!(result.is_ok());
    assert_eq!(trace, ["permission", "sandbox", "process"]);
}
#[tokio::test]
async fn adapter_unsupported_is_typed_runtime() {
    let (result, trace) = run(PermissionDecision::Allow, SandboxDecision::Allow, true).await;
    assert_eq!(result, Err(SkillScriptError::RuntimeAdapter));
    assert_eq!(trace, ["permission", "sandbox", "process"]);
}

static NEXT_REAL_GATE: AtomicUsize = AtomicUsize::new(0);

struct TempIsolation(PathBuf);
impl TempIsolation {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lotta-skill-script-{}-{}",
            std::process::id(),
            NEXT_REAL_GATE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("own")).expect("own");
        std::fs::create_dir_all(path.join("peer")).expect("peer");
        Self(path)
    }
}
impl Drop for TempIsolation {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct CountingPort(AtomicUsize);
impl SandboxPort for CountingPort {
    fn execute(
        &self,
        _: ProcessRequest,
        _: mpsc::Sender<ProcessEvent>,
        _: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(future::ready(Ok(ProcessOutcome {
            exit_code: Some(0),
            timed_out: false,
        })))
    }
}

#[tokio::test]
async fn real_workspace_gate_denies_peer_before_process() {
    let temp = TempIsolation::new();
    let own = temp.0.join("own");
    let peer_file = temp.0.join("peer/input.txt");
    let marker = own.join("marker");
    std::fs::write(&peer_file, b"validated peer input").expect("peer file");
    let sandbox = lotta_runtime::WorkspaceSandbox::new(own.clone(), temp.0.clone());
    let policy = WorkspacePolicy::new(&sandbox).expect("policy");
    let gate = WorkspaceSandboxGate::new(&policy, &own);
    let peer = input(peer_file.to_str().expect("utf8 peer"));
    assert_eq!(
        gate.check(SandboxInvocation {
            internal_name: "Read",
            input: &peer,
        })
        .expect("direct gate"),
        SandboxDecision::Deny
    );
    let trace = Arc::new(Mutex::new(Vec::new()));
    let permission = Permission(PermissionDecision::Allow, "permission", Arc::clone(&trace));
    let process = CountingPort(AtomicUsize::new(0));
    let runner = SkillScriptRunner::new(&permission, &gate, &process);
    let (sender, _) = mpsc::channel(1);
    let result = runner
        .run(
            &definition(),
            &peer,
            request(),
            sender,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(result, Err(SkillScriptError::SandboxDenied));
    assert_eq!(trace.lock().unwrap().as_slice(), ["permission"]);
    assert_eq!(process.0.load(Ordering::SeqCst), 0);
    assert!(!marker.exists());
}

struct CancelledPort {
    calls: AtomicUsize,
    observed_cancelled: AtomicUsize,
}
impl SandboxPort for CancelledPort {
    fn execute(
        &self,
        _: ProcessRequest,
        _: mpsc::Sender<ProcessEvent>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.observed_cancelled
            .store(usize::from(cancellation.is_cancelled()), Ordering::SeqCst);
        Box::pin(future::ready(Err(lotta_runtime::RuntimeError::Cancelled {
            context: "skill script cancellation".into(),
        })))
    }
}

#[tokio::test]
async fn cancellation_is_forwarded_to_process_once() {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let permission = Permission(PermissionDecision::Allow, "permission", Arc::clone(&trace));
    let sandbox = Sandbox(SandboxDecision::Allow, Arc::clone(&trace));
    let process = CancelledPort {
        calls: AtomicUsize::new(0),
        observed_cancelled: AtomicUsize::new(0),
    };
    let runner = SkillScriptRunner::new(&permission, &sandbox, &process);
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let (sender, _) = mpsc::channel(1);
    let result = runner
        .run(
            &definition(),
            &input("/sandbox/file"),
            request(),
            sender,
            cancellation,
        )
        .await;
    assert_eq!(result, Err(SkillScriptError::RuntimeAdapter));
    assert_eq!(process.calls.load(Ordering::SeqCst), 1);
    assert_eq!(process.observed_cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(trace.lock().unwrap().as_slice(), ["permission", "sandbox"]);
}

use lotta_domain::BoundedJsonValue;
use lotta_runtime::{
    bounds::{EXTERNAL_TOOL_CALL_TIMEOUT_MS, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX},
    hooks::HookRuntime,
    ports::*,
};
use lotta_tools::{
    AllowAllSandbox, ExecutorError, OutcomeSink, PermissionDecision, PermissionGate,
    PermissionInvocation, PipelineError, PipelineRequest, RawToolExecutionRequest, RawToolOutcome,
    SecretResolver, ToolExecutor, ToolRegistration, ToolRegistry, ToolsetId, TraceEvent, TraceSink,
    clamp::{ClampError, OverflowWriter},
    execute,
};
use serde_json::{Value, json};
use std::{
    future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(super) enum ExecutorAction {
    Success(String),
    RawFailure(ToolOutcome),
    ExecutorError,
}

#[derive(Default)]
struct State {
    trace: Vec<TraceEvent>,
    executor_calls: usize,
    executor_input: Option<Value>,
    persisted: Vec<ToolOutcome>,
    emitted: Vec<ToolOutcome>,
}

pub(super) struct PipelineCertificate {
    snapshot: Arc<lotta_tools::RegistrySnapshot>,
    state: Arc<Mutex<State>>,
    permissions: FixedPermissions,
    trace: RecordingTrace,
    persistence: RecordingSink,
    emit: RecordingSink,
    overflow: NoopOverflow,
    secrets: NoSecrets,
}

impl PipelineCertificate {
    pub(super) fn new(decision: PermissionDecision, action: ExecutorAction) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let registry = ToolRegistry::new([]).unwrap();
        registry
            .update(
                ToolsetId::None,
                &[ToolRegistration {
                    definition: definition(),
                    executor: Arc::new(RecordingExecutor {
                        state: Arc::clone(&state),
                        action,
                    }),
                }],
                None,
            )
            .unwrap();
        Self {
            snapshot: registry.snapshot().unwrap(),
            state: Arc::clone(&state),
            permissions: FixedPermissions(decision),
            trace: RecordingTrace(Arc::clone(&state)),
            persistence: RecordingSink(Arc::clone(&state), SinkKind::Persist),
            emit: RecordingSink(Arc::clone(&state), SinkKind::Emit),
            overflow: NoopOverflow,
            secrets: NoSecrets,
        }
    }

    pub(super) async fn run(
        &self,
        hook_runtime: Arc<dyn HookRuntime>,
        input: Value,
    ) -> Result<ToolOutcome, PipelineError> {
        execute(PipelineRequest {
            approval_grant: lotta_runtime::ports::ToolApprovalGrant::None,
            tool_call_id: ToolCallId::from_name(
                lotta_runtime::boundary::ProviderName::new("hook-certificate-call".into()).unwrap(),
            ),
            registry: Arc::clone(&self.snapshot),
            model_name: "Read",
            input: BoundedJsonValue::new(input).unwrap(),
            cancellation: CancellationToken::new(),
            hook_runtime: hook_runtime.as_ref(),
            permissions: &self.permissions,
            sandbox: &AllowAllSandbox,
            secrets: &self.secrets,
            trace: &self.trace,
            overflow: &self.overflow,
            persistence: &self.persistence,
            emit: &self.emit,
        })
        .await
    }

    pub(super) fn trace(&self) -> Vec<TraceEvent> {
        self.state.lock().unwrap().trace.clone()
    }

    pub(super) fn executor_calls(&self) -> usize {
        self.state.lock().unwrap().executor_calls
    }

    pub(super) fn executor_input(&self) -> Option<Value> {
        self.state.lock().unwrap().executor_input.clone()
    }

    pub(super) fn persisted(&self) -> Vec<ToolOutcome> {
        self.state.lock().unwrap().persisted.clone()
    }

    pub(super) fn emitted(&self) -> Vec<ToolOutcome> {
        self.state.lock().unwrap().emitted.clone()
    }
}

fn definition() -> Arc<ToolDefinition> {
    Arc::new(ToolDefinition::new(
        InternalToolName::new("Read".into()).unwrap(),
        ModelFacingToolName::new("Read".into()).unwrap(),
        serde_json::from_value(json!({
            "type": "object",
            "required": ["n"],
            "properties": {"n": {"type": "integer"}}
        }))
        .unwrap(),
        ToolDescriptionAsset::new(String::new()).unwrap(),
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Never,
        PermissionAction::new("execute".into()).unwrap(),
        ToolTimeout::new(Duration::from_millis(
            EXTERNAL_TOOL_CALL_TIMEOUT_MS.value as u64,
        ))
        .unwrap(),
        ToolOutputLimit::new(
            TOOL_RESULT_BYTES_MAX.value,
            TOOL_RESULT_MODEL_CHARS_MAX.value,
        )
        .unwrap(),
        serde_json::from_value(json!({"fields": [], "policy": "redact"})).unwrap(),
    ))
}

struct FixedPermissions(PermissionDecision);
impl PermissionGate for FixedPermissions {
    fn check(
        &self,
        _: PermissionInvocation<'_>,
    ) -> Result<PermissionDecision, lotta_tools::permissions::matcher::PermissionError> {
        Ok(self.0)
    }
}

struct RecordingExecutor {
    state: Arc<Mutex<State>>,
    action: ExecutorAction,
}
impl ToolExecutor for RecordingExecutor {
    fn execute(
        &self,
        request: RawToolExecutionRequest,
    ) -> lotta_tools::pipeline::ExecutorFuture<'_> {
        let mut state = self.state.lock().unwrap();
        state.executor_calls += 1;
        state.executor_input = Some(request.input.as_value().clone());
        let result = match &self.action {
            ExecutorAction::Success(output) => Ok(RawToolOutcome::Success(output.clone())),
            ExecutorAction::RawFailure(outcome) => Ok(RawToolOutcome::Failure(outcome.clone())),
            ExecutorAction::ExecutorError => Err(ExecutorError),
        };
        Box::pin(future::ready(result))
    }
}

struct RecordingTrace(Arc<Mutex<State>>);
impl TraceSink for RecordingTrace {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().trace.push(event);
    }
}

#[derive(Clone, Copy)]
enum SinkKind {
    Persist,
    Emit,
}
struct RecordingSink(Arc<Mutex<State>>, SinkKind);
impl OutcomeSink for RecordingSink {
    fn record(&self, _: &str, outcome: &ToolOutcome) -> Result<(), PipelineError> {
        let mut state = self.0.lock().unwrap();
        match self.1 {
            SinkKind::Persist => state.persisted.push(outcome.clone()),
            SinkKind::Emit => state.emitted.push(outcome.clone()),
        }
        Ok(())
    }
}

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn resolve(&self, _: &str) -> Result<Option<String>, PipelineError> {
        Ok(None)
    }
}

struct NoopOverflow;
impl OverflowWriter for NoopOverflow {
    fn write(&self, _: &str, _: &str) -> Result<String, ClampError> {
        Ok(String::new())
    }
}

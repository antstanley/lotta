use super::test_support::*;
use super::*;
use crate::ports::*;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

fn scripts(tool: &str) -> Vec<Vec<ProviderEvent>> {
    let mut first = call_events("call", tool);
    first.push(ProviderEvent::Stop {
        reason: StopReason::ToolUse,
    });
    vec![
        first,
        vec![ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]
}

struct CaptureTool {
    calls: AtomicUsize,
    inputs: Mutex<Vec<serde_json::Value>>,
}

impl ToolPort for CaptureTool {
    fn execute(&self, request: ToolExecutionRequest) -> PortFuture<'_, ToolOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs
            .lock()
            .unwrap()
            .push(request.input.as_value().clone());
        Box::pin(async { Ok(outcome("captured")) })
    }
}

struct CapturingApproval {
    resolution: ApprovalResolution,
    requests: Mutex<Vec<ControlRequest>>,
}

impl ApprovalPort for CapturingApproval {
    fn store_request(&self, request: ControlRequest) -> Result<(), crate::RuntimeError> {
        self.requests.lock().unwrap().push(request);
        Ok(())
    }
    fn await_resolution(
        &self,
        _: ControlRequest,
        _: tokio_util::sync::CancellationToken,
    ) -> PortFuture<'_, ApprovalResolution> {
        let resolution = self.resolution.clone();
        Box::pin(async move { Ok(resolution) })
    }
    fn mark_allowed(&self, _: &ControlRequest, _: &ToolOutcome) -> Result<(), crate::RuntimeError> {
        Ok(())
    }
    fn mark_interrupted(&self, _: &ControlRequest) -> Result<(), crate::RuntimeError> {
        Ok(())
    }
}

async fn run_approval(
    tool: &dyn ToolPort,
    approval: &dyn ApprovalPort,
    effects: &RecordingEffects,
) -> Result<TurnRunOutcome, crate::RuntimeError> {
    let provider = ScriptedProvider::new(scripts("approved"));
    let catalog = configured_catalog(configured_definition(
        "approved",
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Always,
    ));
    let (mut runtime, handle, lease) = runtime();
    let ports = TurnPorts::direct(&provider, tool, &catalog, effects).with_approvals(approval);
    run_turn(&mut runtime, handle, lease, request(), ports).await
}

#[tokio::test]
async fn approval_edit_replaces_executor_input() {
    let tool = CaptureTool {
        calls: AtomicUsize::new(0),
        inputs: Mutex::new(Vec::new()),
    };
    let approval = CapturingApproval {
        resolution: ApprovalResolution::Allow(Some(
            lotta_domain::BoundedJsonValue::new(serde_json::json!({"edited":true})).unwrap(),
        )),
        requests: Mutex::new(Vec::new()),
    };
    let effects = RecordingEffects::default();
    assert_eq!(
        run_approval(&tool, &approval, &effects).await.unwrap(),
        TurnRunOutcome::Completed
    );
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *tool.inputs.lock().unwrap(),
        [serde_json::json!({"edited":true})]
    );
}

#[tokio::test]
async fn invalid_approval_edit_never_reaches_executor() {
    let tool = CaptureTool {
        calls: AtomicUsize::new(0),
        inputs: Mutex::new(Vec::new()),
    };
    let oversized = "x".repeat(crate::bounds::TOOL_INPUT_BYTES_MAX.value + 1);
    let approval = CapturingApproval {
        resolution: ApprovalResolution::Allow(Some(
            lotta_domain::BoundedJsonValue::new(serde_json::json!({"value":oversized})).unwrap(),
        )),
        requests: Mutex::new(Vec::new()),
    };
    let effects = RecordingEffects::default();
    assert!(run_approval(&tool, &approval, &effects).await.is_err());
    assert_eq!(tool.calls.load(Ordering::SeqCst), 0);
}

fn configured_definition(
    name: &str,
    owner: ToolExecutionOwner,
    approval: ToolApprovalPolicy,
) -> ToolDefinition {
    let mut value = definition(name);
    value.execution_owner = owner;
    value.approval_policy = approval;
    value
}

fn configured_catalog(definition: ToolDefinition) -> TurnToolCatalog {
    TurnToolCatalog::new(vec![definition]).unwrap()
}

struct External {
    calls: AtomicUsize,
}
impl ControllerToolPort for External {
    fn execute_external(
        &self,
        _: super::ControllerToolRequestRecord,
        request: ToolExecutionRequest,
    ) -> PortFuture<'_, ToolOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            assert_eq!(
                request.definition.execution_owner,
                ToolExecutionOwner::ModSidecar
            );
            Ok(outcome("external"))
        })
    }
}

struct Approval {
    resolution: Mutex<Option<Result<ApprovalResolution, crate::RuntimeError>>>,
    requests: Mutex<Vec<ControlRequest>>,
    calls: AtomicUsize,
}
impl ApprovalPort for Approval {
    fn store_request(&self, request: ControlRequest) -> Result<(), crate::RuntimeError> {
        assert_eq!(request.call_id, id("call"));
        self.requests.lock().unwrap().push(request);
        Ok(())
    }
    fn await_resolution(
        &self,
        _: ControlRequest,
        _: tokio_util::sync::CancellationToken,
    ) -> PortFuture<'_, ApprovalResolution> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let resolution = self.resolution.lock().unwrap().take().unwrap();
        Box::pin(async move { resolution })
    }
    fn mark_allowed(&self, _: &ControlRequest, _: &ToolOutcome) -> Result<(), crate::RuntimeError> {
        Ok(())
    }
    fn mark_interrupted(&self, _: &ControlRequest) -> Result<(), crate::RuntimeError> {
        Ok(())
    }
}

#[tokio::test]
async fn aborted_approval_uses_user_cancellation_terminal() {
    let tool = CaptureTool {
        calls: AtomicUsize::new(0),
        inputs: Mutex::new(Vec::new()),
    };
    let approval = Approval {
        resolution: Mutex::new(Some(Err(crate::RuntimeError::Cancelled {
            context: "approval abort".into(),
        }))),
        requests: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
    };
    let effects = RecordingEffects::default();
    assert!(matches!(
        run_approval(&tool, &approval, &effects).await.unwrap(),
        TurnRunOutcome::Cancelled(_)
    ));
    assert_eq!(tool.calls.load(Ordering::SeqCst), 0);
    let stops = effects.stops.lock().unwrap();
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].reason, TurnStopReason::UserCancellation);
    assert_eq!(
        effects.events.lock().unwrap().last(),
        Some(&TurnEvent::Cancelled)
    );
}

#[tokio::test]
async fn local_runs_once_persists_in_order_and_resumes_provider() {
    let provider = ScriptedProvider::new(scripts("local"));
    let tools = SequencingTool::new(vec![outcome("local")]);
    let effects = RecordingEffects::default();
    let catalog = configured_catalog(configured_definition(
        "local",
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Never,
    ));
    let (mut runtime, handle, lease) = runtime();
    run_turn(
        &mut runtime,
        handle,
        lease,
        request(),
        TurnPorts::direct(&provider, &tools, &catalog, &effects),
    )
    .await
    .unwrap();
    assert_eq!(tools.calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        *effects.order.lock().unwrap(),
        vec!["result", "event", "finished"]
    );
}

#[tokio::test]
async fn external_uses_non_rust_owner_once_and_resumes_provider() {
    let provider = ScriptedProvider::new(scripts("external"));
    let tools = SequencingTool::new(Vec::new());
    let external = External {
        calls: AtomicUsize::new(0),
    };
    let effects = RecordingEffects::default();
    let catalog = configured_catalog(configured_definition(
        "external",
        ToolExecutionOwner::ModSidecar,
        ToolApprovalPolicy::Never,
    ));
    let (mut runtime, handle, lease) = runtime();
    let ports =
        TurnPorts::direct(&provider, &tools, &catalog, &effects).with_controller_tools(&external);
    run_turn(&mut runtime, handle, lease, request(), ports)
        .await
        .unwrap();
    assert_eq!(external.calls.load(Ordering::SeqCst), 1);
    assert_eq!(tools.calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn approval_pending_persists_before_emit_allows_once_and_resumes() {
    let provider = ScriptedProvider::new(scripts("approved"));
    let tools = SequencingTool::new(vec![outcome("approved")]);
    let approval = Approval {
        resolution: Mutex::new(Some(Ok(ApprovalResolution::Allow(None)))),
        requests: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
    };
    let effects = RecordingEffects::default();
    let catalog = configured_catalog(configured_definition(
        "approved",
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Always,
    ));
    let (mut runtime, handle, lease) = runtime();
    let ports = TurnPorts::direct(&provider, &tools, &catalog, &effects).with_approvals(&approval);
    run_turn(&mut runtime, handle, lease, request(), ports)
        .await
        .unwrap();
    assert_eq!(approval.calls.load(Ordering::SeqCst), 1);
    let requests = approval.requests.lock().unwrap();
    let control = &requests[0];
    assert_eq!(control.request_id.as_str(), "approval-1-call");
    assert_eq!(control.call_id, id("call"));
    assert_eq!(control.lease_generation, 1);
    assert_eq!(control.tool_name.as_str(), "approved");
    assert_eq!(control.input.as_value(), &serde_json::json!({}));
    assert_eq!(
        control.schema.as_value(),
        &serde_json::json!({"type":"object"})
    );
    drop(requests);
    assert_eq!(tools.calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(&effects.order.lock().unwrap()[..2], &["control", "event"]);
}

#[tokio::test]
async fn denied_approval_appends_user_denied_without_executor_and_resumes() {
    let provider = ScriptedProvider::new(scripts("denied"));
    let tools = SequencingTool::new(Vec::new());
    let approval = Approval {
        resolution: Mutex::new(Some(Ok(ApprovalResolution::Deny))),
        requests: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
    };
    let effects = RecordingEffects::default();
    let catalog = configured_catalog(configured_definition(
        "denied",
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Always,
    ));
    let (mut runtime, handle, lease) = runtime();
    let ports = TurnPorts::direct(&provider, &tools, &catalog, &effects).with_approvals(&approval);
    run_turn(&mut runtime, handle, lease, request(), ports)
        .await
        .unwrap();
    assert_eq!(tools.calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        effects.results.lock().unwrap()[0].outcome,
        ToolOutcome::UserDenied { .. }
    ));
}

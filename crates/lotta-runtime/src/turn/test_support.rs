use super::{
    ToolResultRecord, TurnEffectPort, TurnEvent, TurnProjection, TurnStopRecord, TurnToolCatalog,
};
use crate::boundary::{ProviderEventText, ProviderName, ToolArgumentChunk};
use crate::ports::*;
use crate::{ListenerRuntime, RuntimeError, RuntimeHandle, turn::ControlRequest};
use lotta_domain::{
    AgentId, BoundedJsonValue, BoundedVec, ConversationId, ModelDescriptor, NonEmptyString, RunId,
    RuntimeScope, TurnLease,
};
use serde_json::json;
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) enum ProviderScript {
    Events(Vec<ProviderEvent>),
    Error(RuntimeError),
    CancelBefore(Vec<ProviderEvent>, CancellationToken),
    CancelAfterFirst(Vec<ProviderEvent>, CancellationToken),
}

pub(crate) struct ScriptedProvider {
    scripts: Mutex<VecDeque<ProviderScript>>,
    pub(crate) requests: Mutex<Vec<ProviderRequest>>,
    pub(super) calls: AtomicUsize,
}

impl ScriptedProvider {
    pub(crate) fn new(scripts: Vec<Vec<ProviderEvent>>) -> Self {
        Self::configured(scripts.into_iter().map(ProviderScript::Events).collect())
    }

    pub(super) fn configured(scripts: Vec<ProviderScript>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
        }
    }
}

impl ProviderPort for ScriptedProvider {
    fn stream(&self, request: ProviderRequest, events: ProviderEventSink) -> PortFuture<'_, ()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request);
        let script = self.scripts.lock().unwrap().pop_front();
        Box::pin(async move {
            match script {
                Some(ProviderScript::Events(script)) => send_script(script, events).await,
                Some(ProviderScript::CancelBefore(script, token)) => {
                    token.cancel();
                    send_script(script, events).await
                }
                Some(ProviderScript::CancelAfterFirst(script, token)) => {
                    let mut script = script.into_iter();
                    if let Some(event) = script.next() {
                        events.send(event).await?;
                    }
                    token.cancel();
                    for event in script {
                        events.send(event).await?;
                    }
                    Ok(())
                }
                Some(ProviderScript::Error(error)) => Err(error),
                None => Err(RuntimeError::InvalidData {
                    context: "missing provider script".into(),
                }),
            }
        })
    }
}

async fn send_script(
    script: Vec<ProviderEvent>,
    events: ProviderEventSink,
) -> Result<(), RuntimeError> {
    for event in script {
        events.send(event).await?;
    }
    Ok(())
}

pub(crate) struct SequencingTool {
    outcomes: Mutex<VecDeque<Result<ToolOutcome, RuntimeError>>>,
    pub(super) log: Mutex<Vec<String>>,
    pub(super) calls: AtomicUsize,
    in_flight: AtomicUsize,
    pub(super) max_in_flight: AtomicUsize,
    cancel_during_execute: Option<CancellationToken>,
}

impl SequencingTool {
    pub(crate) fn new(outcomes: Vec<ToolOutcome>) -> Self {
        Self::configured(outcomes.into_iter().map(Ok).collect(), None)
    }

    pub(super) fn configured(
        outcomes: Vec<Result<ToolOutcome, RuntimeError>>,
        cancel_during_execute: Option<CancellationToken>,
    ) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
            log: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            cancel_during_execute,
        }
    }
}

impl ToolPort for SequencingTool {
    fn execute(&self, request: ToolExecutionRequest) -> PortFuture<'_, ToolOutcome> {
        let name = request.definition.model_name.as_str().to_owned();
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(current, Ordering::SeqCst);
            self.log.lock().unwrap().push(format!("E{name}"));
            tokio::task::yield_now().await;
            if let Some(token) = &self.cancel_during_execute {
                token.cancel();
            }
            self.log.lock().unwrap().push(format!("X{name}"));
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(RuntimeError::InvalidData {
                    context: "missing tool outcome".into(),
                })?
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct RecordingEffects {
    pub(super) projections: Arc<Mutex<Vec<TurnProjection>>>,
    pub(super) events: Arc<Mutex<Vec<TurnEvent>>>,
    pub(super) results: Arc<Mutex<Vec<ToolResultRecord>>>,
    pub(super) stops: Arc<Mutex<Vec<TurnStopRecord>>>,
    pub(super) order: Arc<Mutex<Vec<&'static str>>>,
    cancel_on_finished: Option<CancellationToken>,
    fail_finished: bool,
}

impl RecordingEffects {
    pub(super) fn configured(
        cancel_on_finished: Option<CancellationToken>,
        fail_finished: bool,
    ) -> Self {
        Self {
            cancel_on_finished,
            fail_finished,
            ..Self::default()
        }
    }
}

impl TurnEffectPort for RecordingEffects {
    fn persist_projection(&self, projection: TurnProjection) -> Result<(), RuntimeError> {
        self.projections.lock().unwrap().push(projection);
        self.order.lock().unwrap().push("projection");
        Ok(())
    }

    fn persist_stop_reason(&self, record: TurnStopRecord) -> Result<(), RuntimeError> {
        self.stops.lock().unwrap().push(record);
        self.order.lock().unwrap().push("stop");
        Ok(())
    }

    fn emit(&self, event: TurnEvent) -> Result<(), RuntimeError> {
        if matches!(event, TurnEvent::Finished { .. } | TurnEvent::Cancelled) {
            if let Some(token) = &self.cancel_on_finished {
                token.cancel();
            }
            if self.fail_finished {
                return Err(RuntimeError::AdapterFailure {
                    code: "finished_effect",
                    context: "turn finished effect".into(),
                });
            }
            self.order.lock().unwrap().push("finished");
        } else {
            self.order.lock().unwrap().push("event");
        }
        self.events.lock().unwrap().push(event);
        Ok(())
    }

    fn append_tool_result(&self, result: ToolResultRecord) -> Result<(), RuntimeError> {
        self.results.lock().unwrap().push(result);
        self.order.lock().unwrap().push("result");
        Ok(())
    }

    fn persist_controller_request(
        &self,
        _: super::ControllerToolRequestRecord,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn persist_compaction_request(&self, _: &super::CompactionRequest) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn persist_control_request(&self, _: &ControlRequest) -> Result<(), RuntimeError> {
        self.order.lock().unwrap().push("control");
        Ok(())
    }
}

pub(crate) fn text(value: &str) -> ProviderEventText {
    ProviderEventText::new(value.to_owned()).unwrap()
}

pub(super) fn id(value: &str) -> ToolCallId {
    ToolCallId::from_name(ProviderName::new(value.to_owned()).unwrap())
}

pub(super) fn chunk(value: &[u8]) -> ToolArgumentChunk {
    ToolArgumentChunk::new(value.to_vec()).unwrap()
}

pub(crate) fn request() -> ProviderRequest {
    ProviderRequest {
        model: ModelDescriptor {
            handle: NonEmptyString::new("fake").unwrap(),
            provider_id: NonEmptyString::new("fake-provider").unwrap(),
            available: true,
            context_window: Some(4096),
            model_settings: None,
        },
        system_prompt: None,
        messages: BoundedVec::new(Vec::new()).unwrap(),
        tools: BoundedVec::new(Vec::new()).unwrap(),
        tool_choice: ProviderToolChoice::Auto,
        image_policy: ImagePolicy::Strict,
        context_tokens_max: TokenLimit::new(1024).unwrap(),
        output_tokens_max: TokenLimit::new(1024).unwrap(),
        reasoning: ReasoningControls {
            enabled: true,
            effort: None,
            tier: None,
        },
        cancellation: CancellationToken::new(),
        context: None,
        deadline: ProviderDeadline::new(Duration::from_secs(10)).unwrap(),
    }
}

pub(super) fn runtime() -> (ListenerRuntime, RuntimeHandle, TurnLease) {
    let mut runtime = ListenerRuntime::new();
    let scope = RuntimeScope::new(
        AgentId::accept("agent").unwrap(),
        ConversationId::generate(1).unwrap(),
        None,
    );
    let handle = runtime
        .get_or_create(&scope, uuid::Uuid::from_u128(1))
        .unwrap();
    let lease = runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .begin_turn("turn".into(), RunId::generate_sequence(1).unwrap())
        .unwrap();
    (runtime, handle, lease)
}

pub(crate) fn outcome(value: &str) -> ToolOutcome {
    ToolOutcome::Success {
        content: ToolResultText::new(value.into(), ToolOutputLimit::new(1024, 1024).unwrap())
            .unwrap(),
    }
}

pub(super) fn definition(name: &str) -> ToolDefinition {
    ToolDefinition::new(
        InternalToolName::new(name.into()).unwrap(),
        ModelFacingToolName::new(name.into()).unwrap(),
        ToolInputSchema::new(BoundedJsonValue::new(json!({"type":"object"})).unwrap()).unwrap(),
        ToolDescriptionAsset::new(String::new()).unwrap(),
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Never,
        PermissionAction::new("execute".into()).unwrap(),
        ToolTimeout::new(Duration::from_secs(1)).unwrap(),
        ToolOutputLimit::new(1024, 1024).unwrap(),
        SecretRedactionSpec::new(
            BoundedVec::new(Vec::new()).unwrap(),
            SecretRedactionPolicy::Redact,
        )
        .unwrap(),
    )
}

pub(crate) fn catalog(names: &[&str]) -> TurnToolCatalog {
    TurnToolCatalog::new(names.iter().map(|name| definition(name)).collect()).unwrap()
}

pub(crate) fn call_events(call: &str, tool: &str) -> Vec<ProviderEvent> {
    let call_id = id(call);
    vec![
        ProviderEvent::ToolCallStart {
            call_id: call_id.clone(),
            name: text(tool),
        },
        ProviderEvent::ToolCallArgumentsDelta {
            call_id: call_id.clone(),
            chunk: chunk(b"{}"),
        },
        ProviderEvent::ToolCallEnd { call_id },
    ]
}

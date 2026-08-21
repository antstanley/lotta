//! Shared fixtures for the external-tool command group selectors.

use std::sync::{Arc, Mutex};

use lotta_domain::{AgentId, BoundedJsonValue, ConversationId, RuntimeScope};
use lotta_runtime::ports::{ToolApprovalGrant, ToolCallId, ToolOutcome};
use lotta_tools::clamp::{ClampError, OverflowWriter};
use lotta_tools::{
    AllowAllPermissions, AllowAllSandbox, OutcomeSink, PipelineError, RegistrySnapshot,
    SecretResolver, TraceEvent, TraceSink,
};
use serde_json::{Value, json};
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;

use super::{ExternalForwarder, ExternalToolBridge};
use crate::ws::ConnectionId;

const AGENT: &str = "agent-1";
const CONVERSATION: &str = "conversation-1";

pub(super) fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept(AGENT.to_owned()).expect("agent"),
        ConversationId::accept(CONVERSATION.to_owned()).expect("conversation"),
        None,
    )
}

pub(super) fn recorder() -> (ExternalToolBridge, RecordedFrames) {
    let frames: RecordedFrames = Arc::default();
    let sink = Arc::clone(&frames);
    let forward: ExternalForwarder = Arc::new(move |connection, message| {
        let frame = serde_json::to_value(&message).expect("message JSON");
        sink.lock().expect("frames").push((connection, frame));
        Ok(())
    });
    (ExternalToolBridge::new(forward), frames)
}

/// Recorded forwarded frames paired with their target connection.
pub(super) type RecordedFrames = Arc<Mutex<Vec<(ConnectionId, Value)>>>;

pub(super) fn wire_scope() -> Value {
    json!({"agent_id": AGENT, "conversation_id": CONVERSATION})
}

pub(super) fn definition(name: &str) -> Value {
    json!({
        "name": name,
        "description": "controller tool",
        "parameters": {
            "type": "object",
            "properties": {"tool_call_id": {"type": "string"}},
            "required": ["tool_call_id"],
        },
    })
}

pub(super) fn update_value(tools: &[Value]) -> Value {
    json!({
        "type": "runtime_external_tools_update",
        "request_id": "update-1",
        "updates": [{
            "runtimes": [wire_scope()],
            "external_tools": [{"tools": tools}],
        }],
    })
}

pub(super) async fn wait_for_frames(frames: &Mutex<Vec<(ConnectionId, Value)>>, count: usize) {
    timeout(Duration::from_secs(5), async {
        while frames.lock().expect("frames").len() < count {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("forwarded frames");
}

fn decoded(value: &Value) -> super::ExternalToolsCommand {
    let frame = crate::framing::decode_text(&value.to_string()).expect("bounded frame");
    super::decode(&frame)
        .expect("decode")
        .expect("external command")
}

pub(super) fn update_command(value: &Value) -> super::ToolsUpdateCommand {
    match decoded(value) {
        super::ExternalToolsCommand::ToolsUpdate(command) => *command,
        super::ExternalToolsCommand::CallResponse(_) => panic!("update command expected"),
    }
}

pub(super) fn response_command(value: &Value) -> super::ToolCallResponseCommand {
    match decoded(value) {
        super::ExternalToolsCommand::CallResponse(command) => *command,
        super::ExternalToolsCommand::ToolsUpdate(_) => panic!("call response expected"),
    }
}

struct RecordingTrace(Mutex<Vec<TraceEvent>>);
impl TraceSink for RecordingTrace {
    fn record(&self, event: TraceEvent) {
        self.0.lock().expect("trace").push(event);
    }
}
struct Sink;
impl OutcomeSink for Sink {
    fn record(&self, _: &str, _: &ToolOutcome) -> Result<(), PipelineError> {
        Ok(())
    }
}
struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn resolve(&self, _: &str) -> Result<Option<String>, PipelineError> {
        Ok(None)
    }
}
struct NoOverflow;
impl OverflowWriter for NoOverflow {
    fn write(&self, _: &str, _: &str) -> Result<String, ClampError> {
        Ok("unused".to_owned())
    }
}

pub(super) async fn run_pipeline(
    snapshot: Arc<RegistrySnapshot>,
    model: &'static str,
) -> ToolOutcome {
    let trace = RecordingTrace(Mutex::new(Vec::new()));
    let sink = Sink;
    let secrets = NoSecrets;
    let overflow = NoOverflow;
    let permissions = AllowAllPermissions;
    let sandbox = AllowAllSandbox;
    let provider =
        lotta_runtime::boundary::ProviderName::new("wire-call".to_owned()).expect("provider");
    lotta_tools::execute(lotta_tools::PipelineRequest {
        approval_grant: ToolApprovalGrant::None,
        tool_call_id: ToolCallId::from_name(provider),
        registry: snapshot,
        model_name: model,
        input: BoundedJsonValue::new(json!({"tool_call_id": "call-wire"})).expect("input"),
        cancellation: CancellationToken::new(),
        hook_runtime: &lotta_runtime::hooks::NoopHookRuntime,
        permissions: &permissions,
        sandbox: &sandbox,
        secrets: &secrets,
        trace: &trace,
        overflow: &overflow,
        persistence: &sink,
        emit: &sink,
    })
    .await
    .expect("pipeline")
}

pub(super) fn response_value(request_id: &str, extra: &Value) -> Value {
    let mut value = json!({
        "type": "external_tool_call_response",
        "request_id": request_id,
        "result": "ok",
    });
    if let (Some(target), Some(object)) = (value.as_object_mut(), extra.as_object()) {
        for (key, item) in object {
            target.insert(key.clone(), item.clone());
        }
    }
    value
}

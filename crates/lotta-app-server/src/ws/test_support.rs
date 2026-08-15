use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use lotta_domain::{
    AgentId, BoundedJsonValue, BoundedVec, Clock, ConversationId, DomainError, InputDisposition,
    NonEmptyString, RuntimeScope, Timestamp,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::service::{
    AbortOutcome, DeviceStateOutcome, InputAdmission, RuntimeEventBatch, RuntimeStartOutcome,
    ServiceFuture, SyncOutcome,
};
use super::*;

pub(super) fn text(value: &str) -> NonEmptyString {
    NonEmptyString::new(value).unwrap_or_else(|error| panic!("test text: {error}"))
}
pub(super) fn bounded(value: Value) -> BoundedJsonValue {
    BoundedJsonValue::new(value).unwrap_or_else(|error| panic!("test JSON: {error}"))
}
pub(super) fn scope(index: usize) -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept(format!("agent-{index}")).unwrap_or_else(|error| panic!("agent: {error}")),
        ConversationId::accept(format!("conversation-{index}"))
            .unwrap_or_else(|error| panic!("conversation: {error}")),
        None,
    )
}
pub(super) fn events(values: Vec<RuntimeEvent>) -> RuntimeEventBatch {
    BoundedVec::new(values).unwrap_or_else(|error| panic!("events: {error}"))
}
pub(super) fn status_event(name: &str) -> RuntimeEvent {
    RuntimeEvent::UpdateLoopStatus {
        loop_status: bounded(json!({"status": name})),
    }
}

pub(super) struct TestClock {
    pub(super) calls: AtomicUsize,
}
impl TestClock {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}
impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56Z")
            .unwrap_or_else(|error| panic!("timestamp: {error}"))
    }
    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}
pub(super) struct TestIds {
    pub(super) calls: AtomicUsize,
}
impl TestIds {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}
impl EventIdGenerator for TestIds {
    fn generate(&self) -> Result<Uuid, crate::error::AppServerError> {
        let value = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Uuid::parse_str(&format!("00000000-0000-4000-8000-{value:012x}"))
            .map_err(|_| crate::error::AppServerError::Internal)
    }
}

#[derive(Default)]
pub(super) struct RecordingService {
    pub(super) calls: AtomicUsize,
    pub(super) writes: AtomicUsize,
    pub(super) stages: Mutex<Vec<String>>,
}
impl RecordingService {
    pub(super) fn stage(&self, value: &str) {
        if let Ok(mut stages) = self.stages.lock() {
            stages.push(value.into());
        }
    }
}
impl RuntimeCommandService for RecordingService {
    fn runtime_start(
        &self,
        _: command::RuntimeStartCommand,
    ) -> ServiceFuture<'_, RuntimeStartOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.writes.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(RuntimeStartOutcome {
                runtime: scope(1),
                created_agent: false,
                created_conversation: false,
                agent: Some(bounded(json!({"id":"agent-1"}))),
                conversation: Some(bounded(json!({"id":"conversation-1"}))),
                broadcasts: events(vec![status_event("WAITING_ON_INPUT")]),
            })
        })
    }
    fn admit_input(&self, _: command::InputCommand) -> ServiceFuture<'_, InputAdmission> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.stage("admit");
        Box::pin(async {
            Ok(InputAdmission {
                disposition: InputDisposition::Started,
                error: None,
                continuation: Some(bounded(json!({"continue":true}))),
                after_ack: events(vec![RuntimeEvent::UpdateQueue {
                    queue: bounded(json!([])),
                    removed: bounded(json!([])),
                }]),
            })
        })
    }
    fn continue_input(
        &self,
        scope: RuntimeScope,
        _: Option<BoundedJsonValue>,
        sink: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()> {
        self.stage("continue");
        Box::pin(async move {
            sink.emit(
                &scope,
                RuntimeEvent::StreamDelta {
                    delta: bounded(json!({"message_type":"status","message":"done"})),
                    subagent_id: None,
                },
            )
        })
    }
    fn sync(&self, _: command::SyncCommand) -> ServiceFuture<'_, SyncOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(SyncOutcome {
                broadcasts: events(vec![status_event("SYNC")]),
            })
        })
    }
    fn abort_message(&self, _: command::AbortMessageCommand) -> ServiceFuture<'_, AbortOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(AbortOutcome { aborted: true }) })
    }
    fn change_device_state(
        &self,
        _: command::ChangeDeviceStateCommand,
    ) -> ServiceFuture<'_, DeviceStateOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(DeviceStateOutcome {
                broadcasts: events(vec![RuntimeEvent::UpdateDeviceStatus {
                    device_status: bounded(json!({"mode":"strict"})),
                }]),
            })
        })
    }
}

pub(super) fn router() -> (
    Arc<Mutex<RuntimeRouter>>,
    Arc<TestClock>,
    Arc<TestIds>,
    ConnectionId,
) {
    let clock = Arc::new(TestClock::new());
    let ids = Arc::new(TestIds::new());
    let mut router = RuntimeRouter::new(clock.clone(), ids.clone());
    let id = router
        .connections
        .open()
        .unwrap_or_else(|error| panic!("open: {error}"));
    router
        .connections
        .initialize(id)
        .unwrap_or_else(|error| panic!("init: {error}"));
    (Arc::new(Mutex::new(router)), clock, ids, id)
}
pub(super) fn decode_wire(value: &Value) -> RuntimeCommand {
    let frame = crate::framing::decode_text(&value.to_string())
        .unwrap_or_else(|error| panic!("frame: {error:?}"));
    command::decode(&frame)
        .unwrap_or_else(|error| panic!("decode: {error:?}"))
        .unwrap_or_else(|| panic!("runtime command"))
}
pub(super) fn response_json(output: &RouteOutput) -> Value {
    serde_json::to_value(&output.responses.as_slice()[0])
        .unwrap_or_else(|error| panic!("response: {error}"))
}

impl From<command::SyncCommand> for RuntimeCommand {
    fn from(value: command::SyncCommand) -> Self {
        Self::Sync(value)
    }
}

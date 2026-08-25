//! `ws::outbound::group_coverage` — one case per §Outbound message groups row.
//!
//! Each case derives its row membership from the pinned fixture's message list
//! via [`members_in_fixture`], then emits a real message through real server
//! machinery (router broadcasts, the Runtime command chain, and the
//! introspection bridge) and asserts the emitted discriminant belongs to its
//! row. Six rows, six cases: Control, Admission, State, Stream, Terminal,
//! Management.

use super::{
    ADMISSION_ROW, CONTROL_ROW, MANAGEMENT_ROW, STATE_ROW, STREAM_ROW, TERMINAL_ROW,
    members_in_fixture,
};
use crate::{
    error::AppServerError,
    ws::{
        command::{
            AbortMessageCommand, ChangeDeviceStateCommand, InputCommand, RuntimeCommand,
            RuntimeStartCommand, SyncCommand,
        },
        connection::ConnectionId,
        envelope::RandomEventIdGenerator,
        event::RuntimeEvent,
        introspection::{AppServerInfoCommand, IntrospectionBridge},
        route_command,
        router::RuntimeRouter,
        service::{
            AbortOutcome, DeviceStateOutcome, InputAdmission, RuntimeCommandService,
            RuntimeEventSink, RuntimeStartOutcome, ServiceFuture, SyncOutcome,
        },
    },
};
use lotta_domain::{
    AgentId, BoundedJsonValue, BoundedVec, ConversationId, InputDisposition, NonEmptyString, RunId,
    RuntimeScope,
};
use lotta_runtime::{CompactionMode, ports::ProviderRequest, turn::CompactionProgress};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const CONNECTION: ConnectionId = 111;

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent-outbound").expect("agent"),
        ConversationId::accept("conversation-outbound").expect("conversation"),
        None,
    )
}

fn text(value: &str) -> NonEmptyString {
    NonEmptyString::new(value.to_owned()).expect("bounded text")
}

fn bounded(value: Value) -> BoundedJsonValue {
    BoundedJsonValue::new(value).expect("bounded json")
}

/// Emits one runtime event through the production router broadcast machinery
/// and returns the delivered frame discriminant.
fn emitted_broadcast_discriminant(event: &RuntimeEvent) -> String {
    let clock = Arc::new(lotta_testkit::clock::FakeClock::new(
        lotta_domain::Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z")
            .expect("fixture instant"),
    ));
    let mut router = RuntimeRouter::new(clock, Arc::new(RandomEventIdGenerator));
    let connection = router.connections.open().expect("connection opens");
    router
        .connections
        .initialize(connection)
        .expect("initializes");
    let scope = scope();
    router
        .connections
        .subscribe(connection, scope.clone())
        .expect("subscribes");
    let deliveries = router.broadcast(&scope, event).expect("broadcasts");
    assert_eq!(deliveries.len(), 1, "one subscribed target");
    let frame = serde_json::to_value(&deliveries.as_slice()[0].frame).expect("frame encodes");
    frame["type"]
        .as_str()
        .expect("stamped frames carry their type")
        .to_owned()
}

#[test]
fn control_row_emits_control_request() {
    let members = members_in_fixture(&CONTROL_ROW).expect("pinned row resolves");
    let emitted = emitted_broadcast_discriminant(&RuntimeEvent::ControlRequest {
        request_id: text("approval-1"),
        request: crate::ws::test_support::approval_request(),
        agent_id: None,
        conversation_id: None,
    });
    assert!(
        members.contains(&emitted),
        "emitted {emitted} must belong to Control-row members {members:?}"
    );
}

#[tokio::test]
async fn admission_row_emits_input_accepted() {
    let members = members_in_fixture(&ADMISSION_ROW).expect("pinned row resolves");
    let clock = Arc::new(lotta_testkit::clock::FakeClock::new(
        lotta_domain::Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z")
            .expect("fixture instant"),
    ));
    let router = Arc::new(std::sync::Mutex::new(RuntimeRouter::new(
        clock,
        Arc::new(RandomEventIdGenerator),
    )));
    let command = InputCommand {
        request_id: Some(text("in-1")),
        runtime: scope(),
        payload: bounded(json!({"text": "hello"})),
    };
    let (output, deferred) = route_command(
        Arc::clone(&router),
        Arc::new(RejectingService),
        CONNECTION,
        RuntimeCommand::Input(command),
    )
    .await
    .expect("rejected admissions still route an acknowledgement");
    assert_eq!(
        deferred.as_ref().map(|deferred| deferred.disposition),
        Some(InputDisposition::Rejected),
        "rejections defer no continuation"
    );
    assert_eq!(output.responses.as_slice().len(), 1);
    let response = serde_json::to_value(&output.responses.as_slice()[0]).expect("encodes");
    let emitted = response["type"].as_str().expect("typed response");
    assert_eq!(emitted, "input_accepted", "admissions acknowledge first");
    assert_eq!(response["accepted"], false);
    assert!(
        members.contains(&emitted.to_owned()),
        "emitted {emitted} must belong to Admission-row members {members:?}"
    );
}

#[test]
fn state_row_emits_device_snapshot() {
    let members = members_in_fixture(&STATE_ROW).expect("pinned row resolves");
    let emitted = emitted_broadcast_discriminant(&RuntimeEvent::UpdateDeviceStatus {
        device_status: Box::new(crate::ws::test_support::device_status()),
    });
    assert_eq!(emitted, "update_device_status");
    assert!(
        members.contains(&emitted),
        "emitted {emitted} must belong to State-row members {members:?}"
    );
}

#[test]
fn stream_row_emits_stream_delta() {
    let members = members_in_fixture(&STREAM_ROW).expect("pinned row resolves");
    let emitted = emitted_broadcast_discriminant(&RuntimeEvent::StreamDelta {
        delta: crate::ws::event::StreamDelta::Other(bounded(json!({"type": "text", "text": "hi"}))),
        subagent_id: None,
    });
    assert!(
        members.contains(&emitted),
        "emitted {emitted} must belong to Stream-row members {members:?}"
    );
}

#[test]
fn terminal_row_emits_turn_finished() {
    let members = members_in_fixture(&TERMINAL_ROW).expect("pinned row resolves");
    let emitted = emitted_broadcast_discriminant(&RuntimeEvent::TurnFinished {
        turn_id: text("turn-1"),
        run_id: Some(RunId::generate_sequence(1).expect("run id")),
        stop_reason: text("end_turn"),
        error: None,
    });
    assert!(
        members.contains(&emitted),
        "emitted {emitted} must belong to Terminal-row members {members:?}"
    );
}

#[test]
fn management_row_emits_app_server_info() {
    let members = members_in_fixture(&MANAGEMENT_ROW).expect("pinned row resolves");
    let captured: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = Arc::clone(&captured);
    let forward = Arc::new(
        move |_: ConnectionId, message| -> Result<(), AppServerError> {
            sink.lock()
                .expect("capture lock")
                .push(serde_json::to_value(&message).expect("encodes"));
            Ok(())
        },
    );
    // The introspection bridge serves the canonical management discovery answer.
    let bridge = IntrospectionBridge::new(forward);
    bridge.register_authenticated(CONNECTION);
    let served = bridge.handle(
        CONNECTION,
        &AppServerInfoCommand {
            request_id: "mgmt-1".to_owned(),
        },
    );
    assert!(served, "authenticated management discovery answers");
    let frames = captured.lock().expect("capture lock").clone();
    assert_eq!(frames.len(), 1, "exactly one response");
    let emitted = frames[0]["type"].as_str().expect("typed").to_owned();
    assert_eq!(emitted, "app_server_info_response");
    assert!(
        members.contains(&emitted),
        "emitted {emitted} must belong to Management-row members {members:?}"
    );
}

/// Service seam answering every input with a rejection acknowledgement.
struct RejectingService;

impl RuntimeCommandService for RejectingService {
    fn runtime_start(&self, _: RuntimeStartCommand) -> ServiceFuture<'_, RuntimeStartOutcome> {
        Box::pin(async { Err(AppServerError::Unavailable) })
    }

    fn admit_input(&self, _: InputCommand) -> ServiceFuture<'_, InputAdmission> {
        fn rejected<'a>() -> ServiceFuture<'a, InputAdmission> {
            Box::pin(std::future::ready(Ok(InputAdmission {
                disposition: InputDisposition::Rejected,
                error: None,
                continuation: None,
                after_ack: BoundedVec::new(Vec::new()).expect("empty batch"),
            })))
        }
        Box::pin(rejected())
    }

    fn continue_input(
        &self,
        _: RuntimeScope,
        _: Option<BoundedJsonValue>,
        _: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()> {
        Box::pin(async { Err(AppServerError::Unavailable) })
    }

    fn compact(
        &self,
        _: RuntimeScope,
        _: CompactionMode,
        _: NonEmptyString,
        _: ProviderRequest,
    ) -> ServiceFuture<'_, CompactionProgress> {
        Box::pin(async { Err(AppServerError::Unavailable) })
    }

    fn sync(&self, _: SyncCommand) -> ServiceFuture<'_, SyncOutcome> {
        Box::pin(async { Err(AppServerError::Unavailable) })
    }

    fn abort_message(&self, _: AbortMessageCommand) -> ServiceFuture<'_, AbortOutcome> {
        Box::pin(async { Err(AppServerError::Unavailable) })
    }

    fn change_device_state(
        &self,
        _: ChangeDeviceStateCommand,
    ) -> ServiceFuture<'_, DeviceStateOutcome> {
        Box::pin(async { Err(AppServerError::Unavailable) })
    }
}

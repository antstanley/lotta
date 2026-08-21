//! `ws::teleport::failure` certificate selectors.
//!
//! `teleport_failed` must leave the runtime in a defined state: the
//! pending-teleport set is empty afterwards and a subsequent input is still
//! admitted normally.

use std::sync::{Arc, Mutex};

use lotta_domain::{
    AgentId, BoundedJsonValue, ConversationId, EntityExtras, NonEmptyString, QueueItem,
    QueueItemKind, QueueItemSource, RuntimeScope, Timestamp,
};
use lotta_runtime::{AdmissionOutcome, AdmissionRequest, AdmissionRoute, ListenerRuntime};
use serde_json::json;

use super::{
    TeleportBridge, TeleportFailedCommand, TeleportForwarder, TeleportMessage,
    TeleportProbeCommand, TeleportRequestCommand, TeleportTarget,
};

const ENQUEUED_AT: &str = "2026-08-14T12:34:56Z";

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent-1").expect("agent"),
        ConversationId::accept("conversation-1").expect("conversation"),
        None,
    )
}

fn request_command(teleport_id: &str) -> TeleportRequestCommand {
    TeleportRequestCommand {
        request_id: NonEmptyString::new("request-1").expect("request id"),
        teleport_id: NonEmptyString::new(teleport_id).expect("teleport id"),
        runtime: scope(),
        target: TeleportTarget {
            connection_id: "connection-9".to_owned(),
            device_id: "device-9".to_owned(),
            connection_name: "destination".to_owned(),
        },
    }
}

fn failed_command(teleport_id: &str) -> TeleportFailedCommand {
    TeleportFailedCommand {
        teleport_id: NonEmptyString::new(teleport_id).expect("teleport id"),
        runtime: scope(),
        error: "destination disconnected".to_owned(),
    }
}

fn ordinary_input(number: usize) -> AdmissionRequest {
    AdmissionRequest {
        item: QueueItem {
            id: NonEmptyString::new(format!("item-{number}")).expect("item id"),
            client_message_id: NonEmptyString::new(format!("client-{number}")).expect("client id"),
            kind: QueueItemKind::Message,
            source: QueueItemSource::User,
            content: BoundedJsonValue::new(json!({"text": "hello"})).expect("content"),
            enqueued_at: Timestamp::parse_persisted_rfc3339(ENQUEUED_AT).expect("timestamp"),
            extras: EntityExtras::default(),
        },
        route: AdmissionRoute::Ordinary,
    }
}

#[test]
fn failure_clears_pending_and_admits_subsequent_input() {
    let (bridge, frames) = recorder();
    let mut runtime = ListenerRuntime::new();
    let handle = runtime
        .get_or_create(&scope(), uuid::Uuid::from_u128(1))
        .expect("runtime");

    // An occupied scope leaves a pending teleport registered.
    bridge.request(9, &request_command("teleport-7"), true);
    assert_eq!(bridge.pending_len(), 1, "teleport stays pending while busy");
    assert!(
        frames.lock().expect("frames").is_empty(),
        "a pending teleport emits no ready outcome"
    );

    // The failure removes the pending entry without any emission.
    assert!(bridge.failed(&failed_command("teleport-7")));
    assert_eq!(
        bridge.pending_len(),
        0,
        "no dangling teleport may survive a failure"
    );

    // The runtime still admits subsequent input normally.
    let outcome = runtime
        .admit(&handle, ordinary_input(1))
        .expect("admission");
    assert!(
        matches!(outcome, AdmissionOutcome::Start(_)),
        "subsequent input starts a normal turn"
    );
    assert!(frames.lock().expect("frames").is_empty());
}

#[test]
fn unknown_failure_identity_changes_nothing() {
    let (bridge, _) = recorder();
    bridge.request(3, &request_command("teleport-7"), true);
    assert!(!bridge.failed(&failed_command("teleport-other")));
    assert_eq!(bridge.pending_len(), 1);
    assert!(bridge.failed(&failed_command("teleport-7")));
    assert_eq!(bridge.pending_len(), 0);
}

#[test]
fn probe_answers_without_touching_pending_state() {
    let (bridge, frames) = recorder();
    let command = TeleportProbeCommand {
        request_id: NonEmptyString::new("probe-1").expect("probe id"),
        runtime: scope(),
    };
    bridge.probe(5, &command);
    assert_eq!(bridge.pending_len(), 0);
    let recorded = frames.lock().expect("frames");
    assert_eq!(recorded.len(), 1, "probe answers exactly one response");
    assert_eq!(
        serde_json::to_value(recorded.first().expect("message")).expect("wire")["type"],
        "teleport_probe_response"
    );
}

fn recorder() -> (TeleportBridge, Arc<Mutex<Vec<TeleportMessage>>>) {
    let frames: Arc<Mutex<Vec<TeleportMessage>>> = Arc::default();
    let sink = Arc::clone(&frames);
    let forward: TeleportForwarder = Arc::new(move |_, message| {
        sink.lock().expect("frames").push(message);
        Ok(())
    });
    (TeleportBridge::new(forward), frames)
}

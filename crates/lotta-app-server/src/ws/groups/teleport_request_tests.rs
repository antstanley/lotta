//! Teleport request disposition selectors: conflict, idempotent replay, and
//! pinned wire-shape rejection.

use std::sync::{Arc, Mutex};

use lotta_domain::{AgentId, ConversationId, NonEmptyString, RuntimeScope};
use serde_json::{Value, json};

use super::{
    TeleportBridge, TeleportCommand, TeleportForwarder, TeleportMessage, TeleportRequestCommand,
    TeleportTarget,
};

fn scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent-1").expect("agent"),
        ConversationId::accept("conversation-1").expect("conversation"),
        None,
    )
}

fn other_scope() -> RuntimeScope {
    RuntimeScope::new(
        AgentId::accept("agent-1").expect("agent"),
        ConversationId::accept("conversation-2").expect("conversation"),
        None,
    )
}

fn request_command(scope_value: RuntimeScope, teleport_id: &str) -> TeleportRequestCommand {
    TeleportRequestCommand {
        request_id: NonEmptyString::new("request-1").expect("request id"),
        teleport_id: NonEmptyString::new(teleport_id).expect("teleport id"),
        runtime: scope_value,
        target: TeleportTarget {
            connection_id: "connection-9".to_owned(),
            device_id: "device-9".to_owned(),
            connection_name: "destination".to_owned(),
        },
    }
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

fn ready_of(message: &TeleportMessage) -> Value {
    serde_json::to_value(message).expect("wire")
}

#[test]
fn idle_scope_answers_ready_immediately() {
    let (bridge, frames) = recorder();
    bridge.request(1, &request_command(scope(), "teleport-1"), false);
    assert_eq!(bridge.pending_len(), 1);
    let recorded = frames.lock().expect("frames");
    assert_eq!(recorded.len(), 1);
    let ready = ready_of(recorded.first().expect("message"));
    assert_eq!(ready["type"], "teleport_ready");
    assert_eq!(ready["success"], true);
    assert_eq!(ready["active_turn"], false);
    assert_eq!(ready["teleport_id"], "teleport-1");
}

#[test]
fn repeat_identical_request_replays_recorded_outcome() {
    let (bridge, frames) = recorder();
    bridge.request(1, &request_command(scope(), "teleport-1"), false);
    bridge.request(2, &request_command(scope(), "teleport-1"), false);
    let recorded = frames.lock().expect("frames");
    assert_eq!(recorded.len(), 2, "replay emits the same ready outcome");
    assert_eq!(
        ready_of(recorded.first().expect("first")),
        ready_of(recorded.get(1).expect("second"))
    );
}

#[test]
fn concurrent_different_teleport_answers_conflict_failure() {
    let (bridge, frames) = recorder();
    bridge.request(1, &request_command(scope(), "teleport-1"), true);
    assert!(frames.lock().expect("frames").is_empty());
    bridge.request(2, &request_command(scope(), "teleport-2"), false);
    let recorded = frames.lock().expect("frames");
    assert_eq!(
        recorded.len(),
        1,
        "conflict answers without displacing pending"
    );
    let ready = ready_of(recorded.first().expect("message"));
    assert_eq!(ready["success"], false);
    assert_eq!(
        ready["error"],
        "Conversation already has a teleport pending"
    );
    drop(recorded);
    assert_eq!(bridge.pending_len(), 1);
}

#[test]
fn busy_scopes_are_independent() {
    let (bridge, frames) = recorder();
    bridge.request(1, &request_command(scope(), "teleport-1"), true);
    bridge.request(2, &request_command(other_scope(), "teleport-2"), false);
    let recorded = frames.lock().expect("frames");
    assert_eq!(recorded.len(), 1, "only the idle scope answers");
    assert_eq!(
        ready_of(recorded.first().expect("message"))["teleport_id"],
        "teleport-2"
    );
}

#[test]
fn malformed_wire_shapes_are_rejected() {
    let rejected = [
        json!({"type": "teleport_probe", "runtime": {"agent_id": "a", "conversation_id": "c"}}),
        json!({"type": "teleport_probe", "request_id": ""}),
        json!({
            "type": "teleport_request",
            "request_id": "r",
            "teleport_id": "t",
            "runtime": {"agent_id": "a", "conversation_id": "c"},
        }),
        json!({
            "type": "teleport_request",
            "request_id": "r",
            "teleport_id": "t",
            "runtime": {"agent_id": "a", "conversation_id": "c"},
            "target": {"connection_id": 7, "device_id": "d", "connection_name": "n"},
        }),
        json!({"type": "teleport_failed", "runtime": {"agent_id": "a", "conversation_id": "c"}}),
        json!({
            "type": "teleport_failed",
            "teleport_id": "t",
            "runtime": {"agent_id": "a", "conversation_id": "c"},
            "error": 42,
        }),
    ];
    for value in &rejected {
        let frame = crate::framing::decode_text(&value.to_string()).expect("frame");
        assert!(
            super::decode(&frame).is_err(),
            "pin rejects malformed teleport command: {value}"
        );
    }
    let accepted = json!({
        "type": "teleport_failed",
        "teleport_id": "t",
        "runtime": {"agent_id": "a", "conversation_id": "c"},
        "error": "boom",
    });
    let frame = crate::framing::decode_text(&accepted.to_string()).expect("frame");
    assert!(
        matches!(
            super::decode(&frame).expect("decoded"),
            Some(TeleportCommand::Failed(_))
        ),
        "pin accepts the exact failed shape"
    );
}

//! `ws::teleport::fixture_round_trip` certificate selectors.

use super::{TeleportCommand, TeleportMessage};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const COMMAND_TAGS: [&str; 3] = ["teleport_probe", "teleport_request", "teleport_failed"];
const MESSAGE_TAGS: [&str; 2] = ["teleport_probe_response", "teleport_ready"];

fn fixture_discriminants(section: &str) -> Vec<String> {
    let raw = include_str!("../../../../../fixtures/protocol/discriminants.json");
    let fixture: Value = serde_json::from_str(raw).expect("bounded fixture");
    fixture[section]["discriminants"]
        .as_array()
        .expect("discriminants")
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn round_trip<T: DeserializeOwned + serde::Serialize>(sample: &Value) {
    let parsed: T = serde_json::from_value(sample.clone()).expect("typed");
    assert_eq!(serde_json::to_value(&parsed).expect("wire"), *sample);
}

#[test]
fn covers_exactly_this_group() {
    let commands = fixture_discriminants("commands");
    let messages = fixture_discriminants("messages");
    for tag in COMMAND_TAGS {
        assert!(commands.iter().any(|entry| entry == tag));
    }
    for tag in MESSAGE_TAGS {
        assert!(messages.iter().any(|entry| entry == tag));
    }
    assert_eq!(
        commands
            .iter()
            .filter(|entry| COMMAND_TAGS.contains(&entry.as_str()))
            .count(),
        COMMAND_TAGS.len()
    );
    assert_eq!(
        messages
            .iter()
            .filter(|entry| MESSAGE_TAGS.contains(&entry.as_str()))
            .count(),
        MESSAGE_TAGS.len()
    );
}

#[test]
fn teleport_probe_round_trip() {
    round_trip::<TeleportCommand>(&json!({
        "type": "teleport_probe",
        "request_id": "probe-1",
        "runtime": {"agent_id": "agent-1", "conversation_id": "conversation-1"},
    }));
}

#[test]
fn teleport_request_round_trip() {
    round_trip::<TeleportCommand>(&json!({
        "type": "teleport_request",
        "request_id": "request-1",
        "teleport_id": "teleport-1",
        "runtime": {"agent_id": "agent-1", "conversation_id": "conversation-1"},
        "target": {
            "connection_id": "connection-9",
            "device_id": "device-9",
            "connection_name": "Stan's MacBook Pro",
        },
    }));
}

#[test]
fn teleport_failed_round_trip() {
    round_trip::<TeleportCommand>(&json!({
        "type": "teleport_failed",
        "teleport_id": "teleport-1",
        "runtime": {"agent_id": "agent-1", "conversation_id": "conversation-1"},
        "error": "destination disconnected",
    }));
}

#[test]
fn teleport_probe_response_round_trip() {
    round_trip::<TeleportMessage>(&json!({
        "type": "teleport_probe_response",
        "request_id": "probe-1",
        "runtime": {"agent_id": "agent-1", "conversation_id": "conversation-1"},
        "supported": true,
        "drains_accepted_inputs": true,
        "idempotent_continuation": true,
    }));
}

#[test]
fn teleport_ready_round_trip() {
    round_trip::<TeleportMessage>(&json!({
        "type": "teleport_ready",
        "teleport_id": "teleport-1",
        "runtime": {"agent_id": "agent-1", "conversation_id": "conversation-1"},
        "success": true,
        "active_turn": false,
    }));
    round_trip::<TeleportMessage>(&json!({
        "type": "teleport_ready",
        "teleport_id": "teleport-2",
        "runtime": {"agent_id": "agent-1", "conversation_id": "conversation-1"},
        "success": true,
        "active_turn": true,
        "mode": "acceptEdits",
        "continuation": {"approvals": [{"behavior": "allow"}]},
    }));
    round_trip::<TeleportMessage>(&json!({
        "type": "teleport_ready",
        "teleport_id": "teleport-3",
        "runtime": {"agent_id": "agent-1", "conversation_id": "conversation-1"},
        "success": false,
        "active_turn": false,
        "error": "Conversation already has a teleport pending",
    }));
}

//! `ws::device::fixture_round_trip` — every device group discriminant
//! round-trips against the pinned protocol fixture listing.

use serde_json::{Value, json};

use super::DeviceCommand;

const COMMAND_TAGS: [&str; 6] = [
    "execute_command",
    "remove_queue_item",
    "search_branches",
    "checkout_branch",
    "secret_list",
    "secret_apply",
];

const MESSAGE_TAGS: [&str; 8] = [
    "execute_command_response",
    "remove_queue_item_response",
    "search_branches_response",
    "checkout_branch_response",
    "secret_list_response",
    "secret_apply_response",
    "update_queue",
    "update_device_status",
];

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

fn round_trip(sample: &Value) {
    let parsed: DeviceCommand = serde_json::from_value(sample.clone()).expect("typed");
    assert_eq!(serde_json::to_value(&parsed).expect("wire"), *sample);
}

#[test]
fn covers_exactly_this_group() {
    let commands = fixture_discriminants("commands");
    let messages = fixture_discriminants("messages");
    for tag in COMMAND_TAGS {
        assert!(
            commands.iter().any(|entry| entry == tag),
            "{tag} listed in the commands fixture"
        );
    }
    for tag in MESSAGE_TAGS {
        assert!(
            messages.iter().any(|entry| entry == tag),
            "{tag} listed in the messages fixture"
        );
    }
}

#[test]
fn execute_command_round_trips() {
    round_trip(&json!({
        "type": "execute_command",
        "command_id": "clear",
        "request_id": "rt-exec",
        "runtime": {"agent_id": "a", "conversation_id": "c"},
        "args": "--all",
    }));
}

#[test]
fn remove_queue_item_round_trips() {
    round_trip(&json!({
        "type": "remove_queue_item",
        "request_id": "rt-rq",
        "runtime": {"agent_id": "a", "conversation_id": "c"},
        "item_id": "item-1",
    }));
}

#[test]
fn search_branches_round_trips() {
    round_trip(&json!({
        "type": "search_branches",
        "request_id": "rt-sb",
        "query": "feature",
        "max_results": 5,
        "cwd": "/tmp/repo",
    }));
}

#[test]
fn checkout_branch_round_trips() {
    round_trip(&json!({
        "type": "checkout_branch",
        "request_id": "rt-cb",
        "branch": "main",
        "create": false,
        "cwd": "/tmp/repo",
    }));
}

#[test]
fn secret_list_round_trips() {
    round_trip(&json!({
        "type": "secret_list",
        "request_id": "rt-sl",
        "agent_id": "agent-1",
    }));
}

#[test]
fn secret_apply_round_trips() {
    round_trip(&json!({
        "type": "secret_apply",
        "request_id": "rt-sa",
        "agent_id": "agent-1",
        "set": {"API_KEY": "v"},
        "unset": ["OLD"],
    }));
}

#[test]
fn execute_command_decodes_with_optional_args() {
    let value = json!({
        "type": "execute_command",
        "command_id": "clear",
        "request_id": "rt-exec",
        "runtime": {"agent_id": "a", "conversation_id": "c"},
        "args": "extra",
    });
    let frame_text = value.to_string();
    let frame = crate::framing::decode_text(&frame_text).expect("bounded frame");
    let decoded = super::decode(&frame).expect("wellformed").expect("routed");
    let DeviceCommand::ExecuteCommand(payload) = decoded else {
        panic!("execute_command decodes into the execute variant");
    };
    assert_eq!(payload.command_id, "clear");
    assert_eq!(payload.args.as_deref(), Some("extra"));
}

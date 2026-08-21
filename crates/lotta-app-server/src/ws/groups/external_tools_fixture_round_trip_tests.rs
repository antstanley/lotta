//! `ws::external_tools::fixture_round_trip` certificate selectors.

use super::{ExternalToolsCommand, ExternalToolsMessage};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const COMMAND_TAGS: [&str; 2] = [
    "runtime_external_tools_update",
    "external_tool_call_response",
];
const MESSAGE_TAGS: [&str; 2] = [
    "external_tool_call_request",
    "runtime_external_tools_update_response",
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
fn runtime_external_tools_update_round_trip() {
    round_trip::<ExternalToolsCommand>(&json!({
        "type": "runtime_external_tools_update",
        "request_id": "update-1",
        "updates": [{
            "runtimes": [
                {"agent_id": "agent-1", "conversation_id": "conversation-1"},
                {"agent_id": "agent-2", "conversation_id": "conversation-2"},
            ],
            "external_tools": [{
                "scope_id": "scope-a",
                "tools": [{
                    "name": "echo",
                    "label": "Echo",
                    "description": "controller tool",
                    "parameters": {"type": "object"},
                }],
            }],
        }],
    }));
}

#[test]
fn external_tool_call_response_round_trip() {
    round_trip::<ExternalToolsCommand>(&json!({
        "type": "external_tool_call_response",
        "request_id": "external-tool-1",
        "result": {"content": [{"type": "text", "text": "ok"}]},
        "tool_call_id": "call-1",
    }));
    round_trip::<ExternalToolsCommand>(&json!({
        "type": "external_tool_call_response",
        "request_id": "external-tool-2",
        "error": "controller failed",
    }));
}

#[test]
fn external_tool_call_request_round_trip() {
    round_trip::<ExternalToolsMessage>(&json!({
        "type": "external_tool_call_request",
        "request_id": "external-tool-3",
        "runtime": {"agent_id": "agent-1", "conversation_id": "conversation-1"},
        "scope_id": "scope-a",
        "tool_call_id": "call-3",
        "tool_name": "echo",
        "input": {"prompt": "hi"},
    }));
    round_trip::<ExternalToolsMessage>(&json!({
        "type": "external_tool_call_request",
        "request_id": "external-tool-4",
        "tool_call_id": "call-4",
        "tool_name": "plain",
        "input": {},
    }));
}

#[test]
fn runtime_external_tools_update_response_round_trip() {
    round_trip::<ExternalToolsMessage>(&json!({
        "type": "runtime_external_tools_update_response",
        "request_id": "update-1",
        "success": true,
    }));
    round_trip::<ExternalToolsMessage>(&json!({
        "type": "runtime_external_tools_update_response",
        "request_id": "update-2",
        "success": false,
        "error": "invalid schema",
    }));
}

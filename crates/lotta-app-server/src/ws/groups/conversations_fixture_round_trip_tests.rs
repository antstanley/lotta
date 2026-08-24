//! `ws::conversations::fixture_round_trip` — every group discriminant
//! round-trips against the pinned protocol fixture listing.

use serde_json::{Value, json};

use super::ConversationsCommand;

const COMMAND_TAGS: [&str; 8] = [
    "conversation_list",
    "conversation_retrieve",
    "conversation_create",
    "conversation_update",
    "conversation_recompile",
    "conversation_fork",
    "conversation_messages_list",
    "conversation_compact",
];

const MESSAGE_TAGS: [&str; 8] = [
    "conversation_list_response",
    "conversation_retrieve_response",
    "conversation_create_response",
    "conversation_update_response",
    "conversation_recompile_response",
    "conversation_fork_response",
    "conversation_messages_list_response",
    "conversation_compact_response",
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
    let parsed: ConversationsCommand = serde_json::from_value(sample.clone()).expect("typed");
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
fn conversation_list_round_trips() {
    round_trip(&json!({"type": "conversation_list", "request_id": "rt-cl"}));
    round_trip(&json!({
        "type": "conversation_list",
        "request_id": "rt-cl-2",
        "query": {
            "agent_id": "agent-local-fixture",
            "summary_search": "retrieval",
            "include_hidden": true,
            "limit": 25,
            "after": "local-conv-9",
        },
    }));
}

#[test]
fn conversation_retrieve_round_trips() {
    round_trip(&json!({
        "type": "conversation_retrieve",
        "request_id": "rt-cr",
        "conversation_id": "local-conv-1",
    }));
}

#[test]
fn conversation_create_round_trips() {
    round_trip(&json!({
        "type": "conversation_create",
        "request_id": "rt-cc-1",
        "body": {"agent_id": "agent-local-fixture"},
    }));
    round_trip(&json!({
        "type": "conversation_create",
        "request_id": "rt-cc-2",
        "body": {
            "agent_id": "agent-local-fixture",
            "summary": "Fixture summary",
            "context_window_limit": 4096,
            "hidden": false,
            "tags": ["team-a"],
        },
    }));
}

#[test]
fn conversation_update_round_trips() {
    round_trip(&json!({
        "type": "conversation_update",
        "request_id": "rt-cu-1",
        "conversation_id": "local-conv-1",
        "body": {},
    }));
    round_trip(&json!({
        "type": "conversation_update",
        "request_id": "rt-cu-2",
        "conversation_id": "local-conv-1",
        "body": {
            "archived": false,
            "last_message_at": "2026-01-02T03:04:05Z",
            "summary": null,
            "model": "model-base",
            "hidden": true,
            "context_window_limit": 1024,
            "tags": ["next"],
        },
    }));
}

#[test]
fn conversation_recompile_round_trips() {
    round_trip(&json!({
        "type": "conversation_recompile",
        "request_id": "rt-crec",
        "conversation_id": "local-conv-1",
    }));
    round_trip(&json!({
        "type": "conversation_recompile",
        "request_id": "rt-crec-2",
        "conversation_id": "local-conv-1",
        "body": {"agent_id": "agent-local-fixture", "dry_run": true},
    }));
}

#[test]
fn conversation_fork_round_trips() {
    round_trip(&json!({
        "type": "conversation_fork",
        "request_id": "rt-cf",
        "conversation_id": "local-conv-1",
    }));
    round_trip(&json!({
        "type": "conversation_fork",
        "request_id": "rt-cf-2",
        "conversation_id": "default",
        "body": {"agent_id": "agent-local-fixture", "hidden": true, "message_id": "letta-msg-3"},
    }));
}

#[test]
fn conversation_messages_list_round_trips() {
    round_trip(&json!({
        "type": "conversation_messages_list",
        "request_id": "rt-cml",
        "conversation_id": "local-conv-1",
    }));
    round_trip(&json!({
        "type": "conversation_messages_list",
        "request_id": "rt-cml-2",
        "conversation_id": "local-conv-1",
        "query": {
            "limit": 10,
            "before": "letta-msg-9",
            "after": "letta-msg-2",
            "order": "asc",
            "include_return_message_types": ["user_message", "tool_return_message"],
        },
    }));
}

#[test]
fn conversation_compact_round_trips() {
    round_trip(&json!({
        "type": "conversation_compact",
        "request_id": "rt-ccp",
        "conversation_id": "local-conv-1",
    }));
    round_trip(&json!({
        "type": "conversation_compact",
        "request_id": "rt-ccp-2",
        "conversation_id": "local-conv-1",
        "body": {"agent_id": "agent-local-fixture"},
    }));
}

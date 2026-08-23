//! `ws::agents::fixture_round_trip` — every group discriminant round-trips
//! against the pinned protocol fixture listing.

use serde_json::{Value, json};

use super::AgentsCommand;

const COMMAND_TAGS: [&str; 6] = [
    "create_agent",
    "agent_list",
    "agent_retrieve",
    "agent_create",
    "agent_update",
    "agent_delete",
];

const MESSAGE_TAGS: [&str; 6] = [
    "create_agent_response",
    "agent_list_response",
    "agent_retrieve_response",
    "agent_create_response",
    "agent_update_response",
    "agent_delete_response",
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
    let parsed: AgentsCommand = serde_json::from_value(sample.clone()).expect("typed");
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
fn create_agent_round_trips() {
    round_trip(&json!({
        "type": "create_agent",
        "request_id": "rt-ca-1",
        "personality": "memo",
    }));
    round_trip(&json!({
        "type": "create_agent",
        "request_id": "rt-ca-2",
        "personality": "kawaii",
        "model": "auto-chat",
        "tags": ["team-b"],
        "pin_global": false,
    }));
}

#[test]
fn agent_list_round_trips() {
    round_trip(&json!({"type": "agent_list", "request_id": "rt-al"}));
    round_trip(&json!({
        "type": "agent_list",
        "request_id": "rt-al-2",
        "query": {
            "name": "archive",
            "query_text": "retrieval",
            "tags": ["team-a", "team-b"],
            "hidden": true,
            "limit": 25,
        },
    }));
    // Pagination continuation echoes the last served agent identifier.
    round_trip(&json!({
        "type": "agent_list",
        "request_id": "rt-al-3",
        "query": {"after": "agent-local-alpha-1"},
    }));
}

#[test]
fn agent_retrieve_round_trips() {
    round_trip(&json!({
        "type": "agent_retrieve",
        "request_id": "rt-ar",
        "agent_id": "agent-local-fixture",
    }));
}

#[test]
fn agent_create_round_trips() {
    round_trip(&json!({
        "type": "agent_create",
        "request_id": "rt-ac-1",
        "body": {"name": "Fixture Agent"},
    }));
    round_trip(&json!({
        "type": "agent_create",
        "request_id": "rt-ac-2",
        "body": {
            "name": "Fixture Agent",
            "model": "model-base",
            "system": "Fixture system prompt.",
            "description": null,
            "tags": ["team-a"],
            "model_settings": {"temperature": 0.2},
            "hidden": true,
            "compaction_settings": {"mode": "sliding_window"},
            "memory_blocks": [
                {"label": "persona", "value": "Fixture persona.", "description": "Persona"},
                {"label": "notes", "value": "Un-described block."},
            ],
        },
    }));
    round_trip(&json!({
        "type": "agent_create",
        "request_id": "rt-ac-3",
        "body": {
            "name": "Cleared Agent",
            "description": "Described.",
            "hidden": false,
            "compaction_settings": null,
        },
    }));
}

#[test]
fn agent_update_round_trips() {
    round_trip(&json!({
        "type": "agent_update",
        "request_id": "rt-au-1",
        "agent_id": "agent-local-fixture",
        "body": {},
    }));
    round_trip(&json!({
        "type": "agent_update",
        "request_id": "rt-au-2",
        "agent_id": "agent-local-fixture",
        "body": {
            "name": "Renamed",
            "description": "Now described.",
            "system": "Replacement prompt.",
            "tags": ["next"],
            "model": "model-next",
            "model_settings": {},
            "hidden": null,
            "compaction_settings": {"mode": "all", "prompt": "Summarize."},
        },
    }));
    round_trip(&json!({
        "type": "agent_update",
        "request_id": "rt-au-3",
        "agent_id": "agent-local-fixture",
        "body": {"compaction_settings": null},
    }));
}

#[test]
fn agent_delete_round_trips() {
    round_trip(&json!({
        "type": "agent_delete",
        "request_id": "rt-ad",
        "agent_id": "agent-local-fixture",
    }));
}

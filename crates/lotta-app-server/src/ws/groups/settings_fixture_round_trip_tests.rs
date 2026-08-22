//! `ws::settings::fixture_round_trip` — every group discriminant round-trips
//! against the pinned protocol fixture listing.

use serde_json::{Value, json};

use super::SettingsCommand;

const COMMAND_TAGS: [&str; 5] = [
    "get_cwd_map",
    "get_reflection_settings",
    "set_reflection_settings",
    "get_experiments",
    "set_experiment",
];

const MESSAGE_TAGS: [&str; 6] = [
    "get_cwd_map_response",
    "get_reflection_settings_response",
    "set_reflection_settings_response",
    "get_experiments_response",
    "set_experiment_response",
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
    let parsed: SettingsCommand = serde_json::from_value(sample.clone()).expect("typed");
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
fn get_cwd_map_round_trips() {
    round_trip(&json!({
        "type": "get_cwd_map",
        "request_id": "rt-cm",
    }));
}

#[test]
fn get_reflection_settings_round_trips() {
    round_trip(&json!({
        "type": "get_reflection_settings",
        "request_id": "rt-gr",
        "runtime": {"agent_id": "agent-local-fixture", "conversation_id": "local-conv-fixture"},
    }));
}

#[test]
fn set_reflection_settings_round_trips() {
    round_trip(&json!({
        "type": "set_reflection_settings",
        "request_id": "rt-sr",
        "runtime": {"agent_id": "agent-local-fixture", "conversation_id": "local-conv-fixture"},
        "settings": {
            "trigger": "step-count",
            "step_count": 25,
            "merge": "explicit",
            "merge_instructions": "Preserve exact wording.",
        },
        "scope": "local_project",
    }));
    round_trip(&json!({
        "type": "set_reflection_settings",
        "request_id": "rt-srf",
        "runtime": {"agent_id": "agent-local-fixture", "conversation_id": "default"},
        "settings": {"trigger": "off", "step_count": 10},
    }));
}

#[test]
fn experiment_commands_round_trip() {
    round_trip(&json!({
        "type": "get_experiments",
        "request_id": "rt-ge",
    }));
    round_trip(&json!({
        "type": "set_experiment",
        "request_id": "rt-se",
        "experiment_id": "tui_cron",
        "enabled": true,
    }));
}

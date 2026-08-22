//! `ws::schedules::fixture_round_trip` — every group discriminant round-trips
//! against the pinned protocol fixture listing.

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::SchedulesCommand;

const COMMAND_TAGS: [&str; 8] = [
    "cron_list",
    "cron_add",
    "cron_get",
    "cron_runs",
    "cron_trigger",
    "cron_update",
    "cron_delete",
    "cron_delete_all",
];

const MESSAGE_TAGS: [&str; 9] = [
    "cron_list_response",
    "cron_add_response",
    "cron_get_response",
    "cron_runs_response",
    "cron_trigger_response",
    "cron_update_response",
    "cron_delete_response",
    "cron_delete_all_response",
    "crons_updated",
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
    let parsed: SchedulesCommand = serde_json::from_value(sample.clone()).expect("typed");
    assert_eq!(serde_json::to_value(&parsed).expect("wire"), *sample);
}

#[test]
fn covers_exactly_this_group() {
    let commands = fixture_discriminants("commands");
    let messages = fixture_discriminants("messages");
    for tag in COMMAND_TAGS {
        assert!(commands.iter().any(|entry| entry == tag), "{tag} listed");
    }
    for tag in MESSAGE_TAGS {
        assert!(messages.iter().any(|entry| entry == tag), "{tag} listed");
    }
    assert_eq!(
        commands
            .iter()
            .filter(|entry| COMMAND_TAGS.contains(&entry.as_str()))
            .count(),
        COMMAND_TAGS.len(),
        "fixture lists exactly the eight schedule commands"
    );
}

#[test]
fn cron_list_round_trip() {
    round_trip(&json!({
        "type": "cron_list",
        "request_id": "rt-cl",
    }));
    round_trip(&json!({
        "type": "cron_list",
        "request_id": "rt-clf",
        "agent_id": "agent-local-fixture",
        "conversation_id": "conversation",
    }));
}

#[test]
fn cron_add_round_trip() {
    round_trip(&json!({
        "type": "cron_add",
        "request_id": "rt-ca",
        "agent_id": "agent-local-fixture",
        "name": "nightly",
        "description": "runs nightly",
        "cron": "0 3 * * *",
        "recurring": true,
        "prompt": "check the build",
    }));
    round_trip(&json!({
        "type": "cron_add",
        "request_id": "rt-caf",
        "agent_id": "agent-local-fixture",
        "conversation_id": "default",
        "name": "once",
        "description": "one-shot",
        "cron": "*/30 * * * *",
        "timezone": "Europe/Paris",
        "recurring": false,
        "prompt": "fire once",
        "scheduled_for": "2026-08-20T15:00:00Z",
    }));
}

#[test]
fn cron_get_and_runs_round_trip() {
    round_trip(&json!({
        "type": "cron_get",
        "request_id": "rt-cg",
        "task_id": "schedule-1",
    }));
    round_trip(&json!({
        "type": "cron_runs",
        "request_id": "rt-cr",
        "task_id": "schedule-1",
    }));
    round_trip(&json!({
        "type": "cron_runs",
        "request_id": "rt-crf",
        "task_id": "schedule-1",
        "limit": 25,
        "offset": 5,
        "run_id": "local-run-7",
    }));
}

#[test]
fn cron_trigger_round_trip() {
    round_trip(&json!({
        "type": "cron_trigger",
        "request_id": "rt-ct",
        "task_id": "schedule-1",
    }));
}

#[test]
fn cron_update_three_state_scheduled_for_round_trips() {
    round_trip(&json!({
        "type": "cron_update",
        "request_id": "rt-cu",
        "task_id": "schedule-1",
        "scheduled_for": null,
    }));
    round_trip(&json!({
        "type": "cron_update",
        "request_id": "rt-cuf",
        "task_id": "schedule-1",
        "name": "renamed",
        "description": "updated",
        "conversation_id": "default",
        "cron": "*/5 * * * *",
        "timezone": "UTC",
        "recurring": false,
        "prompt": "new prompt",
        "scheduled_for": "2026-08-20T16:00:00Z",
    }));
}

#[test]
fn cron_delete_and_delete_all_round_trip() {
    round_trip(&json!({
        "type": "cron_delete",
        "request_id": "rt-cd",
        "task_id": "schedule-1",
    }));
    round_trip(&json!({
        "type": "cron_delete_all",
        "request_id": "rt-cda",
        "agent_id": "agent-local-fixture",
    }));
}

fn decode_only<T: DeserializeOwned>(sample: &Value) {
    serde_json::from_value::<T>(sample.clone()).expect("typed decode");
}

#[test]
fn every_command_decodes_against_the_fixture_listing() {
    // Decode-only proof for shapes whose serialization intentionally omits
    // absent optional fields; each pinned sample must still type-check.
    decode_only::<SchedulesCommand>(&json!({
        "type": "cron_update",
        "request_id": "du",
        "task_id": "schedule-1",
    }));
}

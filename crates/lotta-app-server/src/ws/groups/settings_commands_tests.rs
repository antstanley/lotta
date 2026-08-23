//! `ws::settings::commands` — reads serve pinned response shapes, experiments
//! persist through the global side-store scope, and every successful mutation
//! emits one device-status snapshot after its response.

use serde_json::{Value, json};

use super::support::{bridge, discriminant_of, runtime};
use super::{
    GetExperimentsCommand, GetReflectionSettingsCommand, SetExperimentCommand, SettingsCommand,
};

/// Fixture agent identifier.
const AGENT_ID: &str = "agent-local-settings-commands";

#[test]
fn get_cwd_map_serves_the_boot_directory_before_changes() {
    let fixture = bridge();
    fixture
        .send(&json!({"type": "get_cwd_map", "request_id": "cm-0"}))
        .expect("wellformed command");
    assert_eq!(fixture.last()["type"], "get_cwd_map_response");
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(
        fixture.last()["cwd_map"],
        json!({}),
        "a fresh listener serves an empty map"
    );
    assert_eq!(
        fixture.last()["boot_working_directory"],
        fixture.workspace.to_string_lossy().as_ref(),
        "the boot directory defaults to the workspace root"
    );
}

#[test]
fn set_experiment_persists_through_the_global_side_store_scope() {
    let fixture = bridge();
    fixture
        .send(
            &serde_json::to_value(SettingsCommand::SetExperiment(SetExperimentCommand {
                request_id: "ex-1".to_owned(),
                experiment_id: serde_json::from_str("\"artifacts\"").expect("known id"),
                enabled: true,
            }))
            .expect("command encodes"),
        )
        .expect("wellformed command");
    let response = serde_json::to_value(fixture.messages()[0].clone()).expect("encodes");
    assert_eq!(response["type"], "set_experiment_response");
    assert_eq!(response["success"], true);
    // The override persists inside the scoped global document, read back
    // through the Task 27 side store.
    let document: Value = match lotta_store::side::settings::read(&fixture.paths) {
        Ok(source) => serde_json::from_slice(source.bytes()).expect("global document"),
        Err(error) => panic!("global read failed: {error:?}"),
    };
    assert_eq!(document["experiments"]["artifacts"], true);
    let listing = fixture.messages()[0].clone();
    let encoded = serde_json::to_value(&listing).expect("encodes");
    let artifacts = encoded["experiments"]
        .as_array()
        .expect("experiment list")
        .iter()
        .find(|snapshot| snapshot["id"] == "artifacts")
        .expect("artifacts snapshot")
        .clone();
    assert_eq!(artifacts["enabled"], true);
    assert_eq!(artifacts["source"], "override");
    assert_eq!(artifacts["override"], true);
}

#[test]
fn get_experiments_lists_every_pinned_definition() {
    let fixture = bridge();
    fixture
        .send(
            &serde_json::to_value(SettingsCommand::GetExperiments(GetExperimentsCommand {
                request_id: "ex-2".to_owned(),
            }))
            .expect("command encodes"),
        )
        .expect("wellformed command");
    assert_eq!(fixture.last()["success"], true);
    let listing = fixture.last();
    let ids: Vec<&str> = listing["experiments"]
        .as_array()
        .expect("list")
        .iter()
        .filter_map(|snapshot| snapshot["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "artifacts",
            "conversation_titles",
            "desktop_conversation_bootstrap",
            "diffs",
            "reflection_arena",
            "tui_cron",
        ],
        "pinned definitions in pinned order"
    );
    // Without overrides or env toggles everything resolves to defaults.
    for snapshot in fixture.last()["experiments"].as_array().expect("list") {
        assert_eq!(snapshot["source"], "default");
        assert_eq!(snapshot["enabled"], false);
        assert_eq!(snapshot["override"], json!(null));
    }
}

#[test]
fn get_reflection_settings_resolves_defaults_then_persisted_entries() {
    let fixture = bridge();
    fixture
        .send(
            &serde_json::to_value(SettingsCommand::GetReflectionSettings(
                GetReflectionSettingsCommand {
                    request_id: "rf-1".to_owned(),
                    runtime: runtime(AGENT_ID, "default"),
                },
            ))
            .expect("command encodes"),
        )
        .expect("wellformed command");
    assert_eq!(fixture.last()["success"], true);
    assert_eq!(
        fixture.last()["reflection_settings"]["trigger"],
        "compaction-event",
        "pinned default trigger"
    );
    assert_eq!(
        fixture.last()["reflection_settings"]["step_count"],
        25,
        "pinned default step count"
    );
    assert_eq!(fixture.last()["reflection_settings"]["merge"], "auto");
    assert_eq!(fixture.last()["reflection_settings"]["agent_id"], AGENT_ID);
    // After a write the read serves the persisted entry back.
    fixture
        .send(&json!({
            "type": "set_reflection_settings",
            "request_id": "rf-2",
            "runtime": {"agent_id": AGENT_ID, "conversation_id": "default"},
            "settings": {"trigger": "off", "step_count": 40},
            "scope": "global",
        }))
        .expect("wellformed command");
    fixture
        .send(
            &serde_json::to_value(SettingsCommand::GetReflectionSettings(
                GetReflectionSettingsCommand {
                    request_id: "rf-3".to_owned(),
                    runtime: runtime(AGENT_ID, "default"),
                },
            ))
            .expect("command encodes"),
        )
        .expect("wellformed command");
    assert_eq!(fixture.last()["reflection_settings"]["trigger"], "off");
    assert_eq!(fixture.last()["reflection_settings"]["step_count"], 40);
    // merge and instructions fall back to current values for older clients.
    assert_eq!(fixture.last()["reflection_settings"]["merge"], "auto");
}

#[test]
fn set_reflection_normalizes_scope_and_emits_snapshot_after_response() {
    let fixture = bridge();
    fixture
        .send(&json!({
            "type": "set_reflection_settings",
            "request_id": "rf-4",
            "runtime": {"agent_id": AGENT_ID, "conversation_id": "default"},
            "settings": {"trigger": "step-count", "step_count": 10},
        }))
        .expect("wellformed command");
    let kinds: Vec<&str> = fixture.messages().iter().map(discriminant_of).collect();
    assert_eq!(
        kinds,
        vec!["set_reflection_settings_response", "update_device_status",],
        "one device-status snapshot after the success response"
    );
    // Omitted scope normalizes to both.
    let response = serde_json::to_value(fixture.messages()[0].clone()).expect("encodes");
    assert_eq!(response["scope"], "both");
    let snapshot = &fixture.messages()[1];
    match snapshot {
        super::SettingsMessage::DeviceStatus { device_status } => {
            assert_eq!(
                device_status
                    .reflection_settings
                    .as_ref()
                    .expect("written entry")
                    .step_count,
                10,
                "the snapshot carries the written reflection entry"
            );
        }
        other => panic!("expected device status, got {other:?}"),
    }
}

#[test]
fn unknown_frames_are_not_routed_by_this_group() {
    let fixture = bridge();
    let text = json!({"type": "cron_list", "request_id": "other-1"}).to_string();
    let frame = crate::framing::decode_text(&text).expect("bounded frame");
    let routed = super::decode(&frame).expect("clean decode outcome");
    assert!(routed.is_none(), "foreign commands fall through the chain");
    assert!(
        fixture.is_empty(),
        "nothing is emitted for foreign commands"
    );
}

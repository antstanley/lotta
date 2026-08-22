//! `ws::skills::fixture_round_trip` — every group discriminant round-trips
//! against the pinned protocol fixture listing.

use serde_json::{Value, json};

use super::SkillsCommand;

const COMMAND_TAGS: [&str; 2] = ["skill_enable", "skill_disable"];

const MESSAGE_TAGS: [&str; 3] = [
    "skill_enable_response",
    "skill_disable_response",
    "skills_updated",
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
    let parsed: SkillsCommand = serde_json::from_value(sample.clone()).expect("typed");
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
fn skill_enable_round_trips() {
    round_trip(&json!({
        "type": "skill_enable",
        "request_id": "rt-se",
        "skill_path": "/tmp/skills/note_taker",
    }));
}

#[test]
fn skill_disable_round_trips() {
    round_trip(&json!({
        "type": "skill_disable",
        "request_id": "rt-sd",
        "name": "note_taker",
    }));
}

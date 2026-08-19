//! Canonical fixture round-trip tests validated against `$defs.Schedule`.

use super::{RunUpdate, ScheduleFile, apply_run_update};
use chrono::{TimeZone, Utc};
use lotta_domain::Timestamp;
use serde_json::Value;

const CANONICAL_SCHEMA_JSON: &str = include_str!("../../../../.specs/canonical-types.schema.json");
const FIXTURE_CRONS_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/persistence/side_stores/letta_home/crons.json"
));

#[test]
fn preserves_all_observed_fields_and_extensions() {
    let raw: Value = serde_json::from_slice(FIXTURE_CRONS_BYTES).expect("fixture");
    let decoded = ScheduleFile::decode(FIXTURE_CRONS_BYTES).expect("decode");
    assert_eq!(decoded.tasks.len(), 1);
    assert_eq!(decoded.tasks[0].id.as_str(), "schedule-fixture");
    assert_eq!(decoded.tasks[0].cron.as_str(), "0 0 * * *");
    assert_eq!(decoded.tasks[0].timezone.as_str(), "UTC");
    let mut value = raw;
    value["root_extension"] = serde_json::json!({"future": true});
    value["tasks"][0]["schedule_extension"] = serde_json::json!([1, 2, 3]);
    let source = serde_json::to_vec(&value).expect("source");
    let extended = ScheduleFile::decode(&source).expect("decode");
    let encoded = extended.encode().expect("encode");
    let round_trip = ScheduleFile::decode(&encoded).expect("decode encoded");
    assert_eq!(round_trip, extended);
    let output = serde_json::to_value(round_trip).expect("value");
    assert_eq!(output["root_extension"], value["root_extension"]);
    assert_eq!(
        output["tasks"][0]["schedule_extension"],
        value["tasks"][0]["schedule_extension"]
    );
    let task = output["tasks"][0].as_object().expect("task");
    for required in required_property_names() {
        assert!(task.contains_key(&required), "missing {required}");
    }
}

#[test]
fn reserialized_tasks_validate_against_canonical_schema() {
    let validator = schedule_validator();
    let fixture = ScheduleFile::decode(FIXTURE_CRONS_BYTES).expect("decode");
    let mut updated = fixture.clone();
    let now = Timestamp::from_utc(instant(1_767_225_600));
    apply_run_update(&mut updated.tasks[0], RunUpdate::Missed, now).expect("update");
    let reencoded = ScheduleFile::decode(&updated.encode().expect("encode")).expect("re-decode");
    for file in [fixture, updated, reencoded] {
        for task in &file.tasks {
            let output =
                serde_json::to_value(task).unwrap_or_else(|error| panic!("serialize: {error}"));
            assert!(validator.is_valid(&output), "schema violation: {output}");
        }
    }
}

fn schedule_validator() -> jsonschema::Validator {
    let root: Value = serde_json::from_str(CANONICAL_SCHEMA_JSON)
        .unwrap_or_else(|error| panic!("schema: {error}"));
    let mut definition = root["$defs"]["Schedule"].clone();
    if let Value::Object(map) = &mut definition {
        map.insert("$defs".into(), root["$defs"].clone());
    }
    jsonschema::validator_for(&definition).unwrap_or_else(|error| panic!("validator: {error}"))
}

fn required_property_names() -> Vec<String> {
    let root: Value = serde_json::from_str(CANONICAL_SCHEMA_JSON)
        .unwrap_or_else(|error| panic!("schema: {error}"));
    root["$defs"]["Schedule"]["required"]
        .as_array()
        .expect("required")
        .iter()
        .map(|value| value.as_str().expect("name").to_owned())
        .collect()
}

fn instant(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0).single().expect("test time")
}

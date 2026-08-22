//! `ws::settings::reflection_validation` — invalid reflection settings are
//! rejected before persistence with zero writes. The accepted and rejected
//! rule set mirrors the pinned `reflection-settings-validation.test.ts`.

use serde_json::{Value, json};

use super::support::bridge;

/// Fixture agent identifier.
const AGENT_ID: &str = "agent-local-reflection-validation";

fn command(settings: &Value) -> Value {
    json!({
        "type": "set_reflection_settings",
        "request_id": "request-1",
        "runtime": {
            "agent_id": AGENT_ID,
            "conversation_id": "default",
        },
        "settings": settings,
    })
}

/// Asserts one frame decodes as a valid command.
fn accepts(settings: &Value) {
    let text = command(settings).to_string();
    let frame = crate::framing::decode_text(&text).expect("bounded frame");
    let decoded = super::decode(&frame)
        .expect("valid reflection command")
        .expect("routed");
    assert!(matches!(
        decoded,
        super::SettingsCommand::SetReflectionSettings(_)
    ));
}

/// Asserts one frame is rejected with zero writes to any scope.
fn rejects(settings: &Value) {
    let fixture = bridge();
    let Err(error) = fixture.send(&command(settings)) else {
        panic!("invalid reflection settings must fail decode");
    };
    assert_eq!(error.code, "settings_command_invalid");
    assert!(
        fixture.is_empty(),
        "rejected commands must not emit responses"
    );
    // Zero writes: neither scoped document may exist after rejection.
    assert!(
        fixture.paths.settings().is_ok_and(|path| !path.exists()),
        "global settings must stay unwritten"
    );
    assert!(
        fixture
            .paths
            .project_file(&fixture.workspace, lotta_store::ProjectFile::LocalSettings)
            .is_ok_and(|path| !path.exists()),
        "project-local settings must stay unwritten"
    );
}

#[test]
fn accepts_explicit_merge_settings_and_optional_instructions() {
    accepts(&json!({
        "trigger": "step-count",
        "step_count": 25,
        "merge": "explicit",
        "merge_instructions": "Preserve exact wording.",
    }));
}

#[test]
fn keeps_older_trigger_only_clients_compatible() {
    accepts(&json!({ "trigger": "compaction-event", "step_count": 25 }));
}

#[test]
fn rejects_invalid_merge_modes_without_writes() {
    rejects(&json!({ "trigger": "step-count", "step_count": 25, "merge": "ask" }));
}

#[test]
fn rejects_non_string_merge_instructions_without_writes() {
    rejects(&json!({
        "trigger": "step-count",
        "step_count": 25,
        "merge": "explicit",
        "merge_instructions": 42,
    }));
}

#[test]
fn rejects_out_of_range_step_counts_without_writes() {
    // Zero and negative thresholds are out of range; fractional values fail
    // the integer requirement at decode too.
    rejects(&json!({ "trigger": "step-count", "step_count": 0 }));
    rejects(&json!({ "trigger": "step-count", "step_count": -5 }));
    rejects(&json!({ "trigger": "step-count", "step_count": 2.5 }));
}

#[test]
fn rejects_invalid_trigger_enum_without_writes() {
    rejects(&json!({ "trigger": "sometimes", "step_count": 25 }));
}

#[test]
fn rejects_invalid_scope_enum_without_writes() {
    // The scope enum is validated at command level, like the pinned guard.
    let fixture = bridge();
    let value = json!({
        "type": "set_reflection_settings",
        "request_id": "request-1",
        "runtime": {"agent_id": AGENT_ID, "conversation_id": "default"},
        "settings": {"trigger": "off", "step_count": 25},
        "scope": "everywhere",
    });
    let Err(error) = fixture.send(&value) else {
        panic!("invalid scope must fail decode");
    };
    assert_eq!(error.code, "settings_command_invalid");
    assert!(
        fixture.paths.settings().is_ok_and(|path| !path.exists()),
        "global settings must stay unwritten"
    );
}

#[test]
fn valid_command_persists_after_validation() {
    let fixture = bridge();
    fixture
        .send(&command(&json!({
            "trigger": "step-count",
            "step_count": 25,
            "merge": "explicit",
        })))
        .expect("wellformed command");
    let response = serde_json::to_value(fixture.messages()[0].clone()).expect("encodes");
    assert_eq!(response["type"], "set_reflection_settings_response");
    assert_eq!(response["success"], true);
}

//! Wire-shape round trips for the terminal group against the pinned
//! discriminant fixture.

use super::{
    TerminalCommand, TerminalExitedMessage, TerminalInputCommand, TerminalKillCommand,
    TerminalMessage, TerminalOutputMessage, TerminalResizeCommand, TerminalSpawnedMessage,
};
use serde_json::{Value, json};

const COMMAND_TAGS: [&str; 4] = [
    "terminal_spawn",
    "terminal_input",
    "terminal_resize",
    "terminal_kill",
];
const MESSAGE_TAGS: [&str; 3] = ["terminal_output", "terminal_spawned", "terminal_exited"];

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

#[test]
fn fixture_covers_exactly_this_group() {
    let commands = fixture_discriminants("commands");
    let messages = fixture_discriminants("messages");
    for tag in COMMAND_TAGS {
        assert!(commands.iter().any(|entry| entry == tag));
    }
    for tag in MESSAGE_TAGS {
        assert!(messages.iter().any(|entry| entry == tag));
    }
}

fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(sample: &Value) {
    let parsed: T = serde_json::from_value(sample.clone()).expect("typed");
    assert_eq!(serde_json::to_value(&parsed).expect("wire"), *sample);
}

fn serializes_to<T: serde::Serialize>(value: &T, expected: &Value) {
    assert_eq!(&serde_json::to_value(value).expect("wire"), expected);
}

#[test]
fn terminal_commands_round_trip() {
    round_trip::<TerminalCommand>(&json!({
        "type": "terminal_spawn",
        "terminal_id": "term-1",
        "cols": 120,
        "rows": 40,
        "cwd": "/tmp/work",
    }));
    round_trip::<TerminalCommand>(&json!({
        "type": "terminal_spawn",
        "terminal_id": "term-1",
        "cols": 80,
        "rows": 24,
    }));
    round_trip::<TerminalCommand>(&json!({
        "type": "terminal_input",
        "terminal_id": "term-1",
        "data": "ls\n",
    }));
    round_trip::<TerminalCommand>(&json!({
        "type": "terminal_resize",
        "terminal_id": "term-1",
        "cols": 100,
        "rows": 30,
    }));
    round_trip::<TerminalCommand>(&json!({
        "type": "terminal_kill",
        "terminal_id": "term-1",
    }));
}

#[test]
fn terminal_messages_serialize_with_pinned_names() {
    serializes_to(
        &TerminalMessage::Spawned(TerminalSpawnedMessage {
            terminal_id: "term-1".to_owned(),
            pid: 4242,
        }),
        &json!({
            "type": "terminal_spawned",
            "terminal_id": "term-1",
            "pid": 4242,
        }),
    );
    serializes_to(
        &TerminalMessage::Output(TerminalOutputMessage {
            terminal_id: "term-1".to_owned(),
            data: "hello\n".to_owned(),
        }),
        &json!({
            "type": "terminal_output",
            "terminal_id": "term-1",
            "data": "hello\n",
        }),
    );
    serializes_to(
        &TerminalMessage::Exited(TerminalExitedMessage {
            terminal_id: "term-1".to_owned(),
            exit_code: 0,
            error: None,
        }),
        &json!({
            "type": "terminal_exited",
            "terminal_id": "term-1",
            "exitCode": 0,
        }),
    );
    serializes_to(
        &TerminalMessage::Exited(TerminalExitedMessage {
            terminal_id: "term-1".to_owned(),
            exit_code: 1,
            error: Some("spawn failure".to_owned()),
        }),
        &json!({
            "type": "terminal_exited",
            "terminal_id": "term-1",
            "exitCode": 1,
            "error": "spawn failure",
        }),
    );
}

#[test]
fn decode_routes_every_terminal_discriminant() {
    let spawn = decode_value(&json!({
        "type": "terminal_spawn",
        "terminal_id": "t",
        "cols": 1,
        "rows": 1,
    }));
    assert!(matches!(spawn, Some(TerminalCommand::Spawn(_))));
    let input = decode_value(&json!({
        "type": "terminal_input",
        "terminal_id": "t",
        "data": "x",
    }));
    assert!(matches!(input, Some(TerminalCommand::Input(_))));
    let resize = decode_value(&json!({
        "type": "terminal_resize",
        "terminal_id": "t",
        "cols": 1,
        "rows": 1,
    }));
    assert!(matches!(resize, Some(TerminalCommand::Resize(_))));
    let kill = decode_value(&json!({"type": "terminal_kill", "terminal_id": "t"}));
    assert!(matches!(kill, Some(TerminalCommand::Kill(_))));
}

fn decode_value(value: &Value) -> Option<TerminalCommand> {
    let frame = crate::framing::decode_text(&value.to_string()).expect("bounded frame");
    super::decode(&frame).expect("decode")
}

#[test]
fn oversized_terminal_id_is_rejected_with_correlation() {
    let value = json!({
        "type": "terminal_kill",
        "terminal_id": "x".repeat(super::TERMINAL_ID_BYTES_MAX + 1),
        "request_id": "req-1",
    });
    let frame = crate::framing::decode_text(&value.to_string()).expect("bounded frame");
    let error = super::decode(&frame).expect_err("oversized terminal id");
    assert_eq!(error.request_id.as_deref(), Some("req-1"));
}

#[test]
fn oversized_input_data_is_rejected() {
    let value = json!({
        "type": "terminal_input",
        "terminal_id": "t",
        "data": "x".repeat(super::TERMINAL_INPUT_BYTES_MAX + 1),
    });
    let frame = crate::framing::decode_text(&value.to_string()).expect("bounded frame");
    assert!(super::decode(&frame).is_err());
}

#[test]
fn message_structs_serialize_pinned_fields() {
    let spawned = serde_json::to_value(TerminalMessage::Spawned(TerminalSpawnedMessage {
        terminal_id: "t".to_owned(),
        pid: 7,
    }))
    .expect("wire");
    assert_eq!(spawned["type"], "terminal_spawned");
    let exited = serde_json::to_value(TerminalMessage::Exited(TerminalExitedMessage {
        terminal_id: "t".to_owned(),
        exit_code: 1,
        error: Some("boom".to_owned()),
    }))
    .expect("wire");
    assert_eq!(exited["exitCode"], 1);
    let clean = serde_json::to_value(TerminalMessage::Exited(TerminalExitedMessage {
        terminal_id: "t".to_owned(),
        exit_code: 0,
        error: None,
    }))
    .expect("wire");
    assert!(clean.get("error").is_none());
    let _ = (
        TerminalInputCommand {
            terminal_id: String::new(),
            data: String::new(),
        },
        TerminalResizeCommand {
            terminal_id: String::new(),
            cols: 0,
            rows: 0,
        },
        TerminalKillCommand {
            terminal_id: String::new(),
        },
        TerminalOutputMessage {
            terminal_id: String::new(),
            data: String::new(),
        },
    );
}

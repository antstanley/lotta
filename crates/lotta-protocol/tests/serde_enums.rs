//! Exhaustive tagged-enum serde tests.

use lotta_protocol::{
    ALL_COMMAND_DISCRIMINANTS, ALL_MESSAGE_DISCRIMINANTS, WsProtocolCommand, WsProtocolMessage,
};
use serde_json::{Value, json};

#[test]
fn every_command_round_trips_stable_tag_with_extra_fields() {
    for tag in ALL_COMMAND_DISCRIMINANTS {
        let parsed: WsProtocolCommand = serde_json::from_value(json!({"type":tag,"extra":true}))
            .unwrap_or_else(|error| panic!("command {tag} must decode: {error}"));
        assert_eq!(parsed.discriminant(), *tag);
        assert_eq!(
            serde_json::to_value(parsed).unwrap_or(Value::Null),
            json!({"type":tag})
        );
        assert_eq!(WsProtocolCommand::from_discriminant(tag), Some(parsed));
    }
}

#[test]
fn every_message_round_trips_stable_tag_with_extra_fields() {
    for tag in ALL_MESSAGE_DISCRIMINANTS {
        let parsed: WsProtocolMessage = serde_json::from_value(json!({"type":tag,"extra":true}))
            .unwrap_or_else(|error| panic!("message {tag} must decode: {error}"));
        assert_eq!(parsed.discriminant(), *tag);
        assert_eq!(
            serde_json::to_value(parsed).unwrap_or(Value::Null),
            json!({"type":tag})
        );
        assert_eq!(WsProtocolMessage::from_discriminant(tag), Some(parsed));
    }
}

#[test]
fn protocol_version_is_pinned() {
    assert_eq!(lotta_protocol::APP_SERVER_PROTOCOL_VERSION, 1);
}

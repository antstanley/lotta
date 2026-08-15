//! Protocol decode-policy tests.

use lotta_protocol::{DecodeOutcome, decode_text};
use serde_json::json;

fn input(payload: &serde_json::Value) -> String {
    json!({"type":"input","runtime":{"agent_id":"agent","conversation_id":"conv"},"payload":payload}).to_string()
}

mod decode {
    use super::*;

    #[test]
    fn unknown_type_is_dropped_silently() {
        let effects = decode_text(r#"{"type":"not_a_command"}"#);
        assert_eq!(effects.outcome, DecodeOutcome::DroppedUnknown);
        assert_eq!(effects.state_mutations, 0);
        assert!(effects.outbound.is_empty());
    }

    #[test]
    fn extra_fields_tolerated() {
        let effects = decode_text(r#"{"type":"sync","extra":{"nested":true}}"#);
        assert!(matches!(effects.outcome, DecodeOutcome::Accepted(_)));
        let effects = decode_text(&input(
            &json!({"kind":"create_message","messages":[],"extra":1}),
        ));
        assert!(matches!(effects.outcome, DecodeOutcome::Accepted(_)));
    }

    #[test]
    fn bad_enum_value_rejected() {
        let effects = decode_text(&input(
            &json!({"kind":"create_message","messages":[],"image_failure_mode":"best_effort"}),
        ));
        assert_eq!(effects.outcome, DecodeOutcome::RecoverableInput);
        assert_eq!(
            effects.outbound[0].reason,
            "Protocol violation: input.payload.image_failure_mode must be strict or drop"
        );
    }

    #[test]
    fn malformed_input_is_non_terminal() {
        let malformed = decode_text(&input(&json!({"kind":"create_message"})));
        let notice = &malformed.outbound[0];
        assert_eq!(notice.discriminator, "stream_delta");
        assert_eq!(notice.message_type, "loop_error");
        assert_eq!(
            notice.reason,
            "Protocol violation: input.kind=create_message requires payload.messages[]"
        );
        assert_eq!(notice.stop_reason, "error");
        assert!(!notice.is_terminal);
        assert!(notice.admits_further_input);
        assert_eq!(notice.state_mutations, 0);
        let later = decode_text(&input(&json!({"kind":"create_message","messages":[]})));
        assert!(matches!(later.outcome, DecodeOutcome::Accepted(_)));
    }

    #[test]
    fn strict_input_shapes_match_baseline_reasons() {
        let cases = [
            (
                json!({"kind":"create_message","messages":[],"client_tool_allowlist":[1]}),
                "Protocol violation: input.payload.client_tool_allowlist must be string[]",
            ),
            (
                json!({"kind":"create_message","messages":[],"client_toolset":{"base":"bad"}}),
                "Protocol violation: input.payload.client_toolset must contain an optional valid base and string[] include",
            ),
            (
                json!({"kind":"create_message","messages":[],"exclude_interactive_tools":"yes"}),
                "Protocol violation: input.payload.exclude_interactive_tools must be boolean",
            ),
            (
                json!({"kind":"approval_response","request_id":"r","decision":{"behavior":"maybe"}}),
                "Protocol violation: input.kind=approval_response requires payload.request_id and either payload.decision or payload.error",
            ),
            (
                json!({"kind":"teleport_continue","teleport_id":"","source":{}}),
                "Protocol violation: input.kind=teleport_continue requires teleport_id, source, and optional continuation.approvals[]",
            ),
            (
                json!({"kind":"future"}),
                "Unsupported input payload kind: future",
            ),
        ];
        for (payload, reason) in cases {
            assert_eq!(decode_text(&input(&payload)).outbound[0].reason, reason);
        }
    }

    #[test]
    fn invalid_runtime_and_request_id_drop_silently() {
        let invalid_runtime = r#"{"type":"input","runtime":{"agent_id":"","conversation_id":"c"},"payload":{"kind":"create_message","messages":[]}}"#;
        assert_eq!(
            decode_text(invalid_runtime).outcome,
            DecodeOutcome::SilentDrop
        );
        let invalid_request = r#"{"type":"input","request_id":"","runtime":{"agent_id":"a","conversation_id":"c"},"payload":{"kind":"create_message","messages":[]}}"#;
        assert_eq!(
            decode_text(invalid_request).outcome,
            DecodeOutcome::SilentDrop
        );
    }
}

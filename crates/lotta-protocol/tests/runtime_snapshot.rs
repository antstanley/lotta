//! Runtime projection regression tests.

use lotta_domain::{AgentId, ConversationId, RuntimeScope, TurnStateKind};
use serde::Serialize;
use serde_json::json;

#[derive(Serialize)]
struct ProtocolOwnedRuntimeProjection {
    runtime: RuntimeScope,
    turn_state: TurnStateKind,
}

mod runtime_snapshot {
    use super::*;

    #[test]
    fn task04_projection_remains_serializable_and_nestable() {
        let agent = AgentId::accept("agent").unwrap_or_else(|error| panic!("fixture id: {error}"));
        let conversation =
            ConversationId::accept("conv").unwrap_or_else(|error| panic!("fixture id: {error}"));
        let projection = ProtocolOwnedRuntimeProjection {
            runtime: RuntimeScope::new(agent, conversation, None),
            turn_state: TurnStateKind::Idle,
        };
        let value = serde_json::to_value(projection)
            .unwrap_or_else(|error| panic!("projection must serialize: {error}"));
        assert_eq!(
            value,
            json!({"runtime":{"agent_id":"agent","conversation_id":"conv"},"turn_state":"idle"})
        );
        assert!(value.get("type").is_none());
    }
}

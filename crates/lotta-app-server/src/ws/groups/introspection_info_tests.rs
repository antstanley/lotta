//! `ws::introspection::info` — `app_server_info` reports protocol version 1
//! with the pinned capability shape, and rejects unauthenticated requests.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{APP_SERVER_PROTOCOL_VERSION, AppServerInfoCommand, IntrospectionBridge};
use crate::{framing, ws::ConnectionId};

const CONNECTION: ConnectionId = 101;

type Captured = Arc<Mutex<Vec<(ConnectionId, super::AppServerInfoResponseMessage)>>>;

fn bridge() -> (IntrospectionBridge, Captured) {
    let captured: Captured = Arc::default();
    let sink = Arc::clone(&captured);
    let forward = Arc::new(move |connection, message| {
        sink.lock()
            .expect("capture lock")
            .push((connection, message));
        Ok(())
    });
    (IntrospectionBridge::new(forward), captured)
}

#[test]
fn reports_protocol_version_1() {
    // Pinned comparison anchor: letta-code/src/types/app-server-info.ts:1
    // declares APP_SERVER_PROTOCOL_VERSION = 1.
    assert_eq!(APP_SERVER_PROTOCOL_VERSION, 1);

    let (bridge, captured) = bridge();
    bridge.register_authenticated(CONNECTION);
    let served = bridge.handle(
        CONNECTION,
        &AppServerInfoCommand {
            request_id: "ai-1".to_owned(),
        },
    );
    assert!(served, "authenticated discovery is served");

    let messages = captured.lock().expect("capture lock");
    assert_eq!(messages.len(), 1);
    let (owner, message) = &messages[0];
    assert_eq!(*owner, CONNECTION);
    let encoded = serde_json::to_value(message).expect("encodes");
    assert_eq!(encoded["type"], "app_server_info_response", "pinned tag");
    assert_eq!(encoded["request_id"], "ai-1");
    assert_eq!(encoded["success"], true);
    assert_eq!(
        encoded["protocol_version"], 1,
        "wire value compared by clients against their supported version"
    );
    assert_eq!(encoded["backend"], "local");
    assert_eq!(encoded["letta_code_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        encoded["capabilities"],
        json!({
            "agent_management": true,
            "conversation_management": true,
            "memory_management": true,
            "runtime_start": true,
            "runtime_workspace_sandbox": false,
            "runtime_external_tools_update": true,
            "split_channels": false,
        }),
        "capability flag set audited against actual implemented behavior: the \
         production runtime_start path ignores client workspace-sandbox extensions"
    );
}

#[test]
fn requires_authentication() {
    let (bridge, captured) = bridge();

    let rejected = bridge.handle(
        999_999,
        &AppServerInfoCommand {
            request_id: "ai-2".to_owned(),
        },
    );
    assert!(!rejected, "unregistered connections are unauthenticated");
    assert!(
        captured.lock().expect("capture lock").is_empty(),
        "unauthenticated requests are rejected without any response"
    );

    // A closed connection loses eligibility again.
    bridge.register_authenticated(CONNECTION);
    bridge.unregister(CONNECTION);
    let closed = bridge.handle(
        CONNECTION,
        &AppServerInfoCommand {
            request_id: "ai-3".to_owned(),
        },
    );
    assert!(!closed, "unregistered connections cannot discover");
    assert!(captured.lock().expect("capture lock").is_empty());
}

#[test]
fn decode_routes_only_app_server_info() {
    let command = json!({
        "type": "app_server_info",
        "request_id": "ai-dec",
    });
    let frame = framing::decode_text(&command.to_string()).expect("bounded frame");
    let decoded = super::decode(&frame)
        .expect("wellformed command")
        .expect("introspection routed");
    assert_eq!(decoded.request_id, "ai-dec");

    let other = json!({
        "type": "input",
        "request_id": "not-introspection",
        "payload": null,
    });
    let frame = framing::decode_text(&other.to_string()).expect("bounded frame");
    assert!(
        super::decode(&frame)
            .expect("other groups ignored")
            .is_none(),
        "non-introspection frames fall through the chain"
    );
}

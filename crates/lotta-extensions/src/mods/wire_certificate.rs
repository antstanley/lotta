use super::protocol::*;
use super::registrations::RegistrationBatch;
use super::types::*;
use serde_json::json;

fn owner(generation: u64) -> ModOwner {
    ModOwner {
        id: ModId::new("project:wire.ts".into()).unwrap(),
        generation: Generation(generation),
    }
}
fn id() -> RpcId {
    RpcId::new(7).unwrap()
}

#[test]
fn pinned_initialize_golden_wire() {
    let request = RpcRequest::new(
        id(),
        RpcMethod::Initialize,
        RpcParams::Initialize {
            owner: owner(1),
            capabilities: vec![Capability::Tools],
            conversation_handle: ConversationHandle::new("opaque".into()).unwrap(),
        },
    );
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "jsonrpc":"2.0","id":7,"method":"initialize","params":{"type":"initialize",
            "owner":{"id":"project:wire.ts","generation":1},"capabilities":["tools"],
            "conversation_handle":"opaque"}
        })
    );
}
#[test]
fn malformed_jsonrpc_is_rejected() {
    assert!(
        serde_json::from_value::<RpcRequest>(json!({"jsonrpc":"2.0","id":7,
        "method":"initialize","params":null}))
        .is_err()
    );
}
#[test]
fn zero_id_is_rejected() {
    assert_eq!(RpcId::new(0), Err(ModError::Protocol));
}
#[test]
fn version_is_rejected() {
    let request = RpcRequest {
        jsonrpc: "1.0".into(),
        id: id(),
        method: RpcMethod::Dispose,
        params: RpcParams::Dispose { owner: owner(1) },
    };
    assert_eq!(request.validate(), Err(ModError::Protocol));
}
#[test]
fn method_param_mismatch_is_rejected() {
    let request = RpcRequest {
        jsonrpc: JSON_RPC_VERSION.into(),
        id: id(),
        method: RpcMethod::Reload,
        params: RpcParams::Dispose { owner: owner(1) },
    };
    assert_eq!(request.validate(), Err(ModError::Protocol));
}
#[test]
fn unknown_fields_are_rejected() {
    assert!(
        serde_json::from_value::<RpcResponse>(json!({"jsonrpc":"2.0","id":7,
        "result":{"type":"disposed"},"extra":true}))
        .is_err()
    );
}
#[test]
fn generation_is_present_on_registration_wire() {
    let params = RpcParams::Register {
        owner: owner(9),
        registrations: RegistrationBatch::default(),
    };
    let value = serde_json::to_value(params).unwrap();
    assert_eq!(value.pointer("/owner/generation"), Some(&json!(9)));
}
#[test]
fn owner_is_present_on_every_call_wire() {
    let params = RpcParams::ToolCall {
        owner: owner(2),
        name: "tool".into(),
        tool_call_id: "call".into(),
        input: json!({}),
    };
    assert_eq!(
        serde_json::to_value(params).unwrap().pointer("/owner/id"),
        Some(&json!("project:wire.ts"))
    );
}
#[test]
fn bounded_error_message_is_enforced() {
    let response = RpcResponse {
        jsonrpc: JSON_RPC_VERSION.into(),
        id: id(),
        result: None,
        error: Some(RpcError {
            code: -32000,
            message: "x".repeat(1_025),
        }),
    };
    assert_eq!(response.validate(), Err(ModError::Protocol));
}
#[test]
fn correlation_id_must_be_positive_and_exact_type() {
    assert!(
        serde_json::from_value::<RpcResponse>(json!({"jsonrpc":"2.0","id":"7",
        "result":{"type":"disposed"}}))
        .is_err()
    );
}

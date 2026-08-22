//! Credential redaction across every recorded models/providers response.

use super::support::{
    AGENT_ID, CONNECTION_A, CONVERSATION_ID, SECRET_MARKER, assert_marker_absent,
    bridge_with_openai, encoded, entry_with_handle,
};
use serde_json::{Value, json};

#[tokio::test]
async fn secret_marker_absent_from_all_command_responses() {
    // The planted marker is persisted through the Task 52 store before any
    // command runs, so every response must scrub it end to end.
    let models = bridge_with_openai(&planted_key("openai")).await;
    let sweep = sweep_commands();
    for command in &sweep {
        models.send(CONNECTION_A, command).await;
    }
    let recorded = models.messages_for(CONNECTION_A);
    assert_eq!(recorded.len(), sweep.len());
    assert_marker_absent(&recorded, SECRET_MARKER);
    // Sanity: the exercised commands really reached their expected paths.
    let connect = encoded(&recorded[2]);
    assert_eq!(connect["type"], "connect_provider_response");
    assert_eq!(
        connect["success"], true,
        "a stamped connect with a planted key must succeed"
    );
    let usage = encoded(&recorded[3]);
    assert_eq!(usage["type"], "chatgpt_usage_read_response");
    assert_eq!(usage["error"]["code"], "not_connected");
    let update = encoded(&recorded[4]);
    assert_eq!(update["type"], "update_model_response");
    assert_eq!(update["success"], true);
    let disconnect = encoded(&recorded[6]);
    assert_eq!(disconnect["type"], "disconnect_provider_response");
    assert_eq!(disconnect["success"], true);
}

#[tokio::test]
async fn list_models_reports_readiness_per_entry() {
    let models = bridge_with_openai("sk-openai-ready").await;
    models
        .send(
            CONNECTION_A,
            &json!({ "type": "list_models", "request_id": "ready-1" }),
        )
        .await;
    let responses = models.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    let value = encoded(&responses[0]);
    assert_eq!(value["request_id"], "ready-1");
    assert_eq!(value["success"], true);
    let entries = value["entries"].as_array().expect("catalog entries");
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert!(entry["readiness"].is_string(), "readiness missing");
        assert!(entry["handle"].is_string(), "handle missing");
        assert!(entry["model_settings"].is_object(), "settings missing");
    }
    let connected = entry_with_handle(entries, "openai/gpt-test");
    assert_eq!(connected["readiness"], "ready");
    let disconnected = entry_with_handle(entries, "anthropic/claude-test");
    assert_eq!(disconnected["readiness"], "disconnected");
    assert_eq!(value["available_handles"], json!(["openai/gpt-test"]));
    assert_eq!(
        value["byok_provider_aliases"],
        json!({ "openai": "openai" })
    );
}

fn planted_key(row: &str) -> String {
    format!("{SECRET_MARKER}-{row}")
}

fn sweep_commands() -> Vec<Value> {
    vec![
        json!({ "type": "list_models", "request_id": "redact-lm" }),
        json!({
            "type": "list_connect_providers",
            "request_id": "redact-lcp",
            "target": "local",
        }),
        json!({
            "type": "connect_provider",
            "request_id": "redact-cp",
            "target": "local",
            "provider_id": "anthropic",
            "fields": { "apiKey": planted_key("anthropic") },
        }),
        json!({
            "type": "chatgpt_usage_read",
            "request_id": "redact-cu",
            "target": "local",
        }),
        json!({
            "type": "update_model",
            "request_id": "redact-um",
            "runtime": { "agent_id": AGENT_ID, "conversation_id": CONVERSATION_ID },
            "payload": { "model_id": "openai/gpt-test", "reasoning_effort": "high" },
        }),
        json!({
            "type": "update_toolset",
            "request_id": "redact-ut",
            "runtime": { "agent_id": AGENT_ID, "conversation_id": CONVERSATION_ID },
            "toolset_preference": "auto",
        }),
        json!({
            "type": "disconnect_provider",
            "request_id": "redact-dp",
            "target": "local",
            "provider_id": "openai",
        }),
    ]
}

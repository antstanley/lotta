//! `ws::models::fixture_round_trip` certificate selectors.

use super::{ModelsCommand, ModelsMessage};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const COMMAND_TAGS: [&str; 7] = [
    "list_models",
    "list_connect_providers",
    "connect_provider",
    "disconnect_provider",
    "chatgpt_usage_read",
    "update_model",
    "update_toolset",
];
const MESSAGE_TAGS: [&str; 7] = [
    "list_models_response",
    "list_connect_providers_response",
    "connect_provider_response",
    "disconnect_provider_response",
    "chatgpt_usage_read_response",
    "update_model_response",
    "update_toolset_response",
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

fn round_trip<T: DeserializeOwned + serde::Serialize>(sample: &Value) {
    let parsed: T = serde_json::from_value(sample.clone()).expect("typed");
    assert_eq!(serde_json::to_value(&parsed).expect("wire"), *sample);
}

#[test]
fn covers_exactly_this_group() {
    let commands = fixture_discriminants("commands");
    let messages = fixture_discriminants("messages");
    for tag in COMMAND_TAGS {
        assert!(commands.iter().any(|entry| entry == tag));
    }
    for tag in MESSAGE_TAGS {
        assert!(messages.iter().any(|entry| entry == tag));
    }
    assert_eq!(
        commands
            .iter()
            .filter(|entry| COMMAND_TAGS.contains(&entry.as_str()))
            .count(),
        COMMAND_TAGS.len()
    );
    assert_eq!(
        messages
            .iter()
            .filter(|entry| MESSAGE_TAGS.contains(&entry.as_str()))
            .count(),
        MESSAGE_TAGS.len()
    );
}

#[test]
fn list_models_round_trip() {
    round_trip::<ModelsCommand>(&json!({
        "type": "list_models",
        "request_id": "rt-lm",
    }));
    round_trip::<ModelsCommand>(&json!({
        "type": "list_models",
        "request_id": "rt-lmf",
        "force": true,
    }));
}

#[test]
fn list_connect_providers_round_trip() {
    round_trip::<ModelsCommand>(&json!({
        "type": "list_connect_providers",
        "request_id": "rt-lcp",
        "target": "local",
    }));
}

#[test]
fn connect_provider_round_trip() {
    round_trip::<ModelsCommand>(&json!({
        "type": "connect_provider",
        "request_id": "rt-cp",
        "target": "local",
        "provider_id": "openai",
        "fields": { "apiKey": "sk-openai-rt" },
    }));
    round_trip::<ModelsCommand>(&json!({
        "type": "connect_provider",
        "request_id": "rt-cpb",
        "target": "local",
        "provider_id": "bedrock",
        "auth_method_id": "iam",
        "fields": {
            "accessKey": "AKIA-RT",
            "apiKey": "aws-secret-rt",
            "region": "us-east-1",
        },
    }));
}

#[test]
fn disconnect_provider_round_trip() {
    round_trip::<ModelsCommand>(&json!({
        "type": "disconnect_provider",
        "request_id": "rt-dp",
        "target": "local",
        "provider_id": "openai",
    }));
    round_trip::<ModelsCommand>(&json!({
        "type": "disconnect_provider",
        "request_id": "rt-dpf",
        "target": "local",
        "provider_id": "openai",
        "provider_name": "openai",
        "force": true,
    }));
}

#[test]
fn chatgpt_usage_read_round_trip() {
    round_trip::<ModelsCommand>(&json!({
        "type": "chatgpt_usage_read",
        "request_id": "rt-cu",
        "target": "local",
    }));
    round_trip::<ModelsCommand>(&json!({
        "type": "chatgpt_usage_read",
        "request_id": "rt-cuf",
        "target": "api",
        "provider_name": "chatgpt-plus-pro",
        "force_refresh": true,
    }));
}

#[test]
fn update_model_round_trip() {
    round_trip::<ModelsCommand>(&json!({
        "type": "update_model",
        "request_id": "rt-um",
        "runtime": { "agent_id": "agent-1", "conversation_id": "default" },
        "payload": { "model_id": "openai/gpt-test", "reasoning_effort": "high" },
    }));
    round_trip::<ModelsCommand>(&json!({
        "type": "update_model",
        "request_id": "rt-umh",
        "runtime": {
            "agent_id": "agent-1",
            "conversation_id": "conv-1",
            "acting_user_id": "user-1",
        },
        "payload": { "model_handle": "anthropic/claude-test" },
    }));
}

#[test]
fn update_toolset_round_trip() {
    round_trip::<ModelsCommand>(&json!({
        "type": "update_toolset",
        "request_id": "rt-ut",
        "runtime": { "agent_id": "agent-1", "conversation_id": "default" },
        "toolset_preference": "auto",
    }));
    round_trip::<ModelsCommand>(&json!({
        "type": "update_toolset",
        "request_id": "rt-utg",
        "runtime": {
            "agent_id": "agent-1",
            "conversation_id": "conv-1",
            "acting_user_id": "user-1",
        },
        "toolset_preference": "gemini_snake",
    }));
}

#[test]
fn list_models_response_round_trip() {
    round_trip::<ModelsMessage>(&json!({
        "type": "list_models_response",
        "request_id": "rt-lm",
        "success": true,
        "entries": [{
            "id": "openai/gpt-test",
            "handle": "openai/gpt-test",
            "label": "openai/gpt-test",
            "description": "",
            "readiness": "ready",
            "model_settings": {},
        }],
        "available_handles": ["openai/gpt-test"],
        "byok_provider_aliases": { "openai": "openai" },
    }));
    round_trip::<ModelsMessage>(&json!({
        "type": "list_models_response",
        "request_id": "rt-lme",
        "success": false,
        "entries": [],
        "available_handles": [],
        "byok_provider_aliases": {},
        "error": "catalog unavailable",
    }));
}

#[test]
fn list_connect_providers_response_round_trip() {
    round_trip::<ModelsMessage>(&json!({
        "type": "list_connect_providers_response",
        "request_id": "rt-lcp",
        "success": true,
        "target": "local",
        "providers": [provider_row()],
    }));
    round_trip::<ModelsMessage>(&json!({
        "type": "list_connect_providers_response",
        "request_id": "rt-lcpe",
        "success": false,
        "target": "local",
        "providers": [],
        "error": "store unreadable",
    }));
}

#[test]
fn connect_provider_response_round_trip() {
    let mut row = provider_row();
    row["connected"] = json!({
        "is_connected": true,
        "id": "local-provider-openai",
        "provider_name": "openai",
        "provider_type": "openai",
        "auth_type": "api",
        "base_url": "https://api.openai.com/v1",
        "timeout": false,
        "region": "us-east-1",
    });
    round_trip::<ModelsMessage>(&json!({
        "type": "connect_provider_response",
        "request_id": "rt-cp",
        "success": true,
        "target": "local",
        "providers": [row],
        "models_may_have_changed": true,
    }));
    round_trip::<ModelsMessage>(&json!({
        "type": "connect_provider_response",
        "request_id": "rt-cpe",
        "success": false,
        "target": "local",
        "providers": [],
        "models_may_have_changed": false,
        "error": "Missing OpenAI API key.",
    }));
}

#[test]
fn disconnect_provider_response_round_trip() {
    round_trip::<ModelsMessage>(&json!({
        "type": "disconnect_provider_response",
        "request_id": "rt-dp",
        "success": true,
        "target": "local",
        "providers": [provider_row()],
        "models_may_have_changed": true,
    }));
    round_trip::<ModelsMessage>(&json!({
        "type": "disconnect_provider_response",
        "request_id": "rt-dpe",
        "success": false,
        "target": "local",
        "providers": [],
        "models_may_have_changed": false,
        "error": "provider connection not found",
    }));
}

#[test]
fn chatgpt_usage_read_response_round_trip() {
    round_trip::<ModelsMessage>(&json!({
        "type": "chatgpt_usage_read_response",
        "request_id": "rt-cu",
        "success": true,
        "target": "local",
        "usage": {
            "providerName": "chatgpt-plus-pro",
            "fetchedAt": "2026-01-02T03:04:05Z",
            "summary": "5h 12% used",
            "planType": "pro",
            "limitReached": false,
            "primary": usage_window("five_hours", 12.5, 300, 1000),
            "secondary": usage_window("weekly_all", 40.0, 10080, 2000),
            "additional": [],
        },
    }));
    round_trip::<ModelsMessage>(&json!({
        "type": "chatgpt_usage_read_response",
        "request_id": "rt-cue",
        "success": false,
        "target": "local",
        "error": {
            "code": "network_error",
            "message": "Failed to read ChatGPT usage.",
        },
    }));
}

#[test]
fn update_model_response_round_trip() {
    round_trip::<ModelsMessage>(&json!({
        "type": "update_model_response",
        "request_id": "rt-um",
        "success": true,
        "runtime": { "agent_id": "agent-1", "conversation_id": "default" },
        "applied_to": "agent",
        "model_id": "openai/gpt-test",
        "model_handle": "openai/gpt-test",
        "model_settings": { "reasoning_effort": "high" },
    }));
    round_trip::<ModelsMessage>(&json!({
        "type": "update_model_response",
        "request_id": "rt-ume",
        "success": false,
        "error": "model unavailable",
    }));
}

#[test]
fn update_toolset_response_round_trip() {
    round_trip::<ModelsMessage>(&json!({
        "type": "update_toolset_response",
        "request_id": "rt-ut",
        "success": true,
        "runtime": {
            "agent_id": "agent-1",
            "conversation_id": "conv-1",
            "acting_user_id": "user-1",
        },
        "current_toolset": "gemini_snake",
        "current_toolset_preference": "gemini_snake",
    }));
    round_trip::<ModelsMessage>(&json!({
        "type": "update_toolset_response",
        "request_id": "rt-ute",
        "success": false,
        "runtime": { "agent_id": "agent-1", "conversation_id": "conv-1" },
        "error": "unknown toolset preference",
    }));
}

fn provider_row() -> Value {
    json!({
        "id": "openai",
        "display_name": "OpenAI",
        "description": "Connect an OpenAI API key",
        "provider_type": "openai",
        "provider_name": "openai",
        "provider_names": ["openai"],
        "requires_api_key": true,
        "fields": [{
            "key": "apiKey",
            "label": "API Key",
            "secret": true,
            "required": true,
        }],
        "connected": { "is_connected": false },
        "connected_providers": [],
    })
}

fn usage_window(label: &str, used_percent: f64, minutes: u64, resets_at: u64) -> Value {
    json!({
        "label": label,
        "usedPercent": used_percent,
        "windowDurationMins": minutes,
        "resetsAt": resets_at,
    })
}

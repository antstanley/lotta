//! One decode-route-respond case per models/providers command in the pinned fixture.

use super::support::{
    CONNECTION_A, assert_in_fixture, bridge, bridge_with_chatgpt, bridge_with_openai,
    discriminant_of, encoded, entry_with_handle, find_row, scope_json,
};
use serde_json::json;

#[tokio::test]
async fn list_models_decodes_routes_and_responds() {
    assert_in_fixture("commands", "list_models");
    let models = bridge().await;
    models
        .send(
            CONNECTION_A,
            &json!({ "type": "list_models", "request_id": "lm1", "force": true }),
        )
        .await;
    let responses = models.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    assert_in_fixture("messages", discriminant_of(&responses[0]));
    let value = encoded(&responses[0]);
    assert_eq!(value["type"], "list_models_response");
    assert_eq!(value["request_id"], "lm1");
    assert_eq!(value["success"], true);
    let entries = value["entries"].as_array().expect("catalog entries");
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert_eq!(entry["readiness"], "disconnected");
        assert_eq!(entry["description"], "");
    }
    assert_eq!(value["available_handles"], json!([]));
    assert_eq!(value["byok_provider_aliases"], json!({}));
    // With a live connection the provider's entries project as ready.
    let connected = bridge_with_openai("sk-openai-lm").await;
    connected
        .send(
            CONNECTION_A,
            &json!({ "type": "list_models", "request_id": "lm2" }),
        )
        .await;
    let value = encoded(connected.messages_for(CONNECTION_A).last().expect("second"));
    let entries = value["entries"].as_array().expect("catalog entries");
    let gpt = entry_with_handle(entries, "openai/gpt-test");
    assert_eq!(gpt["readiness"], "ready");
    let claude = entry_with_handle(entries, "anthropic/claude-test");
    assert_eq!(claude["readiness"], "disconnected");
    assert_eq!(value["available_handles"], json!(["openai/gpt-test"]));
}

#[tokio::test]
async fn list_connect_providers_decodes_routes_and_responds() {
    assert_in_fixture("commands", "list_connect_providers");
    let models = bridge().await;
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "list_connect_providers",
                "request_id": "lp1",
                "target": "local",
            }),
        )
        .await;
    let responses = models.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    assert_in_fixture("messages", discriminant_of(&responses[0]));
    let value = encoded(&responses[0]);
    assert_eq!(value["type"], "list_connect_providers_response");
    assert_eq!(value["request_id"], "lp1");
    assert_eq!(value["success"], true);
    assert_eq!(value["target"], "local");
    let rows = value["providers"].as_array().expect("provider rows");
    let openai = find_row(rows, "openai");
    assert_eq!(openai["display_name"], "OpenAI");
    assert_eq!(openai["provider_type"], "openai");
    assert_eq!(openai["requires_api_key"], true);
    assert_eq!(openai["fields"][0]["key"], "apiKey");
    assert_eq!(openai["fields"][0]["secret"], true);
    assert_eq!(openai["connected"]["is_connected"], false);
    assert_eq!(openai["connected_providers"], json!([]));
    let bedrock = find_row(rows, "bedrock");
    assert_eq!(bedrock["auth_methods"][0]["id"], "iam");
    assert_eq!(bedrock["auth_methods"][1]["id"], "profile");
    let connected = bridge_with_openai("sk-openai-lc").await;
    connected
        .send(
            CONNECTION_A,
            &json!({
                "type": "list_connect_providers",
                "request_id": "lp2",
                "target": "local",
            }),
        )
        .await;
    let value = encoded(connected.messages_for(CONNECTION_A).last().expect("second"));
    let rows = value["providers"].as_array().expect("provider rows");
    let openai = find_row(rows, "openai");
    assert_eq!(openai["connected"]["is_connected"], true);
    assert_eq!(openai["connected"]["id"], "local-provider-openai");
    assert_eq!(openai["connected"]["provider_type"], "openai");
    assert_eq!(openai["connected"]["auth_type"], "api");
    let states = openai["connected_providers"].as_array().expect("states");
    assert_eq!(states.len(), 1);
}

#[tokio::test]
async fn connect_provider_decodes_routes_and_responds() {
    assert_in_fixture("commands", "connect_provider");
    let models = bridge().await;
    send_connect(&models, "cp0", "nope-row", None).await;
    let unknown = encoded(
        models
            .messages_for(CONNECTION_A)
            .last()
            .expect("unknown row"),
    );
    assert_eq!(unknown["type"], "connect_provider_response");
    assert_in_fixture("messages", "connect_provider_response");
    assert_eq!(unknown["request_id"], "cp0");
    assert_eq!(unknown["success"], false);
    assert_eq!(unknown["error"], "Unknown provider: nope-row");
    assert_eq!(unknown["providers"], json!([]));
    assert_eq!(unknown["models_may_have_changed"], false);
    send_connect(&models, "cp1", "openai", None).await;
    let missing = encoded(
        models
            .messages_for(CONNECTION_A)
            .last()
            .expect("missing key"),
    );
    assert_eq!(missing["request_id"], "cp1");
    assert_eq!(missing["success"], false);
    assert_eq!(missing["error"], "Missing API Key.");
    // Rejected before any adapter validation runs.
    assert!(models.adapter.validations.lock().expect("lock").is_empty());
    // A stamped connect now passes Task 52 validation, reaches the adapter,
    // and answers success with a refreshed provider snapshot.
    send_connect(&models, "cp2", "openai", Some("sk-openai-connect")).await;
    let accepted = encoded(models.messages_for(CONNECTION_A).last().expect("accepted"));
    assert_eq!(accepted["request_id"], "cp2");
    assert_eq!(accepted["success"], true);
    assert_eq!(accepted["error"], serde_json::Value::Null);
    assert_eq!(
        accepted["models_may_have_changed"], true,
        "a successful connect must refresh model readiness"
    );
    let providers = accepted["providers"].as_array().expect("providers");
    assert!(
        providers.iter().any(|provider| provider["id"] == "openai"),
        "openai must appear connected after a stamped connect"
    );
    assert_eq!(
        models.adapter.validations.lock().expect("lock").len(),
        1,
        "the adapter must observe exactly one validated input"
    );
}

/// Sends one `connect_provider` wire command with an optional API key.
async fn send_connect(
    models: &super::support::TestModels,
    request_id: &str,
    provider_id: &str,
    api_key: Option<&str>,
) {
    let mut body = json!({
        "type": "connect_provider",
        "request_id": request_id,
        "target": "local",
        "provider_id": provider_id,
        "fields": {},
    });
    if let Some(key) = api_key {
        body["fields"] = json!({ "apiKey": key });
    }
    models.send(CONNECTION_A, &body).await;
}

#[tokio::test]
async fn disconnect_provider_decodes_routes_and_responds() {
    assert_in_fixture("commands", "disconnect_provider");
    let models = bridge_with_openai("sk-openai-dc").await;
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "disconnect_provider",
                "request_id": "dp1",
                "target": "local",
                "provider_id": "openai",
            }),
        )
        .await;
    let responses = models.messages_for(CONNECTION_A);
    assert_eq!(responses.len(), 1);
    assert_in_fixture("messages", discriminant_of(&responses[0]));
    let removed = encoded(&responses[0]);
    assert_eq!(removed["type"], "disconnect_provider_response");
    assert_eq!(removed["request_id"], "dp1");
    assert_eq!(removed["success"], true);
    assert_eq!(removed["models_may_have_changed"], true);
    let rows = removed["providers"].as_array().expect("provider rows");
    let openai = find_row(rows, "openai");
    assert_eq!(openai["connected"]["is_connected"], false);
    assert_eq!(openai["connected_providers"], json!([]));
    // Disconnecting an absent record is a typed failure.
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "disconnect_provider",
                "request_id": "dp2",
                "target": "local",
                "provider_id": "openai",
            }),
        )
        .await;
    let absent = encoded(models.messages_for(CONNECTION_A).last().expect("absent"));
    assert_eq!(absent["request_id"], "dp2");
    assert_eq!(absent["success"], false);
    assert_eq!(absent["error"], "provider connection not found");
    assert_eq!(absent["providers"], json!([]));
    assert_eq!(absent["models_may_have_changed"], false);
}

#[tokio::test]
async fn chatgpt_usage_read_decodes_routes_and_responds() {
    assert_in_fixture("commands", "chatgpt_usage_read");
    let models = bridge_with_chatgpt().await;
    read_usage(&models, "cu1", "local", None).await;
    let responses = models.messages_for(CONNECTION_A);
    let value = encoded(&responses[0]);
    assert_eq!(value["type"], "chatgpt_usage_read_response");
    assert_in_fixture("messages", "chatgpt_usage_read_response");
    assert_eq!(value["request_id"], "cu1");
    assert_eq!(value["success"], true);
    assert_eq!(value["target"], "local");
    assert_eq!(value["usage"]["providerName"], "chatgpt-plus-pro");
    assert_eq!(value["usage"]["summary"], "5h 12% used");
    assert_eq!(value["usage"]["planType"], "pro");
    assert_eq!(value["usage"]["primary"]["usedPercent"], 12.5);
    assert_eq!(value["usage"]["additional"], json!([]));
    {
        let calls = models.usage.calls.lock().expect("usage calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], ("chatgpt-plus-pro".to_owned(), false));
    }
    read_usage(&models, "cu2", "local", Some(true)).await;
    {
        let calls = models.usage.calls.lock().expect("usage calls");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1], ("chatgpt-plus-pro".to_owned(), true));
    }
    // The api target is a typed unsupported failure without a reader call.
    read_usage(&models, "cu3", "api", None).await;
    let unsupported = encoded(
        models
            .messages_for(CONNECTION_A)
            .last()
            .expect("unsupported"),
    );
    assert_eq!(unsupported["request_id"], "cu3");
    assert_eq!(unsupported["success"], false);
    assert_eq!(unsupported["error"]["code"], "unsupported_target");
    assert!(unsupported["usage"].is_null());
    // A failing reader surfaces the pinned typed network failure.
    models.usage.fail_next_reads();
    read_usage(&models, "cu4", "local", None).await;
    let failed = encoded(models.messages_for(CONNECTION_A).last().expect("failed"));
    assert_eq!(failed["success"], false);
    assert_eq!(failed["error"]["code"], super::USAGE_ERROR_NETWORK);
    assert_eq!(failed["error"]["message"], super::USAGE_BACKEND_UNAVAILABLE);
}

async fn read_usage(
    models: &super::support::TestModels,
    request_id: &str,
    target: &str,
    force_refresh: Option<bool>,
) {
    let mut body = json!({
        "type": "chatgpt_usage_read",
        "request_id": request_id,
        "target": target,
    });
    if let Some(force) = force_refresh {
        body["force_refresh"] = json!(force);
    }
    models.send(CONNECTION_A, &body).await;
}

#[tokio::test]
async fn update_model_decodes_routes_and_responds() {
    assert_in_fixture("commands", "update_model");
    let models = bridge_with_openai("sk-openai-um").await;
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "update_model",
                "request_id": "um1",
                "runtime": scope_json("default"),
                "payload": { "model_id": "openai/gpt-test", "reasoning_effort": "high" },
            }),
        )
        .await;
    let applied = encoded(models.messages_for(CONNECTION_A).last().expect("applied"));
    assert_eq!(applied["type"], "update_model_response");
    assert_in_fixture("messages", "update_model_response");
    assert_eq!(applied["request_id"], "um1");
    assert_eq!(applied["success"], true);
    assert_eq!(applied["runtime"]["agent_id"], "agent-1");
    assert_eq!(applied["runtime"]["conversation_id"], "default");
    assert_eq!(applied["applied_to"], "agent");
    assert_eq!(applied["model_id"], "openai/gpt-test");
    assert_eq!(applied["model_handle"], "openai/gpt-test");
    assert_eq!(applied["model_settings"]["reasoning_effort"], "high");
    // An unresolvable identifier is rejected without persistence.
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "update_model",
                "request_id": "um2",
                "runtime": scope_json("default"),
                "payload": { "model_id": "sonnet" },
            }),
        )
        .await;
    let missing = encoded(
        models
            .messages_for(CONNECTION_A)
            .last()
            .expect("unresolvable"),
    );
    assert_eq!(missing["request_id"], "um2");
    assert_eq!(missing["success"], false);
    assert_eq!(missing["error"], super::MODEL_NOT_FOUND);
    // An unavailable model is validated before persistence.
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "update_model",
                "request_id": "um3",
                "runtime": scope_json("conv-9"),
                "payload": { "model_id": "anthropic/claude-test" },
            }),
        )
        .await;
    let unavailable = encoded(
        models
            .messages_for(CONNECTION_A)
            .last()
            .expect("unavailable"),
    );
    assert_eq!(unavailable["request_id"], "um3");
    assert_eq!(unavailable["success"], false);
    assert_eq!(unavailable["error"], super::MODEL_UNAVAILABLE);
    assert_eq!(unavailable["model_id"], "anthropic/claude-test");
}

#[tokio::test]
async fn update_toolset_decodes_routes_and_responds() {
    assert_in_fixture("commands", "update_toolset");
    let models = bridge().await;
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "update_toolset",
                "request_id": "ut1",
                "runtime": scope_json("default"),
                "toolset_preference": "gemini_snake",
            }),
        )
        .await;
    let explicit = encoded(models.messages_for(CONNECTION_A).last().expect("explicit"));
    assert_eq!(explicit["type"], "update_toolset_response");
    assert_in_fixture("messages", "update_toolset_response");
    assert_eq!(explicit["request_id"], "ut1");
    assert_eq!(explicit["success"], true);
    assert_eq!(explicit["runtime"]["conversation_id"], "default");
    assert_eq!(explicit["current_toolset"], "gemini_snake");
    assert_eq!(explicit["current_toolset_preference"], "gemini_snake");
    // Auto with no stored model hints resolves to the default toolset.
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "update_toolset",
                "request_id": "ut2",
                "runtime": scope_json("default"),
                "toolset_preference": "auto",
            }),
        )
        .await;
    let auto = encoded(models.messages_for(CONNECTION_A).last().expect("auto"));
    assert_eq!(auto["request_id"], "ut2");
    assert_eq!(auto["success"], true);
    assert_eq!(auto["current_toolset"], "default");
    assert_eq!(auto["current_toolset_preference"], "auto");
}

//! Scoped update commands validate availability and resolve toolsets first.

use super::support::{
    CONNECTION_A, CONVERSATION_ID, bridge, bridge_with_openai, encoded, scope_json, scope_key,
};
use crate::framing;
use lotta_tools::{ToolsetId, ToolsetPreference};
use serde_json::json;

#[tokio::test]
async fn model_update_validates_first() {
    let models = bridge_with_openai("sk-openai-updates").await;
    update_model(&models, "uv1", "openai/gpt-test").await;
    let applied = encoded(models.messages_for(CONNECTION_A).last().expect("applied"));
    assert_eq!(applied["type"], "update_model_response");
    assert_eq!(applied["request_id"], "uv1");
    assert_eq!(applied["success"], true);
    assert_eq!(applied["applied_to"], "agent");
    assert_eq!(applied["model_handle"], "openai/gpt-test");
    let scope = scope_key(CONVERSATION_ID);
    let stored = models
        .bridge
        .stored_model(&scope)
        .await
        .expect("stored model");
    assert_eq!(stored.handle.to_string(), "openai/gpt-test");
    assert_eq!(stored.revision, 1);
    // An unavailable model is rejected before any persistence happens.
    update_model(&models, "uv2", "anthropic/claude-test").await;
    let rejected = encoded(models.messages_for(CONNECTION_A).last().expect("rejected"));
    assert_eq!(rejected["request_id"], "uv2");
    assert_eq!(rejected["success"], false);
    assert_eq!(rejected["error"], super::MODEL_UNAVAILABLE);
    let unchanged = models
        .bridge
        .stored_model(&scope)
        .await
        .expect("stored model");
    assert_eq!(unchanged, stored);
}

#[tokio::test]
async fn toolset_update_resolves_valid_id() {
    let models = bridge().await;
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "update_toolset",
                "request_id": "tv1",
                "runtime": scope_json(CONVERSATION_ID),
                "toolset_preference": "gemini_snake",
            }),
        )
        .await;
    let applied = encoded(models.messages_for(CONNECTION_A).last().expect("applied"));
    assert_eq!(applied["type"], "update_toolset_response");
    assert_eq!(applied["request_id"], "tv1");
    assert_eq!(applied["success"], true);
    assert_eq!(applied["runtime"]["agent_id"], super::support::AGENT_ID);
    assert_eq!(applied["current_toolset"], "gemini_snake");
    assert_eq!(applied["current_toolset_preference"], "gemini_snake");
    let stored = models
        .bridge
        .stored_toolset(&scope_key(CONVERSATION_ID))
        .await;
    assert_eq!(
        stored,
        Some(ToolsetPreference::Explicit(ToolsetId::GeminiSnake))
    );
}

#[tokio::test]
async fn unknown_toolset_name_rejected() {
    let models = bridge().await;
    let command = json!({
        "type": "update_toolset",
        "request_id": "tu9",
        "runtime": scope_json(CONVERSATION_ID),
        "toolset_preference": "bogus_toolset",
    });
    let frame = framing::decode_text(&command.to_string()).expect("bounded frame");
    let error = super::decode(&frame).expect_err("unknown toolset name rejected");
    assert_eq!(error.code, "models_command_invalid");
    assert_eq!(error.message, "invalid models command");
    assert_eq!(error.request_id.as_deref(), Some("tu9"));
    assert!(models.messages_for(CONNECTION_A).is_empty());
}

async fn update_model(models: &super::support::TestModels, request_id: &str, model_id: &str) {
    models
        .send(
            CONNECTION_A,
            &json!({
                "type": "update_model",
                "request_id": request_id,
                "runtime": scope_json(CONVERSATION_ID),
                "payload": { "model_id": model_id },
            }),
        )
        .await;
}

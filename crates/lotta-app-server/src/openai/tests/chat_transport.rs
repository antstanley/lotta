use serde_json::Value;

use super::test_support::{agent, launch, post, roots, seed};

#[tokio::test]
async fn external_missing_model_is_exact_openai_boundary() {
    let roots = roots("chat-missing-model");
    seed(
        &roots.storage,
        &[agent("agent-local-chat-present", "present", false)],
    )
    .await;
    let mut handle = launch(&roots, true, None).await;
    let response = post(
        &handle,
        "/v1/chat/completions",
        "",
        r#"{"model":"absent","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await;
    assert_eq!(response.status, 404);
    assert_eq!(response.content_type.as_deref(), Some("application/json"));
    let body: Value = serde_json::from_str(&response.body)
        .unwrap_or_else(|error| panic!("OpenAI error JSON: {error}; {}", response.body));
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert_eq!(body["error"]["code"], "model_not_found");
    assert_eq!(body["error"]["param"], Value::Null);
    handle.shutdown();
    handle
        .wait()
        .await
        .unwrap_or_else(|error| panic!("listener shutdown: {error}"));
}

#[tokio::test]
async fn external_invalid_request_uses_openai_envelope() {
    let roots = roots("chat-invalid-request");
    let mut handle = launch(&roots, true, None).await;
    let response = post(
        &handle,
        "/v1/chat/completions",
        "",
        r#"{"model":"","messages":[]}"#,
    )
    .await;
    assert_eq!(response.status, 400);
    assert_eq!(response.content_type.as_deref(), Some("application/json"));
    let body: Value = serde_json::from_str(&response.body)
        .unwrap_or_else(|error| panic!("OpenAI error JSON: {error}; {}", response.body));
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert_eq!(body["error"]["code"], Value::Null);
    handle.shutdown();
    handle
        .wait()
        .await
        .unwrap_or_else(|error| panic!("listener shutdown: {error}"));
}

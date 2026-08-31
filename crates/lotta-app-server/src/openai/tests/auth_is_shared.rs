use super::test_support::{get, launch, roots};
use crate::config::ServerArgs;

#[tokio::test(flavor = "multi_thread")]
async fn http_and_websocket_use_same_capability_policy() {
    let roots = roots("auth-shared");
    let handle = launch(&roots, true, Some("correct-token")).await;
    for target in ["/v1/models", "/app-server-info", "/ws"] {
        assert_eq!(get(&handle, target, "").await.status, 401);
        assert_eq!(
            get(&handle, target, "Authorization: Bearer wrong\r\n")
                .await
                .status,
            401
        );
    }
    let openai_unauthorized = get(&handle, "/v1/models", "").await;
    let envelope: serde_json::Value = serde_json::from_str(&openai_unauthorized.body)
        .unwrap_or_else(|error| panic!("OpenAI auth JSON: {error}"));
    assert_eq!(envelope["error"]["type"], "authentication_error");
    assert_eq!(envelope["error"]["param"], serde_json::Value::Null);
    assert_eq!(envelope["error"]["code"], serde_json::Value::Null);
    let auth = "Authorization: Bearer correct-token\r\n";
    assert_eq!(get(&handle, "/v1/models", auth).await.status, 200);
    assert_eq!(get(&handle, "/app-server-info", auth).await.status, 200);
    assert_eq!(get(&handle, "/ws", auth).await.status, 400);
    assert_eq!(get(&handle, "/healthz", "").await.status, 200);
    handle
        .wait()
        .await
        .unwrap_or_else(|error| panic!("stop: {error}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn origin_policy_is_shared() {
    let roots = roots("origin-shared");
    let handle = launch(&roots, true, None).await;
    let headers = "Origin: https://example.test\r\n";
    assert_eq!(get(&handle, "/v1/models", headers).await.status, 401);
    assert_eq!(get(&handle, "/app-server-info", headers).await.status, 401);
    assert_eq!(get(&handle, "/ws", headers).await.status, 401);
    handle
        .wait()
        .await
        .unwrap_or_else(|error| panic!("stop: {error}"));
}

#[test]
fn non_loopback_requires_auth_with_openai_routes() {
    let roots = roots("non-loopback");
    let args = ServerArgs {
        listen: Some("ws://0.0.0.0:0".to_owned()),
        listen_enabled: true,
        openai_api: true,
        storage_dir: Some(roots.storage),
        workspace_dir: Some(roots.workspace),
        ..ServerArgs::default()
    };
    assert!(args.prepare().is_err());
}

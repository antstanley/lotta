use serde_json::Value;

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::test_support::{
    agent, launch, launch_with_runtime, launch_with_runtime_and_controller, post, post_at, roots,
    seed,
};

struct PanicController;

impl crate::ws::TurnController for PanicController {
    fn submit_turn(
        &self,
        _: crate::ws::command::InputCommand,
        _: crate::ws::DeferredInput,
        _: CancellationToken,
        _: Arc<dyn crate::ws::RuntimeEventSink>,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::error::AppServerError>> + Send + '_>> {
        Box::pin(async { panic!("synthetic controller panic") })
    }
}

struct ExitGuard<'a>(&'a AtomicUsize);

impl Drop for ExitGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct BlockingController {
    entered: Notify,
    exited: AtomicUsize,
}

impl crate::ws::TurnController for BlockingController {
    fn submit_turn(
        &self,
        _: crate::ws::command::InputCommand,
        _: crate::ws::DeferredInput,
        cancellation: CancellationToken,
        _: Arc<dyn crate::ws::RuntimeEventSink>,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::error::AppServerError>> + Send + '_>> {
        Box::pin(async move {
            let _guard = ExitGuard(&self.exited);
            self.entered.notify_one();
            cancellation.cancelled().await;
            Err(crate::error::AppServerError::Unavailable)
        })
    }
}

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

#[tokio::test(flavor = "multi_thread")]
async fn real_listener_o7_claims_before_allocation_and_replays_json_and_sse() {
    let roots = roots("chat-o7-real-listener");
    seed(
        &roots.storage,
        &[agent("agent-local-chat-o7", "memo", false)],
    )
    .await;
    let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
    let mut handle = launch_with_runtime(&roots, true, None, runtime.clone()).await;
    let json_body = r#"{"model":"memo","messages":[{"role":"user","content":"hi"}]}"#;
    let sse_body = r#"{"model":"memo","messages":[{"role":"user","content":"hi"}],"stream":true}"#;
    let standard = "Idempotency-Key: o7-key\r\n";
    let (json_response, sse_response) = tokio::join!(
        post(&handle, "/v1/chat/completions", standard, json_body),
        post(&handle, "/v1/chat/completions", standard, sse_body),
    );
    assert_eq!(json_response.status, 200);
    assert_eq!(
        json_response.content_type.as_deref(),
        Some("application/json")
    );
    assert_eq!(sse_response.status, 200);
    assert_eq!(
        sse_response.content_type.as_deref(),
        Some("text/event-stream")
    );

    let replay = post(
        &handle,
        "/v1/chat/completions",
        "X-Idempotency-Key: o7-key\r\n",
        json_body,
    )
    .await;
    assert_eq!(replay.status, 200);
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 2);
    let stages = runtime
        .stages
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(stages.as_slice(), ["admit", "continue"]);
    let subscriptions = runtime
        .subscriptions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0].1, 0);

    let conversations = roots.storage.join("conversations");
    let remaining = std::fs::read_dir(conversations).map_or(0, std::iter::Iterator::count);
    assert_eq!(
        remaining, 0,
        "ephemeral conversation artifacts must be deleted"
    );
    handle.shutdown();
    handle
        .wait()
        .await
        .unwrap_or_else(|error| panic!("listener shutdown: {error}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn real_listener_panicking_owner_settles_waiter_and_cleans_ephemeral_state() {
    let roots = roots("chat-owner-panic");
    seed(
        &roots.storage,
        &[agent("agent-local-chat-panic", "memo", false)],
    )
    .await;
    let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
    let mut handle =
        launch_with_runtime_and_controller(&roots, true, None, runtime, Arc::new(PanicController))
            .await;
    let response = post(
        &handle,
        "/v1/chat/completions",
        "Idempotency-Key: panic-key\r\n",
        r#"{"model":"memo","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await;
    assert_eq!(response.status, 500);
    let error: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(error["error"]["message"], "internal server error");
    let remaining = std::fs::read_dir(roots.storage.join("conversations"))
        .map_or(0, std::iter::Iterator::count);
    assert_eq!(remaining, 0);
    handle.shutdown();
    handle.wait().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn listener_shutdown_cancels_joins_owner_and_settles_http_waiter() {
    let roots = roots("chat-owner-shutdown");
    seed(
        &roots.storage,
        &[agent("agent-local-chat-shutdown", "memo", false)],
    )
    .await;
    let runtime = Arc::new(crate::ws::test_support::RecordingService::default());
    let controller = Arc::new(BlockingController::default());
    let mut handle =
        launch_with_runtime_and_controller(&roots, true, None, runtime, controller.clone()).await;
    let address = handle.address();
    let request = tokio::spawn(async move {
        post_at(
            address,
            "/v1/chat/completions",
            "Idempotency-Key: shutdown-key\r\n",
            r#"{"model":"memo","messages":[{"role":"user","content":"hi"}]}"#,
        )
        .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        controller.entered.notified(),
    )
    .await
    .unwrap();
    handle.shutdown();
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 500);
    handle.wait().await.unwrap();
    assert_eq!(controller.exited.load(Ordering::SeqCst), 1);
    let remaining = std::fs::read_dir(roots.storage.join("conversations"))
        .map_or(0, std::iter::Iterator::count);
    assert_eq!(remaining, 0);
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

use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use lotta_domain::{Clock, DomainError, Timestamp};
use tower::ServiceExt;

use super::{ListenerState, SocketLimits, build_router, start_listener_for_test};
use crate::{
    auth::AuthPolicy,
    bounds::{WS_FRAME_BYTES_MAX, WS_PING_INTERVAL_MS},
    config::ServerArgs,
    errors::AppServerError,
};

struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        match Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56Z") {
            Ok(value) => value,
            Err(error) => panic!("fixed timestamp: {error}"),
        }
    }

    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

fn router() -> axum::Router {
    let state = Arc::new(ListenerState {
        auth: AuthPolicy::None,
        clock: Arc::new(TestClock),
        shutdown: tokio_util::sync::CancellationToken::new(),
        limits: SocketLimits::default(),
        runtime_router: Arc::new(std::sync::Mutex::new(crate::ws::RuntimeRouter::new(
            Arc::new(TestClock),
            Arc::new(crate::ws::RandomEventIdGenerator),
        ))),
        runtime_service: Arc::new(crate::ws::UnsupportedRuntimeCommandService),
        turn_controller: Arc::new(crate::ws::UnsupportedRuntimeCommandService),
        observer: Arc::new(crate::observer::InertRuntimeBroadcastObserver),
        external_tools: Arc::new(crate::ws::external_tools::ExternalToolBridge::new(
            crate::ws::external_tools::inert_forwarder(),
        )),
        teleports: Arc::new(crate::ws::teleport::TeleportBridge::new(
            crate::ws::teleport::inert_forwarder(),
        )),
        terminals: Arc::new(crate::ws::terminal::TerminalBridge::new(
            inert_terminal_forwarder(),
            Arc::new(TestClock),
        )),
        files: Arc::new(
            crate::ws::files::FilesBridge::with_poll_interval(
                crate::ws::files::inert_forwarder(),
                &files_workspace("transport"),
                &files_workspace("transport-artifacts"),
                60_000,
            )
            .expect("files bridge"),
        ),
        memories: Arc::new(
            crate::ws::memory::MemoryBridge::new(
                crate::ws::memory::inert_forwarder(),
                &files_workspace("transport-memfs"),
                Arc::new(TestClock),
            )
            .expect("memory bridge"),
        ),
        next_observation: std::sync::atomic::AtomicU64::new(1),
        outbound: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    });
    build_router("/ws", state)
}

fn files_workspace(name: &str) -> std::path::PathBuf {
    static ORDINAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let ordinal = ORDINAL.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "lotta-listener-{name}-{}-{ordinal}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("workspace directory");
    path.canonicalize().expect("canonical workspace")
}

fn inert_terminal_forwarder() -> crate::ws::terminal::TerminalForwarder {
    Arc::new(|_, _| Ok(()))
}

async fn response(method: Method, path: &str) -> (StatusCode, String) {
    let request = match Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
    {
        Ok(request) => request,
        Err(error) => panic!("request: {error}"),
    };
    let response = match router().oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    };
    let status = response.status();
    let bytes = match to_bytes(response.into_body(), 4096).await {
        Ok(bytes) => bytes,
        Err(error) => panic!("body: {error}"),
    };
    let body = match String::from_utf8(bytes.to_vec()) {
        Ok(body) => body,
        Err(error) => panic!("UTF-8 body: {error}"),
    };
    (status, body)
}

#[tokio::test]
async fn actual_fallback_is_stable_json_404() {
    let actual = response(Method::GET, "/missing").await;
    assert_eq!(actual.0, StatusCode::NOT_FOUND);
    assert_eq!(actual.1, r#"{"code":"not_found","error":"not found"}"#);
}

#[tokio::test]
async fn actual_method_fallback_is_stable_json_405() {
    let actual = response(Method::POST, "/healthz").await;
    assert_eq!(actual.0, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        actual.1,
        r#"{"code":"method_not_allowed","error":"method not allowed"}"#
    );
}

#[tokio::test]
async fn actual_forbidden_handler_is_stable_json_403() {
    let actual = response(Method::GET, "/__test/forbidden").await;
    assert_eq!(
        actual,
        (
            StatusCode::FORBIDDEN,
            r#"{"code":"auth_forbidden","error":"forbidden"}"#.into()
        )
    );
}

#[tokio::test]
async fn actual_unavailable_handler_is_stable_json_503() {
    let actual = response(Method::GET, "/__test/unavailable").await;
    assert_eq!(
        actual,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"code":"service_unavailable","error":"service unavailable"}"#.into()
        )
    );
}

#[tokio::test]
async fn actual_internal_handler_is_stable_json_500() {
    let actual = response(Method::GET, "/__test/internal").await;
    assert_eq!(
        actual,
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"code":"internal_error","error":"internal server error"}"#.into()
        )
    );
}

#[tokio::test]
async fn actual_malformed_upgrade_is_stable_json_400() {
    let actual = response(Method::GET, "/ws").await;
    assert_eq!(actual.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        actual.1,
        r#"{"code":"request_malformed","error":"malformed request"}"#
    );
}

#[test]
fn receive_error_classification_never_labels_non_capacity_as_1009() {
    let protocol = tungstenite::Error::Protocol(
        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
    );
    let wrapped = axum::Error::new(protocol);
    let code = super::websocket_error_close(wrapped).map(|value| value.0);
    assert_ne!(code, Some(axum::extract::ws::close_code::SIZE));
}

#[tokio::test]
async fn test_listener_uses_same_path_with_canonical_limits() {
    let args = ServerArgs {
        listen_enabled: true,
        ..ServerArgs::default()
    };
    let prepared = match args.prepare() {
        Ok(prepared) => prepared,
        Err(error) => panic!("prepare: {error}"),
    };
    let handle = match start_listener_for_test(
        prepared,
        Arc::new(TestClock),
        WS_FRAME_BYTES_MAX,
        WS_PING_INTERVAL_MS,
    )
    .await
    {
        Ok(handle) => handle,
        Err(error) => panic!("start: {error}"),
    };
    assert_ne!(handle.address().port(), 0);
    if let Err(error) = handle.wait().await {
        panic!("wait: {error}");
    }
}

#[test]
fn explicit_error_variants_have_stable_route_statuses() {
    assert_eq!(AppServerError::Forbidden.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        AppServerError::Unavailable.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        AppServerError::Internal.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket, close_code, rejection::WebSocketUpgradeRejection},
    },
    http::{HeaderMap, Request},
    response::{IntoResponse, Response},
    routing::get,
};
use lotta_domain::Clock;
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    auth::origin,
    bounds::{HTTP_BODY_BYTES_MAX, WS_FRAME_BYTES_MAX, WS_PING_INTERVAL_MS},
    config::{PreparedServer, is_loopback_host},
    error::AppServerError,
    heartbeat::Heartbeat,
};

#[cfg(test)]
#[path = "listener/tests/heartbeat.rs"]
mod heartbeat_integration;
#[cfg(test)]
#[path = "listener/tests/transport.rs"]
mod transport;
#[cfg(test)]
#[path = "listener/tests/url_resolution.rs"]
mod url_resolution;
#[cfg(test)]
#[path = "listener/tests/websocket.rs"]
mod websocket;

/// Compatibility re-export of the canonical WebSocket frame ceiling.
pub use crate::bounds::WS_FRAME_BYTES_MAX as FRAME_BYTES_MAX;

struct SocketLimits {
    frame_bytes: usize,
    ping_interval_ms: u64,
}

impl Default for SocketLimits {
    fn default() -> Self {
        Self {
            frame_bytes: WS_FRAME_BYTES_MAX,
            ping_interval_ms: WS_PING_INTERVAL_MS,
        }
    }
}

struct ListenerState {
    auth: crate::auth::AuthPolicy,
    clock: Arc<dyn Clock + Send + Sync>,
    shutdown: CancellationToken,
    limits: SocketLimits,
}

/// Owned running listener with resolved URLs and graceful shutdown.
pub struct ListenerHandle {
    address: SocketAddr,
    base_url: String,
    websocket_url: String,
    openai_url: Option<String>,
    shutdown: CancellationToken,
    task: JoinHandle<Result<(), AppServerError>>,
}

impl ListenerHandle {
    /// Returns the actual bound address, including an OS-selected port.
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }
    /// Returns the resolved HTTP authority represented with the baseline `ws` scheme.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    /// Returns the resolved configured websocket endpoint.
    #[must_use]
    pub fn websocket_url(&self) -> &str {
        &self.websocket_url
    }
    /// Returns the optional OpenAI-compatible HTTP base.
    #[must_use]
    pub fn openai_url(&self) -> Option<&str> {
        self.openai_url.as_deref()
    }
    /// Requests graceful listener and active WebSocket shutdown.
    pub fn shutdown(&mut self) {
        self.shutdown.cancel();
    }
    /// Waits for the owned serve task after requesting shutdown.
    ///
    /// # Errors
    /// Returns a stable task or listener failure if graceful serving fails.
    pub async fn wait(mut self) -> Result<(), AppServerError> {
        self.shutdown();
        self.task.await.map_err(|_| AppServerError::Task)?
    }
}

/// Binds and starts an Axum WebSocket listener after all policy preparation.
///
/// # Errors
/// Returns a stable listener error when bind or local address discovery fails.
pub async fn start_listener(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
) -> Result<ListenerHandle, AppServerError> {
    start_listener_with_limits(prepared, clock, SocketLimits::default()).await
}

#[cfg(test)]
async fn start_listener_for_test(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    frame_bytes: usize,
    ping_interval_ms: u64,
) -> Result<ListenerHandle, AppServerError> {
    let limits = SocketLimits {
        frame_bytes,
        ping_interval_ms,
    };
    start_listener_with_limits(prepared, clock, limits).await
}

async fn start_listener_with_limits(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    limits: SocketLimits,
) -> Result<ListenerHandle, AppServerError> {
    if !is_loopback_host(&prepared.host) && prepared.auth.is_none() {
        return Err(AppServerError::Config(
            "non-loopback listeners require websocket authentication",
        ));
    }
    let listener = TcpListener::bind(format_bind_address(&prepared.host, prepared.port))
        .await
        .map_err(|_| AppServerError::Listener)?;
    let address = listener
        .local_addr()
        .map_err(|_| AppServerError::Listener)?;
    let (base_url, websocket_url, openai_url) = resolved_urls(&prepared, address);
    tracing::info!(base_url, websocket_url, "app server listener started");
    let shutdown = CancellationToken::new();
    let state = Arc::new(ListenerState {
        auth: prepared.auth,
        clock,
        shutdown: shutdown.clone(),
        limits,
    });
    let router = build_router(&prepared.websocket_path, state);
    let server_shutdown = shutdown.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_shutdown.cancelled_owned())
            .await
            .map_err(|_| AppServerError::Listener)
    });
    Ok(ListenerHandle {
        address,
        base_url,
        websocket_url,
        openai_url,
        shutdown,
        task,
    })
}

fn build_router(path: &str, state: Arc<ListenerState>) -> Router {
    let router = Router::new()
        .route("/", get(upgrade).fallback(method_not_allowed))
        .route(
            "/healthz",
            get(|| async { "ok\n" }).fallback(method_not_allowed),
        )
        .route(
            "/readyz",
            get(|| async { "ok\n" }).fallback(method_not_allowed),
        );
    let router = if path == "/" {
        router
    } else {
        router.route(path, get(upgrade).fallback(method_not_allowed))
    };
    #[cfg(test)]
    let router = router
        .route("/__test/forbidden", get(test_forbidden))
        .route("/__test/unavailable", get(test_unavailable))
        .route("/__test/internal", get(test_internal));
    router
        .with_state(state)
        .fallback(not_found)
        .layer(axum::extract::DefaultBodyLimit::max(HTTP_BODY_BYTES_MAX))
}

async fn upgrade(
    State(state): State<Arc<ListenerState>>,
    headers: HeaderMap,
    websocket: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if let Err(error) = state.auth.authorize(&headers, state.clock.as_ref()) {
        tracing::warn!(code = error.code(), "websocket authentication denied");
        return error.into_response();
    }
    if let Err(error) = origin::enforce(&headers, &state.auth) {
        tracing::warn!(code = error.code(), "websocket origin denied");
        return error.into_response();
    }
    let Ok(websocket) = websocket else {
        return AppServerError::Malformed.into_response();
    };
    let connection = state.clone();
    websocket
        .max_frame_size(state.limits.frame_bytes)
        .max_message_size(state.limits.frame_bytes)
        .on_failed_upgrade(|_| {})
        .on_upgrade(move |socket| serve_socket(socket, connection))
        .into_response()
}

async fn serve_socket(mut socket: WebSocket, state: Arc<ListenerState>) {
    let mut heartbeat = Heartbeat::new(state.clock.as_ref());
    let interval_ms = state.limits.ping_interval_ms;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(interval_ms));
    interval.tick().await;
    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => {
                send_close(&mut socket, close_code::AWAY, "server shutdown").await;
                break;
            }
            _ = interval.tick() => {
                if heartbeat.is_expired(state.clock.as_ref()) {
                    send_close(&mut socket, close_code::AWAY, "heartbeat expired").await;
                    break;
                }
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                let keep_open = handle_incoming(
                    &mut socket,
                    incoming,
                    &mut heartbeat,
                    state.clock.as_ref(),
                )
                .await;
                if !keep_open {
                    break;
                }
            }
        }
    }
}

async fn handle_incoming(
    socket: &mut WebSocket,
    incoming: Option<Result<Message, axum::Error>>,
    heartbeat: &mut Heartbeat,
    clock: &(dyn Clock + Send + Sync),
) -> bool {
    match incoming {
        Some(Ok(Message::Pong(_))) => {
            heartbeat.record_pong(clock);
            true
        }
        Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await.is_ok(),
        Some(Ok(Message::Text(text))) => handle_text(socket, &text).await,
        Some(Ok(Message::Binary(_))) => {
            send_close(socket, close_code::UNSUPPORTED, "binary unsupported").await;
            false
        }
        Some(Ok(Message::Close(_))) | None => false,
        Some(Err(error)) => {
            let (code, reason) = websocket_error_close(error);
            if let Some((code, reason)) = code.zip(reason) {
                send_close(socket, code, reason).await;
            }
            false
        }
    }
}

fn websocket_error_close(error: axum::Error) -> (Option<u16>, Option<&'static str>) {
    let inner = error.into_inner();
    let Some(error) = inner.downcast_ref::<tungstenite::Error>() else {
        return (Some(close_code::ERROR), Some("internal websocket error"));
    };
    match error {
        tungstenite::Error::Capacity(_) => (Some(close_code::SIZE), Some("message too large")),
        tungstenite::Error::Protocol(_) => {
            (Some(close_code::PROTOCOL), Some("websocket protocol error"))
        }
        tungstenite::Error::Utf8(_) => (Some(close_code::INVALID), Some("invalid UTF-8")),
        tungstenite::Error::Io(_)
        | tungstenite::Error::Tls(_)
        | tungstenite::Error::WriteBufferFull(_) => (None, None),
        _ => (Some(close_code::ERROR), Some("internal websocket error")),
    }
}

async fn handle_text(socket: &mut WebSocket, text: &str) -> bool {
    match crate::framing::decode_text(text) {
        Ok(_) => true,
        Err(error) => match serde_json::to_string(&error) {
            Ok(body) => socket.send(Message::Text(body.into())).await.is_ok(),
            Err(_) => false,
        },
    }
}

async fn send_close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    let _ = socket.send(Message::Close(Some(frame))).await;
}

async fn not_found(_: Request<Body>) -> Response {
    AppServerError::NotFound.into_response()
}

async fn method_not_allowed() -> Response {
    AppServerError::MethodNotAllowed.into_response()
}

#[cfg(test)]
async fn test_forbidden() -> Response {
    AppServerError::Forbidden.into_response()
}

#[cfg(test)]
async fn test_unavailable() -> Response {
    AppServerError::Unavailable.into_response()
}

#[cfg(test)]
async fn test_internal() -> Response {
    AppServerError::Internal.into_response()
}

fn format_bind_address(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn resolved_urls(
    prepared: &PreparedServer,
    address: SocketAddr,
) -> (String, String, Option<String>) {
    let host = if prepared.host.contains(':') {
        format!("[{}]", prepared.host)
    } else {
        prepared.host.clone()
    };
    let base = format!("ws://{host}:{}", address.port());
    let websocket = format!("{base}{}", prepared.websocket_path);
    let openai = prepared
        .openai_api
        .then(|| format!("http://{host}:{}/v1", address.port()));
    (base, websocket, openai)
}

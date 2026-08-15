use std::{collections::HashMap, net::SocketAddr, sync::Arc};

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
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    auth::origin,
    bounds::{HTTP_BODY_BYTES_MAX, WS_FRAME_BYTES_MAX, WS_PING_INTERVAL_MS},
    config::{PreparedServer, is_loopback_host},
    error::AppServerError,
    heartbeat::Heartbeat,
    ws::{
        EventDeliveryBatch, RandomEventIdGenerator, RouterEventSink, RuntimeCommandService,
        RuntimeRouter, UnsupportedRuntimeCommandService, lock_router, route_command,
    },
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
    runtime_router: Arc<std::sync::Mutex<RuntimeRouter>>,
    runtime_service: Arc<dyn RuntimeCommandService>,
    outbound: Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
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
    start_listener_with_runtime_service(prepared, clock, Arc::new(UnsupportedRuntimeCommandService))
        .await
}

/// Binds a listener with an injectable Runtime command application service.
///
/// # Errors
/// Returns a stable listener error when startup fails.
pub async fn start_listener_with_runtime_service(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    runtime_service: Arc<dyn RuntimeCommandService>,
) -> Result<ListenerHandle, AppServerError> {
    start_listener_with_limits(prepared, clock, SocketLimits::default(), runtime_service).await
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
    start_listener_with_limits(
        prepared,
        clock,
        limits,
        Arc::new(UnsupportedRuntimeCommandService),
    )
    .await
}

async fn start_listener_with_limits(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    limits: SocketLimits,
    runtime_service: Arc<dyn RuntimeCommandService>,
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
    let runtime_router = RuntimeRouter::new(clock.clone(), Arc::new(RandomEventIdGenerator));
    let state = Arc::new(ListenerState {
        auth: prepared.auth,
        clock,
        shutdown: shutdown.clone(),
        limits,
        runtime_router: Arc::new(std::sync::Mutex::new(runtime_router)),
        runtime_service,
        outbound: Arc::new(std::sync::Mutex::new(HashMap::new())),
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

/// Maximum queued outbound frames for one live connection.
pub const WS_OUTBOUND_FRAMES_PER_CONNECTION_MAX: usize = 256;

async fn serve_socket(mut socket: WebSocket, state: Arc<ListenerState>) {
    let (sender, mut receiver) = mpsc::channel(WS_OUTBOUND_FRAMES_PER_CONNECTION_MAX);
    let Ok(connection_id) = open_connection(&state, sender) else {
        return;
    };
    let mut heartbeat = Heartbeat::new(state.clock.as_ref());
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(
        state.limits.ping_interval_ms,
    ));
    interval.tick().await;
    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => {
                send_close(&mut socket, close_code::AWAY, "server shutdown").await;
                break;
            }
            Some(body) = receiver.recv() => {
                if socket.send(Message::Text(body.into())).await.is_err() {
                    break;
                }
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
                    incoming,
                    &mut socket,
                    &mut heartbeat,
                    &state,
                    connection_id,
                ).await;
                if !keep_open {
                    break;
                }
            }
        }
    }
    close_connection(&state, connection_id);
}

fn open_connection(
    state: &ListenerState,
    sender: mpsc::Sender<String>,
) -> Result<crate::ws::ConnectionId, AppServerError> {
    let id = {
        let mut router = lock_router(&state.runtime_router)?;
        let id = router.connections.open()?;
        router.connections.initialize(id)?;
        id
    };
    let inserted = prepare_outbound(state, id, sender);
    if inserted.is_err() {
        lock_router(&state.runtime_router)?.connections.close(id);
    }
    inserted.map(|()| id)
}

fn prepare_outbound(
    state: &ListenerState,
    id: crate::ws::ConnectionId,
    sender: mpsc::Sender<String>,
) -> Result<(), AppServerError> {
    let mut outbound = state
        .outbound
        .lock()
        .map_err(|_| AppServerError::Internal)?;
    outbound
        .try_reserve(1)
        .map_err(|_| AppServerError::Unavailable)?;
    outbound.insert(id, sender);
    Ok(())
}

fn close_connection(state: &ListenerState, id: crate::ws::ConnectionId) {
    if let Ok(mut outbound) = state.outbound.lock() {
        outbound.remove(&id);
    }
    if let Ok(mut router) = state.runtime_router.lock() {
        router.connections.close(id);
    }
}

async fn handle_incoming(
    incoming: Option<Result<Message, axum::Error>>,
    socket: &mut WebSocket,
    heartbeat: &mut Heartbeat,
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
) -> bool {
    match incoming {
        Some(Ok(Message::Pong(_))) => {
            heartbeat.record_pong(state.clock.as_ref());
            true
        }
        Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await.is_ok(),
        Some(Ok(Message::Text(text))) => handle_text(&text, state, connection_id).await,
        Some(Ok(Message::Binary(_))) => {
            send_close(socket, close_code::UNSUPPORTED, "binary unsupported").await;
            false
        }
        Some(Ok(Message::Close(_))) | None => false,
        Some(Err(error)) => {
            if let Some((code, reason)) = websocket_error_close(error) {
                send_close(socket, code, reason).await;
            }
            false
        }
    }
}

fn websocket_error_close(error: axum::Error) -> Option<(u16, &'static str)> {
    let inner = error.into_inner();
    let Some(error) = inner.downcast_ref::<tungstenite::Error>() else {
        return Some((close_code::ERROR, "internal websocket error"));
    };
    match error {
        tungstenite::Error::Capacity(_) => Some((close_code::SIZE, "message too large")),
        tungstenite::Error::Protocol(_) => Some((close_code::PROTOCOL, "websocket protocol error")),
        tungstenite::Error::Utf8(_) => Some((close_code::INVALID, "invalid UTF-8")),
        tungstenite::Error::Io(_)
        | tungstenite::Error::Tls(_)
        | tungstenite::Error::WriteBufferFull(_) => None,
        _ => Some((close_code::ERROR, "internal websocket error")),
    }
}

async fn handle_text(
    text: &str,
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
) -> bool {
    let frame = match crate::framing::decode_text(text) {
        Ok(frame) => frame,
        Err(error) => return dispatch_value(state, connection_id, &error).is_ok(),
    };
    let command = match crate::ws::command::decode(&frame) {
        Ok(Some(command)) => command,
        Ok(None) => return true,
        Err(error) => return dispatch_value(state, connection_id, &error).is_ok(),
    };
    let routed = route_command(
        state.runtime_router.clone(),
        state.runtime_service.clone(),
        connection_id,
        command,
    )
    .await;
    let Ok((output, deferred)) = routed else {
        return dispatch_typed_failure(state, connection_id, &frame).is_ok();
    };
    if dispatch_output(state, connection_id, &output).is_err() {
        return false;
    }
    if let Some(deferred) = deferred {
        let sink = event_sink(state);
        if state
            .runtime_service
            .continue_input(deferred.scope, deferred.continuation, sink)
            .await
            .is_err()
        {
            return false;
        }
    }
    true
}

fn event_sink(state: &Arc<ListenerState>) -> Arc<dyn crate::ws::RuntimeEventSink> {
    let outbound = state.outbound.clone();
    Arc::new(RouterEventSink::new(
        state.runtime_router.clone(),
        Arc::new(move |deliveries| dispatch_deliveries(&outbound, &deliveries)),
    ))
}

fn dispatch_output(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    output: &crate::ws::RouteOutput,
) -> Result<(), AppServerError> {
    for response in output.responses.as_slice() {
        dispatch_value(state, connection_id, response)?;
    }
    dispatch_deliveries(&state.outbound, &output.deliveries)
}

fn dispatch_value(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    value: &impl serde::Serialize,
) -> Result<(), AppServerError> {
    let sender = state
        .outbound
        .lock()
        .map_err(|_| AppServerError::Internal)?
        .get(&connection_id)
        .cloned()
        .ok_or(AppServerError::Unavailable)?;
    let body = serde_json::to_string(value).map_err(|_| AppServerError::Internal)?;
    sender
        .try_send(body)
        .map_err(|_| AppServerError::Unavailable)
}

fn dispatch_deliveries(
    outbound: &std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>,
    deliveries: &EventDeliveryBatch,
) -> Result<(), AppServerError> {
    let senders = {
        let outbound = outbound.lock().map_err(|_| AppServerError::Internal)?;
        deliveries
            .as_slice()
            .iter()
            .map(|delivery| {
                outbound
                    .get(&delivery.connection_id)
                    .cloned()
                    .ok_or(AppServerError::Unavailable)
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    for (delivery, sender) in deliveries.as_slice().iter().zip(senders) {
        let body = serde_json::to_string(&delivery.frame).map_err(|_| AppServerError::Internal)?;
        sender
            .try_send(body)
            .map_err(|_| AppServerError::Unavailable)?;
    }
    Ok(())
}

fn dispatch_typed_failure(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> Result<(), AppServerError> {
    let Some(request_id) = frame.request_id.clone() else {
        return Ok(());
    };
    let runtime = frame
        .value
        .get("runtime")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok());
    let response = match frame.value.get("type").and_then(serde_json::Value::as_str) {
        Some("runtime_start") => crate::ws::ConnectionResponse::RuntimeStart {
            request_id,
            success: false,
            runtime: None,
            agent: None,
            conversation: None,
            created: crate::ws::router::CreatedFlags {
                agent: false,
                conversation: false,
            },
            error: Some("runtime service unavailable".into()),
        },
        Some("sync") => crate::ws::ConnectionResponse::SyncResponse {
            request_id,
            runtime: runtime.ok_or(AppServerError::Malformed)?,
            success: false,
            error: Some("runtime service unavailable".into()),
        },
        Some("abort_message") => crate::ws::ConnectionResponse::AbortMessage {
            request_id,
            runtime: runtime.ok_or(AppServerError::Malformed)?,
            aborted: false,
            success: false,
            error: Some("runtime service unavailable".into()),
        },
        Some("input") => crate::ws::ConnectionResponse::InputAccepted {
            request_id,
            runtime: runtime.ok_or(AppServerError::Malformed)?,
            accepted: false,
            disposition: None,
            error: Some("runtime service unavailable".into()),
        },
        _ => return Ok(()),
    };
    dispatch_value(state, connection_id, &response)
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

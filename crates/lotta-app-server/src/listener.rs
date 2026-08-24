use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

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
use tokio::{
    net::TcpListener,
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

use crate::{
    auth::origin,
    bounds::{HTTP_BODY_BYTES_MAX, WS_FRAME_BYTES_MAX, WS_PING_INTERVAL_MS},
    config::{PreparedServer, is_loopback_host},
    error::AppServerError,
    heartbeat::Heartbeat,
    ws::{
        EventDeliveryBatch, RandomEventIdGenerator, RouterEventSink, RuntimeCommandService,
        RuntimeRouter, ServiceBackedTurnController, TurnController,
        UnsupportedRuntimeCommandService,
        agents::{AgentsBridge, decode as decode_agents},
        conversations::{ConversationsBridge, decode as decode_conversations},
        external_tools::{
            ExternalForwarder, ExternalToolBridge, ExternalToolsCommand, ExternalToolsMessage,
            ToolsUpdateResponseMessage, decode as decode_external_tools,
        },
        files::{FilesBridge, decode as decode_files},
        lock_router,
        memory::{MemoryBridge, decode as decode_memory},
        models::{ModelsBridge, decode as decode_models},
        route_command,
        schedules::{SchedulesBridge, decode as decode_schedules},
        settings::{SettingsBridge, decode as decode_settings},
        skills::{SkillsBridge, decode as decode_skills},
        teleport::{TeleportBridge, TeleportCommand, TeleportForwarder, decode as decode_teleport},
        terminal::{TerminalBridge, TerminalCommand, TerminalForwarder, decode as decode_terminal},
    },
};

#[cfg(test)]
#[path = "listener/tests/heartbeat.rs"]
mod heartbeat_integration;
#[cfg(test)]
#[path = "listener/tests/observer.rs"]
mod observer_tests;
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

/// Shared outbound frame sink keyed by connection identity.
type SharedOutbound = Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>;

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
    turn_controller: Arc<dyn TurnController>,
    observer: Arc<dyn crate::observer::RuntimeBroadcastObserver>,
    agents: Arc<AgentsBridge>,
    conversations: Arc<ConversationsBridge>,
    external_tools: Arc<ExternalToolBridge>,
    teleports: Arc<TeleportBridge>,
    terminals: Arc<TerminalBridge>,
    files: Arc<FilesBridge>,
    memories: Arc<MemoryBridge>,
    models: Arc<ModelsBridge>,
    schedules: Arc<SchedulesBridge>,
    skills: Arc<SkillsBridge>,
    settings: Arc<SettingsBridge>,
    next_observation: AtomicU64,
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
    let turn_controller = Arc::new(ServiceBackedTurnController::new(runtime_service.clone()));
    start_listener_with_runtime_service_and_controller(
        prepared,
        clock,
        runtime_service,
        turn_controller,
    )
    .await
}

/// Binds a listener with a Runtime service and production turn controller.
///
/// # Errors
/// Returns a stable listener error when startup fails.
pub async fn start_listener_with_runtime_service_and_controller(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    runtime_service: Arc<dyn RuntimeCommandService>,
    turn_controller: Arc<dyn TurnController>,
) -> Result<ListenerHandle, AppServerError> {
    start_listener_with_runtime_service_controller_and_observer(
        prepared,
        clock,
        runtime_service,
        turn_controller,
        Arc::new(crate::observer::InertRuntimeBroadcastObserver),
    )
    .await
}

/// Binds a listener with a Runtime service and read-only dispatch observer.
///
/// # Errors
/// Returns a stable listener error when startup fails.
pub async fn start_listener_with_runtime_service_and_observer(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    runtime_service: Arc<dyn RuntimeCommandService>,
    observer: Arc<dyn crate::observer::RuntimeBroadcastObserver>,
) -> Result<ListenerHandle, AppServerError> {
    start_listener_with_runtime_service_controller_and_observer(
        prepared,
        clock,
        runtime_service.clone(),
        Arc::new(ServiceBackedTurnController::new(runtime_service)),
        observer,
    )
    .await
}

async fn start_listener_with_runtime_service_controller_and_observer(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    runtime_service: Arc<dyn RuntimeCommandService>,
    turn_controller: Arc<dyn TurnController>,
    observer: Arc<dyn crate::observer::RuntimeBroadcastObserver>,
) -> Result<ListenerHandle, AppServerError> {
    start_listener_with_limits(
        prepared,
        clock,
        SocketLimits::default(),
        runtime_service,
        turn_controller,
        observer,
        None,
    )
    .await
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
        Arc::new(UnsupportedRuntimeCommandService),
        Arc::new(crate::observer::InertRuntimeBroadcastObserver),
        None,
    )
    .await
}

/// Command-group bridges composed once by the host process and shared with
/// the listener state.
///
/// Holding the exact [`SkillsBridge`] and [`SettingsBridge`] instances the
/// listener serves means skills enable/disable and cwd changes recorded over
/// the WebSocket are visible to the turn controller and runtime service that
/// prepare subsequent turns.
#[derive(Clone)]
pub struct SharedGroupBridges {
    outbound: Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
    skills: Arc<SkillsBridge>,
    settings: Arc<SettingsBridge>,
}

impl SharedGroupBridges {
    /// Composes the shared outbound sink plus the skills and settings bridges
    /// over one canonical storage root and its authorized workspace root.
    ///
    /// # Errors
    /// Returns a stable listener error when the settings store root is invalid.
    pub fn new(
        storage_dir: &std::path::Path,
        workspace_dir: &std::path::Path,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, AppServerError> {
        let outbound: Arc<
            std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>,
        > = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let skills = Arc::new(SkillsBridge::new(
            skills_forwarder(&outbound),
            storage_dir,
            clock,
        ));
        let settings = Arc::new(SettingsBridge::new(
            settings_forwarder(&outbound),
            storage_dir,
            workspace_dir,
        )?);
        Ok(Self {
            outbound,
            skills,
            settings,
        })
    }

    /// The shared skills bridge backing `skill_enable`/`skill_disable`.
    #[must_use]
    pub fn skills(&self) -> Arc<SkillsBridge> {
        Arc::clone(&self.skills)
    }

    /// The shared settings bridge backing cwd-map persistence.
    #[must_use]
    pub fn settings(&self) -> Arc<SettingsBridge> {
        Arc::clone(&self.settings)
    }
}

/// Binds a listener over host-composed skills/settings bridges.
///
/// The bridges (and their shared outbound sink) are the exact instances the
/// composition root handed to its controllers, so WebSocket group mutations
/// apply to subsequent turns.
///
/// # Errors
/// Returns a stable listener error when bind or bridge storage roots fail.
pub async fn start_listener_with_runtime_service_controller_observer_and_bridges(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    runtime_service: Arc<dyn RuntimeCommandService>,
    turn_controller: Arc<dyn TurnController>,
    observer: Arc<dyn crate::observer::RuntimeBroadcastObserver>,
    shared: SharedGroupBridges,
) -> Result<ListenerHandle, AppServerError> {
    start_listener_with_limits(
        prepared,
        clock,
        SocketLimits::default(),
        runtime_service,
        turn_controller,
        observer,
        Some(shared),
    )
    .await
}

/// Composes the shared skills/settings bridges plus their outbound sink when
/// the host process did not hand in its own instances.
fn compose_shared_bridges(
    prepared: &PreparedServer,
    clock: &Arc<dyn Clock + Send + Sync>,
) -> Result<SharedGroupBridges, AppServerError> {
    let outbound: SharedOutbound = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let skills = Arc::new(SkillsBridge::new(
        skills_forwarder(&outbound),
        &prepared.storage_dir,
        Arc::clone(clock),
    ));
    let settings = Arc::new(SettingsBridge::new(
        settings_forwarder(&outbound),
        &prepared.storage_dir,
        &prepared.workspace_dir,
    )?);
    Ok(SharedGroupBridges {
        outbound,
        skills,
        settings,
    })
}

async fn start_listener_with_limits(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    limits: SocketLimits,
    runtime_service: Arc<dyn RuntimeCommandService>,
    turn_controller: Arc<dyn TurnController>,
    observer: Arc<dyn crate::observer::RuntimeBroadcastObserver>,
    shared: Option<SharedGroupBridges>,
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
    let SharedGroupBridges {
        outbound,
        skills,
        settings,
    } = match shared {
        Some(shared) => shared,
        None => compose_shared_bridges(&prepared, &clock)?,
    };
    // The artifacts root backs Task 37 tool artifact operations; the files
    // group creates it on first bind next to the canonical storage root.
    let artifacts_dir = prepared.storage_dir.join("artifacts");
    let files = Arc::new(FilesBridge::new(
        files_forwarder(&outbound),
        &prepared.workspace_dir,
        &artifacts_dir,
    )?);
    let memories = Arc::new(MemoryBridge::new(
        memory_forwarder(&outbound),
        &prepared.storage_dir,
        clock.clone(),
    )?);
    let agents = Arc::new(AgentsBridge::new(
        agents_forwarder(&outbound),
        &prepared.storage_dir,
        Arc::clone(&clock),
    )?);
    let conversations = compose_conversations_bridge(&outbound, &prepared.storage_dir, &clock)?;
    let state = Arc::new(ListenerState {
        auth: prepared.auth,
        clock: Arc::clone(&clock),
        shutdown: shutdown.clone(),
        limits,
        runtime_router: Arc::new(std::sync::Mutex::new(runtime_router)),
        runtime_service,
        turn_controller,
        observer,
        external_tools: Arc::new(ExternalToolBridge::new(external_forwarder(&outbound))),
        teleports: Arc::new(TeleportBridge::new(teleport_forwarder(&outbound))),
        terminals: Arc::new(TerminalBridge::new(
            terminal_forwarder(&outbound),
            clock.clone(),
        )),
        files,
        memories,
        agents,
        conversations,
        models: Arc::new(ModelsBridge::new(
            models_forwarder(&outbound),
            &prepared.storage_dir,
            Arc::clone(&clock),
        )?),
        schedules: Arc::new(SchedulesBridge::new(
            schedules_forwarder(&outbound),
            &prepared.storage_dir,
            Arc::clone(&clock),
        )?),
        skills,
        settings,
        next_observation: AtomicU64::new(1),
        outbound,
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
    let connection_cancellation = state.shutdown.child_token();
    let mut turns = JoinSet::new();
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
                    &connection_cancellation,
                    &mut turns,
                ).await;
                if !keep_open {
                    break;
                }
            }
        }
    }
    connection_cancellation.cancel();
    while turns.join_next().await.is_some() {}
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
    state.external_tools.disconnect(id);
    state.terminals.disconnect(id);
    state.files.disconnect(id);
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
    cancellation: &CancellationToken,
    turns: &mut JoinSet<()>,
) -> bool {
    match incoming {
        Some(Ok(Message::Pong(_))) => {
            heartbeat.record_pong(state.clock.as_ref());
            true
        }
        Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await.is_ok(),
        Some(Ok(Message::Text(text))) => {
            handle_text(&text, state, connection_id, cancellation, turns).await
        }
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
    cancellation: &CancellationToken,
    turns: &mut JoinSet<()>,
) -> bool {
    let frame = match crate::framing::decode_text(text) {
        Ok(frame) => frame,
        Err(error) => return dispatch_value(state, connection_id, &error).is_ok(),
    };
    let command = match crate::ws::command::decode(&frame) {
        Ok(Some(command)) => command,
        Ok(None) => return handle_external_frame(state, connection_id, &frame),
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
    if let Some(deferred) = deferred
        && deferred.disposition == lotta_domain::InputDisposition::Started
    {
        let sink = event_sink(state);
        let Ok(command) = serde_json::from_value(frame.value.clone()) else {
            return dispatch_typed_failure(state, connection_id, &frame).is_ok();
        };
        let controller = state.turn_controller.clone();
        let state = state.clone();
        let failure_frame = frame.clone();
        let turn_cancellation = cancellation.child_token();
        turns.spawn(async move {
            let result = if controller.is_control_continuation(&deferred) {
                state
                    .runtime_service
                    .continue_input(deferred.scope, deferred.continuation, sink)
                    .await
            } else {
                controller
                    .submit_turn(command, deferred, turn_cancellation, sink)
                    .await
            };
            if result.is_err() {
                let _ = dispatch_typed_failure(&state, connection_id, &failure_frame);
            }
        });
    }
    true
}

fn event_sink(state: &Arc<ListenerState>) -> Arc<dyn crate::ws::RuntimeEventSink> {
    let observer_state = state.clone();
    Arc::new(RouterEventSink::new(
        state.runtime_router.clone(),
        Arc::new(move |scope, event, deliveries| {
            dispatch_event_batch(&observer_state, scope, event, &deliveries)
        }),
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
    for batch in output.event_batches.as_slice() {
        dispatch_event_batch(
            state,
            batch.scope.clone(),
            batch.event.clone(),
            &batch.deliveries,
        )?;
    }
    Ok(())
}

fn dispatch_value(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    value: &impl serde::Serialize,
) -> Result<(), AppServerError> {
    send_frame(&state.outbound, connection_id, value)
}

fn send_frame(
    outbound: &std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>,
    connection_id: crate::ws::ConnectionId,
    value: &impl serde::Serialize,
) -> Result<(), AppServerError> {
    let sender = outbound
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

fn external_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> ExternalForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn teleport_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> TeleportForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn terminal_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> TerminalForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn files_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> crate::ws::files::FilesForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn memory_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> crate::ws::memory::MemoryForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn models_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> crate::ws::models::ModelsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn schedules_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> crate::ws::schedules::SchedulesForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn skills_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> crate::ws::skills::SkillsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn settings_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> crate::ws::settings::SettingsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn agents_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> crate::ws::agents::AgentsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn conversations_forwarder(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
) -> crate::ws::conversations::ConversationsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

/// Composes the conversation management bridge over the canonical storage root.
fn compose_conversations_bridge(
    outbound: &Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<String>>>>,
    storage_dir: &std::path::Path,
    clock: &Arc<dyn Clock + Send + Sync>,
) -> Result<Arc<ConversationsBridge>, AppServerError> {
    Ok(Arc::new(ConversationsBridge::new(
        conversations_forwarder(outbound),
        storage_dir,
        Arc::clone(clock),
    )?))
}

/// Decodes and routes the external-tool command group for one frame.
fn handle_external_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_external_tools(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_teleport_frame(state, connection_id, frame),
        Ok(Some(command)) => route_external_command(state, connection_id, &command),
    }
}

/// Decodes and routes the teleport command group for one frame.
fn handle_teleport_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_teleport(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_terminal_frame(state, connection_id, frame),
        Ok(Some(command)) => route_teleport_command(state, connection_id, &command),
    }
}

/// Decodes and routes the terminal command group for one frame.
fn handle_terminal_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_terminal(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_files_frame(state, connection_id, frame),
        Ok(Some(command)) => route_terminal_command(state, connection_id, &command),
    }
}

/// Decodes and routes the files command group for one frame.
fn handle_files_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_files(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_memory_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.files.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the memory command group for one frame. Handlers run in
/// detached tasks; the memory bridge owns no per-connection resources, so
/// connection cleanup needs no memory step.
fn handle_memory_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_memory(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_models_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.memories.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the models/providers command group for one frame.
fn handle_models_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_models(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_schedules_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.models.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the schedules command group for one frame. Handlers run
/// in detached tasks; the schedules bridge owns no per-connection resources,
/// so connection cleanup needs no schedules step.
fn handle_schedules_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_schedules(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_skills_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.schedules.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the skills command group for one frame. Handlers run in
/// detached tasks; the skills bridge owns no per-connection resources, so
/// connection cleanup needs no skills step.
fn handle_skills_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_skills(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_settings_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.skills.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the settings command group for one frame. Handlers run
/// in detached tasks; the settings bridge owns no per-connection resources,
/// so connection cleanup needs no settings step.
fn handle_settings_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_settings(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_agents_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.settings.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the agent management command group for one frame.
/// Handlers run in detached tasks; the agents bridge owns no per-connection
/// resources, so connection cleanup needs no agents step.
fn handle_agents_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_agents(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_conversations_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.agents.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the conversation management command group for one
/// frame. Handlers run in detached tasks; the conversations bridge owns no
/// per-connection resources, so connection cleanup needs no conversations
/// step. This is the final group of the decode chain.
fn handle_conversations_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_conversations(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => true,
        Ok(Some(command)) => {
            state.conversations.handle(connection_id, &command);
            true
        }
    }
}

fn route_terminal_command(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    command: &TerminalCommand,
) -> bool {
    match command {
        TerminalCommand::Spawn(spawn) => {
            state.terminals.spawn(connection_id, spawn);
            true
        }
        TerminalCommand::Input(input) => {
            state.terminals.input(connection_id, input);
            true
        }
        TerminalCommand::Resize(resize) => {
            state.terminals.resize(connection_id, resize);
            true
        }
        TerminalCommand::Kill(kill) => {
            state.terminals.kill(connection_id, kill);
            true
        }
    }
}

fn route_teleport_command(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    command: &TeleportCommand,
) -> bool {
    match command {
        TeleportCommand::Probe(probe) => {
            state.teleports.probe(connection_id, probe);
            true
        }
        TeleportCommand::Request(request) => {
            // The compatibility listener owns no runtime registry, so no turn
            // can be processing through it; scopes answer ready immediately.
            state.teleports.request(connection_id, request, false);
            true
        }
        TeleportCommand::Failed(failure) => {
            state.teleports.failed(failure);
            true
        }
    }
}

fn route_external_command(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    command: &ExternalToolsCommand,
) -> bool {
    match command {
        ExternalToolsCommand::ToolsUpdate(update) => {
            let outcome = state.external_tools.apply_update(connection_id, update);
            let message = ExternalToolsMessage::UpdateResponse(ToolsUpdateResponseMessage {
                request_id: update.request_id.as_str().to_owned(),
                success: outcome.is_ok(),
                error: outcome.err(),
            });
            dispatch_value(state, connection_id, &message).is_ok()
        }
        ExternalToolsCommand::CallResponse(response) => {
            let _ = state
                .external_tools
                .handle_response(connection_id, response);
            true
        }
    }
}

fn dispatch_event_batch(
    state: &ListenerState,
    scope: lotta_domain::RuntimeScope,
    event: crate::ws::RuntimeEvent,
    deliveries: &EventDeliveryBatch,
) -> Result<(), AppServerError> {
    dispatch_deliveries(&state.outbound, deliveries)?;
    // Observation is post-dispatch and cannot affect routing. Once the ordinal space is exhausted,
    // retain the saturated counter and skip all later observations rather than wrapping it.
    let ordinal = state
        .next_observation
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            value.checked_add(1)
        })
        .ok();
    let Some(ordinal) = ordinal else {
        return Ok(());
    };
    let value = crate::observer::observation(ordinal, scope, event, deliveries);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.observer.observe(value);
    }));
    Ok(())
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

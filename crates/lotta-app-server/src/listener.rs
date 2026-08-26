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
        device::{DeviceBridge, decode as decode_device},
        external_tools::{
            ExternalForwarder, ExternalToolBridge, ExternalToolsCommand, ExternalToolsMessage,
            ToolsUpdateResponseMessage, decode as decode_external_tools,
        },
        files::{FilesBridge, decode as decode_files},
        introspection::{IntrospectionBridge, decode as decode_introspection},
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
#[path = "listener/tests/reconnect_security.rs"]
mod reconnect_security;
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
type OutboundBatch = Vec<String>;
type SharedOutbound =
    Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<OutboundBatch>>>>;

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
    listener_instance: String,
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
    devices: Arc<DeviceBridge>,
    introspection: Arc<IntrospectionBridge>,
    next_observation: AtomicU64,
    outbound: SharedOutbound,
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

#[cfg(test)]
async fn start_listener_with_state_for_test(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    runtime_service: Arc<dyn RuntimeCommandService>,
) -> Result<(ListenerHandle, Arc<ListenerState>), AppServerError> {
    let listener = TcpListener::bind(format_bind_address(&prepared.host, prepared.port))
        .await
        .map_err(|_| AppServerError::Listener)?;
    let address = listener
        .local_addr()
        .map_err(|_| AppServerError::Listener)?;
    let (base_url, websocket_url, openai_url) = resolved_urls(&prepared, address);
    let shutdown = CancellationToken::new();
    let path = prepared.websocket_path.clone();
    let state = compose_listener_state(
        prepared,
        &clock,
        SocketLimits {
            frame_bytes: WS_FRAME_BYTES_MAX,
            ping_interval_ms: 3_600_000,
        },
        RuntimeEndpoints::from_parts(
            runtime_service,
            Arc::new(UnsupportedRuntimeCommandService),
            Arc::new(crate::observer::InertRuntimeBroadcastObserver),
        ),
        None,
        shutdown.clone(),
    )?;
    register_device_runtime_ports(&state);
    let server_shutdown = state.shutdown.clone();
    let router = build_router(&path, Arc::clone(&state));
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_shutdown.cancelled_owned())
            .await
            .map_err(|_| AppServerError::Listener)
    });
    let handle = ListenerHandle {
        address,
        base_url,
        websocket_url,
        openai_url,
        shutdown,
        task,
    };
    Ok((handle, state))
}

/// Command-group bridges composed once by the host process and shared with
/// the listener state.
///
/// Holding the exact [`SkillsBridge`] and [`SettingsBridge`] instances the
/// listener serves means skills enable/disable and cwd changes recorded over
/// the WebSocket are visible to the turn controller and runtime service that
/// prepare subsequent turns. The optional conversation lease authority is the
/// host's production runtime state: when registered, WebSocket conversation
/// compaction shares the turn path's authoritative lifecycle registry.
#[derive(Clone)]
pub struct SharedGroupBridges {
    outbound: SharedOutbound,
    skills: Arc<SkillsBridge>,
    settings: Arc<SettingsBridge>,
    conversations_authority:
        Arc<std::sync::Mutex<Option<Arc<dyn crate::ws::conversations::ConversationAuthority>>>>,
    queue_authority: Arc<std::sync::Mutex<Option<Arc<dyn crate::ws::device::QueueAuthority>>>>,
    mod_commands:
        Arc<std::sync::Mutex<Option<Arc<lotta_extensions::mods::registry::ModRegistries>>>>,
    background_processes:
        Arc<std::sync::Mutex<Option<Arc<dyn crate::ws::device::BackgroundProcessSource>>>>,
    device_status_authority:
        Arc<std::sync::Mutex<Option<crate::ws::device::DeviceStatusAuthority>>>,
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
        let outbound: SharedOutbound = Arc::new(std::sync::Mutex::new(HashMap::new()));
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
            conversations_authority: Arc::new(std::sync::Mutex::new(None)),
            queue_authority: Arc::new(std::sync::Mutex::new(None)),
            mod_commands: Arc::new(std::sync::Mutex::new(None)),
            background_processes: Arc::new(std::sync::Mutex::new(None)),
            device_status_authority: Arc::new(std::sync::Mutex::new(None)),
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

    /// Registers the production conversation lease authority so the listener's
    /// conversations bridge acquires its compaction command leases from the
    /// authoritative runtime registry and routes through the production
    /// compaction service. Idempotent; later registrations win.
    pub fn register_conversations_authority(
        &self,
        authority: Option<Arc<dyn crate::ws::conversations::ConversationAuthority>>,
    ) {
        if let Ok(mut current) = self.conversations_authority.lock() {
            *current = authority;
        }
    }

    /// Snapshot of the registered conversation lease authority, if any.
    #[must_use]
    pub fn conversations_authority(
        &self,
    ) -> Option<Arc<dyn crate::ws::conversations::ConversationAuthority>> {
        self.conversations_authority
            .lock()
            .ok()
            .and_then(|current| current.clone())
    }

    /// Registers the authoritative queue port backing device removals.
    pub fn register_queue_authority(
        &self,
        authority: Option<Arc<dyn crate::ws::device::QueueAuthority>>,
    ) {
        if let Ok(mut current) = self.queue_authority.lock() {
            *current = authority;
        }
    }

    /// Snapshot of the registered queue authority, if any.
    #[must_use]
    pub fn queue_authority(&self) -> Option<Arc<dyn crate::ws::device::QueueAuthority>> {
        self.queue_authority
            .lock()
            .ok()
            .and_then(|current| current.clone())
    }

    /// Registers the Task 45 mod command registry backing `execute_command`.
    pub fn register_mod_commands(
        &self,
        registries: Option<Arc<lotta_extensions::mods::registry::ModRegistries>>,
    ) {
        if let Ok(mut current) = self.mod_commands.lock() {
            *current = registries;
        }
    }

    /// Snapshot of the registered mod command registry, if any.
    #[must_use]
    pub fn mod_commands(&self) -> Option<Arc<lotta_extensions::mods::registry::ModRegistries>> {
        self.mod_commands
            .lock()
            .ok()
            .and_then(|current| current.clone())
    }

    /// Registers the background-process source feeding status snapshots.
    pub fn register_background_processes(
        &self,
        source: Option<Arc<dyn crate::ws::device::BackgroundProcessSource>>,
    ) {
        if let Ok(mut current) = self.background_processes.lock() {
            *current = source;
        }
    }

    /// Snapshot of the registered background-process source, if any.
    #[must_use]
    pub fn background_processes(
        &self,
    ) -> Option<Arc<dyn crate::ws::device::BackgroundProcessSource>> {
        self.background_processes
            .lock()
            .ok()
            .and_then(|current| current.clone())
    }

    /// Registers the production device-status authority.
    pub fn register_device_status_authority(
        &self,
        authority: Option<crate::ws::device::DeviceStatusAuthority>,
    ) {
        if let Ok(mut current) = self.device_status_authority.lock() {
            *current = authority;
        }
    }

    /// Returns the registered production device-status authority.
    #[must_use]
    pub fn device_status_authority(&self) -> Option<crate::ws::device::DeviceStatusAuthority> {
        self.device_status_authority
            .lock()
            .ok()
            .and_then(|current| current.clone())
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
        conversations_authority: Arc::new(std::sync::Mutex::new(None)),
        queue_authority: Arc::new(std::sync::Mutex::new(None)),
        mod_commands: Arc::new(std::sync::Mutex::new(None)),
        background_processes: Arc::new(std::sync::Mutex::new(None)),
        device_status_authority: Arc::new(std::sync::Mutex::new(None)),
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
    let websocket_path = prepared.websocket_path.clone();
    let endpoints = RuntimeEndpoints::from_parts(runtime_service, turn_controller, observer);
    let state = compose_listener_state(
        prepared,
        &clock,
        limits,
        endpoints,
        shared,
        shutdown.clone(),
    )?;

    register_device_runtime_ports(&state);
    let router = build_router(&websocket_path, Arc::clone(&state));
    let server_shutdown = state.shutdown.clone();
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

/// Runtime command endpoints handed to every composed listener state.
struct RuntimeEndpoints {
    service: Arc<dyn RuntimeCommandService>,
    turn_controller: Arc<dyn TurnController>,
    observer: Arc<dyn crate::observer::RuntimeBroadcastObserver>,
}

impl RuntimeEndpoints {
    fn from_parts(
        runtime_service: Arc<dyn RuntimeCommandService>,
        turn_controller: Arc<dyn TurnController>,
        observer: Arc<dyn crate::observer::RuntimeBroadcastObserver>,
    ) -> Self {
        Self {
            service: runtime_service,
            turn_controller,
            observer,
        }
    }
}

/// Composes the full per-listener routing state over host-provided bridges.
///
/// The bridges handed in through `shared` are the exact instances the host's
/// controllers serve, so WebSocket group mutations reach subsequent turns.
fn compose_listener_state(
    prepared: PreparedServer,
    clock: &Arc<dyn Clock + Send + Sync>,
    limits: SocketLimits,
    endpoints: RuntimeEndpoints,
    shared: Option<SharedGroupBridges>,
    shutdown: CancellationToken,
) -> Result<Arc<ListenerState>, AppServerError> {
    let shared = match shared {
        Some(shared) => shared,
        None => compose_shared_bridges(&prepared, clock)?,
    };
    let conversations_authority = shared.conversations_authority();
    let device_ports = DevicePorts::from_shared(&shared);
    let SharedGroupBridges {
        outbound,
        skills,
        settings,
        ..
    } = shared;
    // The artifacts root backs Task 37 tool artifact operations; the files
    // group creates it on first bind next to the canonical storage root.
    let artifacts_dir = prepared.storage_dir.join("artifacts");
    let storage = compose_storage_bridges(
        &outbound,
        &prepared,
        clock,
        &artifacts_dir,
        conversations_authority,
    )?;
    let devices = compose_device_bridges(&outbound, &prepared, device_ports.queue_authority)?;
    devices.register_mod_commands_if_set(device_ports.mod_commands);
    devices.register_background_processes_if_set(device_ports.background);
    if let Some(authority) = device_ports.status_authority {
        devices.register_status_authority(authority);
    }
    Ok(Arc::new(ListenerState {
        auth: prepared.auth,
        listener_instance: listener_instance(clock),
        clock: Arc::clone(clock),
        shutdown,
        limits,
        runtime_router: Arc::new(std::sync::Mutex::new(RuntimeRouter::new(
            clock.clone(),
            Arc::new(RandomEventIdGenerator),
        ))),
        runtime_service: endpoints.service,
        turn_controller: endpoints.turn_controller,
        observer: endpoints.observer,
        external_tools: Arc::new(ExternalToolBridge::new(external_forwarder(&outbound))),
        teleports: Arc::new(TeleportBridge::new(teleport_forwarder(&outbound))),
        terminals: Arc::new(TerminalBridge::new(
            terminal_forwarder(&outbound),
            clock.clone(),
        )),
        files: storage.files,
        memories: storage.memories,
        agents: storage.agents,
        conversations: storage.conversations,
        models: storage.models,
        schedules: storage.schedules,
        skills,
        settings,
        devices,
        introspection: Arc::new(IntrospectionBridge::new(introspection_forwarder(&outbound))),
        next_observation: AtomicU64::new(1),
        outbound,
    }))
}

fn listener_instance(clock: &Arc<dyn Clock + Send + Sync>) -> String {
    format!(
        "listener-{}-{}",
        std::process::id(),
        clock
            .now()
            .as_utc()
            .timestamp_nanos_opt()
            .unwrap_or_default()
    )
}

/// Device-group ports registered by the production composition root.
struct DevicePorts {
    queue_authority: Option<Arc<dyn crate::ws::device::QueueAuthority>>,
    mod_commands: Option<Arc<lotta_extensions::mods::registry::ModRegistries>>,
    background: Option<Arc<dyn crate::ws::device::BackgroundProcessSource>>,
    status_authority: Option<crate::ws::device::DeviceStatusAuthority>,
}

impl DevicePorts {
    fn from_shared(shared: &SharedGroupBridges) -> Self {
        Self {
            queue_authority: shared.queue_authority(),
            mod_commands: shared.mod_commands(),
            background: shared.background_processes(),
            status_authority: shared.device_status_authority(),
        }
    }
}

/// Wires the late-bound device ports that need listener-owned state: the
/// router-backed event sink and subscription gate, the settings-backed cwd
/// resolver, and the built-in slash-command runner over live bridges.
fn register_device_runtime_ports(state: &Arc<ListenerState>) {
    let devices = Arc::downgrade(&state.devices);
    let snapshot_devices = Arc::downgrade(&state.devices);
    state
        .runtime_service
        .register_device_snapshot_source(Arc::new(move |connection, scope| {
            let device = snapshot_devices
                .upgrade()
                .ok_or(AppServerError::Unavailable)?;
            device.status_snapshot_for(Some(connection), scope)
        }));
    state.devices.register_event_sink(event_sink(state));
    state.devices.register_scope_gate(Arc::new({
        let runtime_router = Arc::clone(&state.runtime_router);
        move |connection| {
            lock_router(&runtime_router)
                .map(|router| router.connections.subscriptions_of(connection))
                .unwrap_or_default()
        }
    }));
    state.devices.register_cwd_resolver(Arc::new({
        let settings = Arc::clone(&state.settings);
        move |scope| {
            settings
                .cwd_for_next_turn(
                    Some(scope.agent_id.as_str()),
                    scope.conversation_id.as_str(),
                )
                .effective()
                .to_path_buf()
        }
    }));
    state
        .devices
        .register_builtin_runner(builtin_runner(Arc::clone(&state.conversations), devices));
}

/// Builds the pinned built-in slash-command runner over live bridges.
///
/// `/compact` routes through the real conversation compaction flow;
/// `/reload` re-advertises device status so refreshed registrations reach
/// clients. The remaining pinned ids have no server-side port yet and answer
/// the pinned failure shape instead of pretending success.
fn builtin_runner(
    conversations: Arc<ConversationsBridge>,
    devices: std::sync::Weak<DeviceBridge>,
) -> crate::ws::device::BuiltinRunner {
    Arc::new(move |connection, command, _cancellation| {
        let conversations = Arc::clone(&conversations);
        let devices = devices.clone();
        Box::pin(async move {
            match command.command_id.as_str() {
                "compact" => {
                    conversations
                        .compact_runtime_text(&command.runtime, command.args.as_deref())
                        .await
                }
                "reload" => {
                    if let Some(devices) = devices.upgrade() {
                        devices.refresh_status_for(connection);
                    }
                    Ok("Reloaded".to_owned())
                }
                other if crate::ws::device::BUILTIN_COMMANDS.contains(&other) => {
                    // Honest failure: these pinned ids have no server port yet.
                    Err(format!(
                        "Failed: /{other} requires listener-local capabilities \
                         this server does not provide"
                    ))
                }
                _ => Err(format!("Unknown command: {}", command.command_id)),
            }
        })
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
    let reconnect_identity = authenticated_reconnect_identity(&state, &headers);
    let connection = state.clone();
    websocket
        .max_frame_size(state.limits.frame_bytes)
        .max_message_size(state.limits.frame_bytes)
        .on_failed_upgrade(|_| {})
        .on_upgrade(move |socket| serve_socket(socket, connection, reconnect_identity))
        .into_response()
}

/// Maximum queued outbound frames for one live connection.
pub const WS_OUTBOUND_FRAMES_PER_CONNECTION_MAX: usize = 256;

const RECONNECT_CLIENT_ID_HEADER: &str = "x-lotta-reconnect-id";
pub(crate) const RECONNECT_CLIENT_ID_BYTES_MAX: usize = 256;

fn authenticated_reconnect_identity(
    state: &ListenerState,
    headers: &HeaderMap,
) -> Option<crate::ws::connection::ReconnectIdentity> {
    if state.auth.is_none() {
        return None;
    }
    let client_id = headers
        .get(RECONNECT_CLIENT_ID_HEADER)?
        .to_str()
        .ok()?
        .trim();
    if client_id.is_empty()
        || client_id.len() > RECONNECT_CLIENT_ID_BYTES_MAX
        || !client_id.is_ascii()
        || client_id.bytes().any(|byte| byte.is_ascii_control())
    {
        return None;
    }
    let principal = state.auth.principal(headers).ok()?;
    Some(crate::ws::connection::ReconnectIdentity {
        listener_instance: state.listener_instance.clone(),
        principal,
        client_id: client_id.to_owned(),
    })
}

async fn serve_socket(
    mut socket: WebSocket,
    state: Arc<ListenerState>,
    reconnect_identity: Option<crate::ws::connection::ReconnectIdentity>,
) {
    let (sender, mut receiver) = mpsc::channel(WS_OUTBOUND_FRAMES_PER_CONNECTION_MAX);
    let Ok(connection_id) = open_connection(&state, sender, reconnect_identity.as_ref()) else {
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
            Some(batch) = receiver.recv() => {
                if send_outbound_batch(&mut socket, batch).await.is_err() {
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
    close_connection(&state, connection_id).await;
}

fn open_connection(
    state: &ListenerState,
    sender: mpsc::Sender<OutboundBatch>,
    reconnect_identity: Option<&crate::ws::connection::ReconnectIdentity>,
) -> Result<crate::ws::ConnectionId, AppServerError> {
    let id = {
        let mut router = lock_router(&state.runtime_router)?;
        let id = router.connections.open_authenticated(reconnect_identity)?;
        router.connections.initialize(id)?;
        id
    };
    state.introspection.register_authenticated(id);
    let inserted = prepare_outbound(state, id, sender);
    if inserted.is_err() {
        lock_router(&state.runtime_router)?.connections.close(id);
    }
    inserted.map(|()| id)
}

fn prepare_outbound(
    state: &ListenerState,
    id: crate::ws::ConnectionId,
    sender: mpsc::Sender<OutboundBatch>,
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

async fn close_connection(state: &ListenerState, id: crate::ws::ConnectionId) {
    state.external_tools.disconnect(id);
    state.terminals.disconnect(id);
    state.files.disconnect(id);
    state.introspection.unregister(id);
    // Device work is cancellation-bound: cancel first, then reap its joins.
    state.devices.disconnect(id).await;
    if let Ok(mut outbound) = state.outbound.lock() {
        outbound.remove(&id);
    }
    if let Ok(mut router) = state.runtime_router.lock() {
        router.connections.suspend(id);
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
    if let Some(deferred) = deferred {
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

async fn send_outbound_batch(
    socket: &mut WebSocket,
    batch: OutboundBatch,
) -> Result<(), axum::Error> {
    for body in batch {
        if outbound_frame_allowed(&body) {
            socket.send(Message::Text(body.into())).await?;
        }
    }
    Ok(())
}

type OutboundFrameFilter = Arc<dyn Fn(&str) -> bool + Send + Sync>;
type SharedOutboundFrameFilter = std::sync::Mutex<Option<OutboundFrameFilter>>;

static OUTBOUND_FRAME_FILTER: std::sync::OnceLock<SharedOutboundFrameFilter> =
    std::sync::OnceLock::new();

#[doc(hidden)]
pub fn set_test_outbound_frame_filter(filter: Option<OutboundFrameFilter>) {
    let slot = OUTBOUND_FRAME_FILTER.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = filter;
    }
}

fn outbound_frame_allowed(body: &str) -> bool {
    let slot = OUTBOUND_FRAME_FILTER.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(guard) = slot.lock()
        && let Some(filter) = guard.as_ref()
    {
        return filter(body);
    }
    true
}

fn dispatch_atomic_output(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    output: &crate::ws::RouteOutput,
) -> Result<(), AppServerError> {
    let mut frames = Vec::new();
    for batch in output.event_batches.as_slice() {
        for delivery in batch.deliveries.as_slice() {
            if delivery.connection_id == connection_id {
                frames.push(
                    serde_json::to_string(&delivery.frame).map_err(|_| AppServerError::Internal)?,
                );
            }
        }
    }
    for response in output.responses.as_slice() {
        frames.push(serde_json::to_string(response).map_err(|_| AppServerError::Internal)?);
    }
    let sender = state
        .outbound
        .lock()
        .map_err(|_| AppServerError::Internal)?
        .get(&connection_id)
        .cloned()
        .ok_or(AppServerError::Unavailable)?;
    sender
        .try_send(frames)
        .map_err(|_| AppServerError::Unavailable)
}

fn dispatch_output(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    output: &crate::ws::RouteOutput,
) -> Result<(), AppServerError> {
    if output.response_after_events {
        return dispatch_atomic_output(state, connection_id, output);
    }
    dispatch_responses(state, connection_id, output)?;
    dispatch_batches(state, output)
}

fn dispatch_responses(
    state: &ListenerState,
    connection_id: crate::ws::ConnectionId,
    output: &crate::ws::RouteOutput,
) -> Result<(), AppServerError> {
    for response in output.responses.as_slice() {
        dispatch_value(state, connection_id, response)?;
    }
    Ok(())
}

fn dispatch_batches(
    state: &ListenerState,
    output: &crate::ws::RouteOutput,
) -> Result<(), AppServerError> {
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
    outbound: &std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<OutboundBatch>>>,
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
        .try_send(vec![body])
        .map_err(|_| AppServerError::Unavailable)
}

fn external_forwarder(outbound: &SharedOutbound) -> ExternalForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn teleport_forwarder(outbound: &SharedOutbound) -> TeleportForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn terminal_forwarder(outbound: &SharedOutbound) -> TerminalForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn files_forwarder(outbound: &SharedOutbound) -> crate::ws::files::FilesForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn memory_forwarder(outbound: &SharedOutbound) -> crate::ws::memory::MemoryForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn models_forwarder(outbound: &SharedOutbound) -> crate::ws::models::ModelsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn schedules_forwarder(outbound: &SharedOutbound) -> crate::ws::schedules::SchedulesForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn skills_forwarder(outbound: &SharedOutbound) -> crate::ws::skills::SkillsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn settings_forwarder(outbound: &SharedOutbound) -> crate::ws::settings::SettingsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn device_forwarder(outbound: &SharedOutbound) -> crate::ws::device::DeviceForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn introspection_forwarder(
    outbound: &SharedOutbound,
) -> crate::ws::introspection::IntrospectionForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn agents_forwarder(outbound: &SharedOutbound) -> crate::ws::agents::AgentsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

fn conversations_forwarder(
    outbound: &SharedOutbound,
) -> crate::ws::conversations::ConversationsForwarder {
    let outbound = Arc::clone(outbound);
    Arc::new(move |connection_id, message| send_frame(&outbound, connection_id, &message))
}

/// Composes the conversation management bridge over the canonical storage root.
///
/// The optional authoritative lease authority comes from host-composed shared
/// bridges, so compaction commands acquire their command leases from the same
/// lifecycle registry the production turn path uses.
fn compose_conversations_bridge(
    outbound: &SharedOutbound,
    storage_dir: &std::path::Path,
    clock: &Arc<dyn Clock + Send + Sync>,
    authority: Option<Arc<dyn crate::ws::conversations::ConversationAuthority>>,
) -> Result<Arc<ConversationsBridge>, AppServerError> {
    Ok(Arc::new(ConversationsBridge::new(
        conversations_forwarder(outbound),
        storage_dir,
        Arc::clone(clock),
        authority,
    )?))
}

/// Storage-root-backed command-group bridges composed once at startup.
struct StorageBridges {
    files: Arc<FilesBridge>,
    memories: Arc<MemoryBridge>,
    agents: Arc<AgentsBridge>,
    conversations: Arc<ConversationsBridge>,
    models: Arc<ModelsBridge>,
    schedules: Arc<SchedulesBridge>,
}

/// Composes the storage-root-backed command-group bridges once at startup.
fn compose_storage_bridges(
    outbound: &SharedOutbound,
    prepared: &PreparedServer,
    clock: &Arc<dyn Clock + Send + Sync>,
    artifacts_dir: &std::path::Path,
    authority: Option<Arc<dyn crate::ws::conversations::ConversationAuthority>>,
) -> Result<StorageBridges, AppServerError> {
    let files = Arc::new(FilesBridge::new(
        files_forwarder(outbound),
        &prepared.workspace_dir,
        artifacts_dir,
    )?);
    let memories = Arc::new(MemoryBridge::new(
        memory_forwarder(outbound),
        &prepared.storage_dir,
        Arc::clone(clock),
    )?);
    let agents = Arc::new(AgentsBridge::new(
        agents_forwarder(outbound),
        &prepared.storage_dir,
        Arc::clone(clock),
    )?);
    let conversations =
        compose_conversations_bridge(outbound, &prepared.storage_dir, clock, authority)?;
    let models = Arc::new(ModelsBridge::new(
        models_forwarder(outbound),
        &prepared.storage_dir,
        Arc::clone(clock),
    )?);
    let schedules = Arc::new(SchedulesBridge::new(
        schedules_forwarder(outbound),
        &prepared.storage_dir,
        Arc::clone(clock),
    )?);
    Ok(StorageBridges {
        files,
        memories,
        agents,
        conversations,
        models,
        schedules,
    })
}

/// Composes the device bridge over canonical roots with the host-registered
/// queue authority, and returns it alongside its introspection sibling.
///
/// # Errors
/// Returns a stable listener error when the workspace root is not absolute.
fn compose_device_bridges(
    outbound: &SharedOutbound,
    prepared: &PreparedServer,
    queue_authority: Option<Arc<dyn crate::ws::device::QueueAuthority>>,
) -> Result<Arc<DeviceBridge>, AppServerError> {
    let devices = Arc::new(DeviceBridge::new(
        device_forwarder(outbound),
        &prepared.workspace_dir,
        &prepared.storage_dir,
    )?);
    if let Some(authority) = queue_authority {
        devices.register_queue_authority(authority);
    }
    Ok(devices)
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
/// per-connection resources, so connection cleanup needs no conversations step.
fn handle_conversations_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_conversations(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_device_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.conversations.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the device command group for one frame. Handlers run in
/// detached tasks; the device bridge owns no per-connection resources, so
/// connection cleanup needs no device step.
fn handle_device_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_device(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => handle_introspection_frame(state, connection_id, frame),
        Ok(Some(command)) => {
            state.devices.handle(connection_id, &command);
            true
        }
    }
}

/// Decodes and routes the introspection group for one frame. This is the
/// final group of the decode chain.
fn handle_introspection_frame(
    state: &Arc<ListenerState>,
    connection_id: crate::ws::ConnectionId,
    frame: &crate::framing::DecodedFrame,
) -> bool {
    match decode_introspection(frame) {
        Err(error) => dispatch_value(state, connection_id, &error).is_ok(),
        Ok(None) => true,
        Ok(Some(command)) => state.introspection.handle(connection_id, &command),
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
    outbound: &std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<OutboundBatch>>>,
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
            .try_send(vec![body])
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

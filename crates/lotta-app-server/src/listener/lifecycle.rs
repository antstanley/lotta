use super::{
    AgentsBridge, AppServerError, Arc, AtomicU64, CancellationToken, Clock, ConversationsBridge,
    DeviceBridge, ExternalToolBridge, FilesBridge, HashMap, IntrospectionBridge, JoinHandle,
    MemoryBridge, ModelsBridge, PreparedServer, RandomEventIdGenerator, Router,
    RuntimeCommandService, RuntimeRouter, SchedulesBridge, ServiceBackedTurnController,
    SettingsBridge, SkillsBridge, SocketAddr, StorageBridges, TcpListener, TeleportBridge,
    TerminalBridge, TurnController, UnsupportedRuntimeCommandService, WS_FRAME_BYTES_MAX,
    WS_PING_INTERVAL_MS, build_router, compose_device_bridges, compose_storage_bridges, event_sink,
    external_forwarder, format_bind_address, introspection_forwarder, is_loopback_host,
    lock_router, mpsc, publish_expired_subscription_updates, publish_shutdown_subscription_updates,
    resolved_urls, settings_forwarder, skills_forwarder, teleport_forwarder, terminal_forwarder,
    turn_supervisor,
};

/// Shared outbound frame sink keyed by connection identity.
pub(super) type OutboundBatch = Vec<String>;
pub(super) type SharedOutbound =
    Arc<std::sync::Mutex<HashMap<crate::ws::ConnectionId, mpsc::Sender<OutboundBatch>>>>;

pub(super) struct SocketLimits {
    pub(super) frame_bytes: usize,
    pub(super) ping_interval_ms: u64,
}

impl Default for SocketLimits {
    fn default() -> Self {
        Self {
            frame_bytes: WS_FRAME_BYTES_MAX,
            ping_interval_ms: WS_PING_INTERVAL_MS,
        }
    }
}

pub(super) struct ListenerState {
    pub(super) auth: crate::auth::AuthPolicy,
    pub(super) openai_api: bool,
    pub(super) channel_host_protocol_only: bool,
    pub(super) channel_session: Option<crate::auth::channel_session::ChannelSessionAuthenticator>,
    pub(super) listener_instance: String,
    pub(super) clock: Arc<dyn Clock + Send + Sync>,
    pub(super) shutdown: CancellationToken,
    pub(super) limits: SocketLimits,
    pub(super) runtime_router: Arc<std::sync::Mutex<RuntimeRouter>>,
    pub(super) runtime_service: Arc<dyn RuntimeCommandService>,
    pub(super) turn_controller: Arc<dyn TurnController>,
    pub(super) turns: turn_supervisor::RuntimeTurnSupervisor,
    pub(super) observer: Arc<dyn crate::observer::RuntimeBroadcastObserver>,
    pub(super) agents: Arc<AgentsBridge>,
    pub(super) conversations: Arc<ConversationsBridge>,
    pub(super) external_tools: Arc<ExternalToolBridge>,
    pub(super) channel_tools: Option<Arc<lotta_tools::external::ChannelExternalToolManager>>,
    pub(super) teleports: Arc<TeleportBridge>,
    pub(super) terminals: Arc<TerminalBridge>,
    pub(super) files: Arc<FilesBridge>,
    pub(super) memories: Arc<MemoryBridge>,
    pub(super) models: Arc<ModelsBridge>,
    pub(super) schedules: Arc<SchedulesBridge>,
    pub(super) skills: Arc<SkillsBridge>,
    pub(super) settings: Arc<SettingsBridge>,
    pub(super) devices: Arc<DeviceBridge>,
    pub(super) introspection: Arc<IntrospectionBridge>,
    pub(super) openai_chat: Arc<crate::openai::chat::ChatState>,
    pub(super) openai_responses: Arc<crate::openai::responses::ResponsesState>,
    pub(super) next_observation: AtomicU64,
    pub(super) outbound: SharedOutbound,
}

#[cfg(test)]
pub(super) fn test_openai_chat(
    agents: &Arc<AgentsBridge>,
    conversations: &Arc<ConversationsBridge>,
    clock: Arc<dyn Clock + Send + Sync>,
    shutdown: CancellationToken,
) -> Arc<crate::openai::chat::ChatState> {
    let service: Arc<dyn RuntimeCommandService> = Arc::new(UnsupportedRuntimeCommandService);
    let controller: Arc<dyn TurnController> = Arc::new(UnsupportedRuntimeCommandService);
    Arc::new(crate::openai::chat::ChatState::new(
        Arc::clone(agents),
        Arc::clone(conversations),
        service,
        controller,
        clock,
        shutdown,
    ))
}

#[cfg(test)]
pub(super) fn test_openai_responses(
    agents: &Arc<AgentsBridge>,
    conversations: &Arc<ConversationsBridge>,
    clock: Arc<dyn Clock + Send + Sync>,
    shutdown: CancellationToken,
) -> Arc<crate::openai::responses::ResponsesState> {
    Arc::new(crate::openai::responses::ResponsesState::new(
        test_openai_chat(agents, conversations, clock, shutdown),
    ))
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

pub(super) async fn start_listener_with_runtime_service_controller_and_observer(
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
pub(super) async fn start_listener_for_test(
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
pub(crate) async fn start_listener_with_responses_for_test(
    prepared: PreparedServer,
    clock: Arc<dyn Clock + Send + Sync>,
    runtime_service: Arc<dyn RuntimeCommandService>,
    turn_controller: Arc<dyn TurnController>,
    repository: Option<Arc<dyn crate::ws::conversations::ConversationCommandRepository>>,
    owner_capacity: usize,
) -> Result<
    (
        ListenerHandle,
        Arc<crate::openai::responses::ResponsesState>,
    ),
    AppServerError,
> {
    let listener = TcpListener::bind(format_bind_address(&prepared.host, prepared.port))
        .await
        .map_err(|_| AppServerError::Listener)?;
    let address = listener
        .local_addr()
        .map_err(|_| AppServerError::Listener)?;
    let (base_url, websocket_url, openai_url) = resolved_urls(&prepared, address);
    let shutdown = CancellationToken::new();
    let path = prepared.websocket_path.clone();
    let endpoints = RuntimeEndpoints::from_parts(
        runtime_service,
        turn_controller,
        Arc::new(crate::observer::InertRuntimeBroadcastObserver),
    );
    let mut state = compose_listener_state(
        prepared,
        &clock,
        SocketLimits::default(),
        endpoints,
        None,
        shutdown.clone(),
    )?;
    let repository = repository.unwrap_or_else(|| state.openai_chat.conversations.clone());
    let responses = Arc::new(
        crate::openai::responses::ResponsesState::with_repository_and_capacity(
            Arc::clone(&state.openai_chat),
            repository,
            owner_capacity,
        ),
    );
    Arc::get_mut(&mut state)
        .ok_or(AppServerError::Internal)?
        .openai_responses = Arc::clone(&responses);
    register_device_runtime_ports(&state);
    let router = build_router(&path, Arc::clone(&state));
    let task = tokio::spawn(run_listener(listener, router, state));
    let handle = ListenerHandle {
        address,
        base_url,
        websocket_url,
        openai_url,
        shutdown,
        task,
    };
    Ok((handle, responses))
}

#[cfg(test)]
pub(super) async fn start_listener_with_state_for_test(
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
    let router = build_router(&path, Arc::clone(&state));
    let server_state = Arc::clone(&state);
    let task = tokio::spawn(run_listener(listener, router, server_state));
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
    channel_tools:
        Arc<std::sync::Mutex<Option<Arc<lotta_tools::external::ChannelExternalToolManager>>>>,
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
            channel_tools: Arc::new(std::sync::Mutex::new(None)),
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

    /// Registers the canonical channel manager consumed by production turn setup.
    pub fn register_channel_tools(
        &self,
        manager: Option<Arc<lotta_tools::external::ChannelExternalToolManager>>,
    ) {
        if let Ok(mut current) = self.channel_tools.lock() {
            *current = manager;
        }
    }

    /// Returns the canonical channel manager when production composition installed it.
    #[must_use]
    pub fn channel_tools(&self) -> Option<Arc<lotta_tools::external::ChannelExternalToolManager>> {
        self.channel_tools
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
pub(super) fn compose_shared_bridges(
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
        channel_tools: Arc::new(std::sync::Mutex::new(None)),
    })
}

pub(super) async fn start_listener_with_limits(
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
    let task = tokio::spawn(run_listener(listener, router, state));
    Ok(ListenerHandle {
        address,
        base_url,
        websocket_url,
        openai_url,
        shutdown,
        task,
    })
}

pub(super) const SUBSCRIPTION_LEASE_SWEEP_SECONDS: u64 = 1;

pub(super) async fn run_listener(
    listener: TcpListener,
    router: Router,
    state: Arc<ListenerState>,
) -> Result<(), AppServerError> {
    let shutdown = state.shutdown.clone();
    let server = std::future::IntoFuture::into_future(
        axum::serve(listener, router).with_graceful_shutdown(shutdown.cancelled_owned()),
    );
    tokio::pin!(server);
    let mut sweep = tokio::time::interval(std::time::Duration::from_secs(
        SUBSCRIPTION_LEASE_SWEEP_SECONDS,
    ));
    sweep.tick().await;
    let server_result = loop {
        tokio::select! {
            result = &mut server => break result.map_err(|_| AppServerError::Listener),
            _ = sweep.tick() => publish_expired_subscription_updates(&state).await,
        }
    };
    state.shutdown.cancel();
    state.openai_responses.shutdown().await;
    state.openai_chat.shutdown().await;
    let turn_result = state.turns.shutdown().await;
    publish_shutdown_subscription_updates(&state).await;
    server_result.and(turn_result)
}

/// Runtime command endpoints handed to every composed listener state.
pub(super) struct RuntimeEndpoints {
    pub(super) service: Arc<dyn RuntimeCommandService>,
    pub(super) turn_controller: Arc<dyn TurnController>,
    pub(super) observer: Arc<dyn crate::observer::RuntimeBroadcastObserver>,
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
pub(super) fn compose_listener_state(
    prepared: PreparedServer,
    clock: &Arc<dyn Clock + Send + Sync>,
    limits: SocketLimits,
    endpoints: RuntimeEndpoints,
    shared: Option<SharedGroupBridges>,
    shutdown: CancellationToken,
) -> Result<Arc<ListenerState>, AppServerError> {
    let shared = resolve_shared_bridges(shared, &prepared, clock)?;
    let conversations_authority = shared.conversations_authority();
    let channel_tools = shared.channel_tools();
    let mut device_ports = DevicePorts::from_shared(&shared);
    let SharedGroupBridges {
        outbound,
        skills,
        settings,
        ..
    } = shared;
    let storage = compose_listener_storage(&outbound, &prepared, clock, conversations_authority)?;
    let queue_authority = device_ports.queue_authority.take();
    let devices = compose_device_bridges(&outbound, &prepared, queue_authority)?;
    register_composed_device_ports(&devices, device_ports);
    let turns = turn_supervisor::RuntimeTurnSupervisor::new(shutdown.clone());
    let (openai_chat, openai_responses) =
        compose_openai_states(&storage, &endpoints, clock, &shutdown);
    let listener_instance = bind_listener_instance(&prepared, clock)?;
    Ok(Arc::new(ListenerState {
        auth: prepared.auth,
        openai_api: prepared.openai_api,
        channel_host_protocol_only: prepared.channel_host_protocol_only,
        channel_session: prepared.channel_session,
        listener_instance,
        clock: Arc::clone(clock),
        shutdown,
        limits,
        runtime_router: compose_runtime_router(clock),
        runtime_service: endpoints.service,
        turn_controller: endpoints.turn_controller,
        turns,
        observer: endpoints.observer,
        external_tools: Arc::new(ExternalToolBridge::new(external_forwarder(&outbound))),
        channel_tools,
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
        openai_chat,
        openai_responses,
        next_observation: AtomicU64::new(1),
        outbound,
    }))
}

fn compose_listener_storage(
    outbound: &SharedOutbound,
    prepared: &PreparedServer,
    clock: &Arc<dyn Clock + Send + Sync>,
    authority: Option<Arc<dyn crate::ws::conversations::ConversationAuthority>>,
) -> Result<StorageBridges, AppServerError> {
    let artifacts_dir = prepared.storage_dir.join("artifacts");
    compose_storage_bridges(outbound, prepared, clock, &artifacts_dir, authority)
}

fn bind_listener_instance(
    prepared: &PreparedServer,
    clock: &Arc<dyn Clock + Send + Sync>,
) -> Result<String, AppServerError> {
    let instance = listener_instance(clock);
    if let Some(authenticator) = &prepared.channel_session {
        authenticator.bind_listener(&prepared.host, &prepared.websocket_path, &instance)?;
    }
    Ok(instance)
}

pub(super) fn compose_openai_states(
    storage: &StorageBridges,
    endpoints: &RuntimeEndpoints,
    clock: &Arc<dyn Clock + Send + Sync>,
    shutdown: &CancellationToken,
) -> (
    Arc<crate::openai::chat::ChatState>,
    Arc<crate::openai::responses::ResponsesState>,
) {
    let chat = Arc::new(crate::openai::chat::ChatState::new(
        Arc::clone(&storage.agents),
        Arc::clone(&storage.conversations),
        Arc::clone(&endpoints.service),
        Arc::clone(&endpoints.turn_controller),
        Arc::clone(clock),
        shutdown.clone(),
    ));
    let responses = Arc::new(crate::openai::responses::ResponsesState::new(Arc::clone(
        &chat,
    )));
    (chat, responses)
}

pub(super) fn compose_runtime_router(
    clock: &Arc<dyn Clock + Send + Sync>,
) -> Arc<std::sync::Mutex<RuntimeRouter>> {
    Arc::new(std::sync::Mutex::new(RuntimeRouter::new(
        Arc::clone(clock),
        Arc::new(RandomEventIdGenerator),
    )))
}

pub(super) fn resolve_shared_bridges(
    shared: Option<SharedGroupBridges>,
    prepared: &PreparedServer,
    clock: &Arc<dyn Clock + Send + Sync>,
) -> Result<SharedGroupBridges, AppServerError> {
    match shared {
        Some(shared) => Ok(shared),
        None => compose_shared_bridges(prepared, clock),
    }
}

pub(super) fn register_composed_device_ports(devices: &Arc<DeviceBridge>, ports: DevicePorts) {
    devices.register_mod_commands_if_set(ports.mod_commands);
    devices.register_background_processes_if_set(ports.background);
    if let Some(authority) = ports.status_authority {
        devices.register_status_authority(authority);
    }
}

pub(super) fn listener_instance(clock: &Arc<dyn Clock + Send + Sync>) -> String {
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
pub(super) struct DevicePorts {
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
pub(super) fn register_device_runtime_ports(state: &Arc<ListenerState>) {
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
pub(super) fn builtin_runner(
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

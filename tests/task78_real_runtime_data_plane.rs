use lotta_app_server::{
    config::ServerArgs,
    error::AppServerError,
    listener::{
        SharedGroupBridges, start_listener_with_runtime_service_controller_observer_and_bridges,
    },
    observer::InertRuntimeBroadcastObserver,
    ws::{
        InputAdmissionWork, RuntimeCommandService, RuntimeEvent, RuntimeEventSink, TurnController,
        command::{
            AbortMessageCommand, ChangeDeviceStateCommand, InputCommand, RuntimeStartCommand,
            SyncCommand,
        },
        event::{LoopState, LoopStatus, StreamDelta},
        service::{
            AbortOutcome, DeviceStateOutcome, InputAdmission, RuntimeEventBatch,
            RuntimeStartOutcome, ServiceFuture, SyncOutcome,
        },
    },
};
use lotta_channels::{
    adapter::{
        AdapterFactoryContext, AdapterRuntimeEvent, CanonicalRouteKey, ChannelAdapterError,
        ChannelAdapterFactory, ChannelRuntimeClient, HostContext, InboundChannelMessage,
        MessageChannelResult,
    },
    control_plane::{ChildFrame, ControlPlane, FrameMetadata, ParentFrame, read_line, write_line},
    topology::ChannelStore,
};
use lotta_domain::{
    AgentId, BoundedJsonValue, Clock, ConversationId, InputDisposition, NonEmptyString,
    RuntimeScope, Timestamp,
};
use lotta_runtime::{
    CompactionMode,
    boundary::ProviderName,
    ports::{ProviderRequest, ToolApprovalGrant, ToolCallId, ToolOutcome},
    turn::CompactionProgress,
};
use lotta_tools::{
    AllowAllPermissions, AllowAllSandbox, OutcomeSink, PipelineError, PipelineRequest,
    SecretResolver, ToolRegistry, ToolsetId, TraceEvent, TraceSink, execute,
    external::{ChannelExternalToolManager, ChannelRuntimeKey},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::BufReader,
    sync::{Notify, oneshot},
};
use tokio_util::sync::CancellationToken;

const DEADLINE: Duration = Duration::from_secs(5);
static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

struct FixedClock;
impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp::parse_persisted_rfc3339("2026-08-30T00:00:00Z").unwrap()
    }
    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, lotta_domain::DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

#[derive(Default)]
struct CapturingFactory {
    clients: Mutex<BTreeMap<CanonicalRouteKey, ChannelRuntimeClient>>,
    attached: Notify,
}
impl CapturingFactory {
    async fn clients(&self) -> BTreeMap<CanonicalRouteKey, ChannelRuntimeClient> {
        loop {
            if let Ok(clients) = self.clients.lock()
                && clients.len() == 2
            {
                return clients.clone();
            }
            self.attached.notified().await;
        }
    }
}
impl ChannelAdapterFactory for CapturingFactory {
    fn start(
        &self,
        context: AdapterFactoryContext,
        runtime: ChannelRuntimeClient,
    ) -> Result<(), ChannelAdapterError> {
        self.clients
            .lock()
            .map_err(|_| ChannelAdapterError::Unavailable)?
            .insert(CanonicalRouteKey::from_route(&context.route), runtime);
        self.attached.notify_waiters();
        Ok(())
    }
}

struct DataPlaneService {
    starts: AtomicU64,
}
impl DataPlaneService {
    fn new() -> Self {
        Self {
            starts: AtomicU64::new(0),
        }
    }
}
impl RuntimeCommandService for DataPlaneService {
    fn runtime_start(
        &self,
        _: lotta_app_server::ws::ConnectionId,
        command: RuntimeStartCommand,
    ) -> ServiceFuture<'_, RuntimeStartOutcome> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let scope = RuntimeScope::new(
            AgentId::accept(command.agent_id.expect("agent id").as_str()).unwrap(),
            ConversationId::accept(command.conversation_id.expect("conversation id").as_str())
                .unwrap(),
            None,
        );
        Box::pin(async move {
            Ok(RuntimeStartOutcome {
                runtime: scope,
                created_agent: false,
                created_conversation: false,
                agent: None,
                conversation: None,
                broadcasts: event_batch(Vec::new()),
            })
        })
    }

    fn admit_input(&self, command: InputCommand) -> ServiceFuture<'_, InputAdmission> {
        let continuation = command.payload.clone();
        Box::pin(async move {
            Ok(InputAdmission {
                disposition: InputDisposition::Started,
                error: None,
                continuation: Some(continuation.clone()),
                work: InputAdmissionWork::NewStarted(continuation),
                after_ack: event_batch(Vec::new()),
            })
        })
    }

    fn continue_input(
        &self,
        _: RuntimeScope,
        _: Option<BoundedJsonValue>,
        _: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()> {
        Box::pin(async { Err(AppServerError::Unavailable) })
    }

    fn compact(
        &self,
        _: RuntimeScope,
        _: CompactionMode,
        _: NonEmptyString,
        _: ProviderRequest,
    ) -> ServiceFuture<'_, CompactionProgress> {
        Box::pin(async { Err(AppServerError::Unavailable) })
    }

    fn sync(
        &self,
        _: lotta_app_server::ws::ConnectionId,
        _: SyncCommand,
    ) -> ServiceFuture<'_, SyncOutcome> {
        Box::pin(async {
            Ok(SyncOutcome {
                broadcasts: event_batch(Vec::new()),
            })
        })
    }

    fn abort_message(&self, _: AbortMessageCommand) -> ServiceFuture<'_, AbortOutcome> {
        Box::pin(async { Ok(AbortOutcome { aborted: true }) })
    }

    fn change_device_state(
        &self,
        _: ChangeDeviceStateCommand,
    ) -> ServiceFuture<'_, DeviceStateOutcome> {
        Box::pin(async {
            Ok(DeviceStateOutcome {
                broadcasts: event_batch(Vec::new()),
            })
        })
    }
}

struct DataPlaneTurns {
    manager: Arc<ChannelExternalToolManager>,
    registry: Arc<ToolRegistry>,
    tool_results: Arc<Mutex<Vec<String>>>,
}
impl TurnController for DataPlaneTurns {
    fn submit_turn(
        &self,
        command: InputCommand,
        deferred: lotta_app_server::ws::DeferredInput,
        _: CancellationToken,
        sink: Arc<dyn RuntimeEventSink>,
    ) -> ServiceFuture<'_, ()> {
        let manager = Arc::clone(&self.manager);
        let registry = Arc::clone(&self.registry);
        let results = Arc::clone(&self.tool_results);
        let request_id = command
            .request_id
            .expect("input request id")
            .as_str()
            .to_owned();
        let client_message_id = command.payload.as_value()["messages"][0]["client_message_id"]
            .as_str()
            .expect("client message id");
        let route_number = if client_message_id == "route1-inbound" {
            1
        } else {
            2
        };
        Box::pin(async move {
            sink.emit(
                &deferred.scope,
                RuntimeEvent::StreamDelta {
                    delta: StreamDelta::Other(
                        BoundedJsonValue::new(serde_json::json!({
                            "kind": "assistant", "text": format!("delta-{route_number}")
                        }))
                        .unwrap(),
                    ),
                    subagent_id: None,
                },
            )?;

            let runtime = ChannelRuntimeKey {
                agent_id: deferred.scope.agent_id.as_str().to_owned(),
                conversation_id: deferred.scope.conversation_id.as_str().to_owned(),
            };
            let registrations = manager
                .runtime_registrations(&runtime)
                .expect("MessageChannel registration");
            assert_eq!(registrations.len(), 1);
            assert_eq!(
                registrations[0].definition.model_name.as_str(),
                "MessageChannel"
            );
            let snapshot = registry
                .compose(ToolsetId::None, &registrations, None)
                .expect("production registry merge");
            let output = execute_message_channel(snapshot, route_number).await;
            results.lock().unwrap().push(output);

            sink.emit(
                &deferred.scope,
                RuntimeEvent::UpdateLoopStatus {
                    loop_status: LoopState {
                        status: LoopStatus::WaitingOnInput,
                        active_run_ids: vec![request_id.clone()],
                        executing_tool_call_ids: Vec::new(),
                    },
                },
            )?;
            if route_number == 1 {
                return Err(AppServerError::Unavailable);
            }
            sink.emit(
                &deferred.scope,
                RuntimeEvent::TurnFinished {
                    turn_id: NonEmptyString::new("turn-route-2".to_owned()).unwrap(),
                    run_id: None,
                    stop_reason: NonEmptyString::new("end_turn".to_owned()).unwrap(),
                    error: None,
                },
            )
        })
    }
}

async fn execute_message_channel(
    snapshot: Arc<lotta_tools::RegistrySnapshot>,
    route_number: u64,
) -> String {
    let trace = IgnoreTrace;
    let sink = IgnoreOutcome;
    let result = execute(PipelineRequest {
        tool_call_id: ToolCallId::from_name(
            ProviderName::new(format!("tool-call-{route_number}")).unwrap(),
        ),
        approval_grant: ToolApprovalGrant::None,
        registry: snapshot,
        model_name: "MessageChannel",
        input: BoundedJsonValue::new(serde_json::json!({
            "message": format!("outbound-{route_number}")
        }))
        .unwrap(),
        cancellation: CancellationToken::new(),
        hook_runtime: &lotta_runtime::hooks::NoopHookRuntime,
        permissions: &AllowAllPermissions,
        sandbox: &AllowAllSandbox,
        secrets: &NoSecrets,
        trace: &trace,
        overflow: &NoOverflow,
        persistence: &sink,
        emit: &sink,
    })
    .await
    .expect("canonical MessageChannel pipeline");
    match result {
        ToolOutcome::Success { content } => content.as_str().to_owned(),
        other => panic!("MessageChannel failed: {other:?}"),
    }
}

struct IgnoreTrace;
impl TraceSink for IgnoreTrace {
    fn record(&self, _: TraceEvent) {}
}
struct IgnoreOutcome;
impl OutcomeSink for IgnoreOutcome {
    fn record(&self, _: &str, _: &ToolOutcome) -> Result<(), PipelineError> {
        Ok(())
    }
}
struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn resolve(&self, _: &str) -> Result<Option<String>, PipelineError> {
        Ok(None)
    }
}
struct NoOverflow;
impl lotta_tools::clamp::OverflowWriter for NoOverflow {
    fn write(&self, _: &str, _: &str) -> Result<String, lotta_tools::clamp::ClampError> {
        Ok("unused".to_owned())
    }
}

fn event_batch(events: Vec<RuntimeEvent>) -> RuntimeEventBatch {
    RuntimeEventBatch::new(events).expect("bounded runtime events")
}

fn fixture() -> (PathBuf, ChannelStore) {
    let home = std::env::temp_dir().join(format!(
        "task78-real-data-plane-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::create_dir_all(&home).unwrap();
    let store = ChannelStore::under_letta_home(&home).unwrap();
    for (channel, account, chat, thread) in [
        (
            "task78-alpha",
            "account-alpha",
            "chat-alpha",
            "thread-alpha",
        ),
        ("task78-beta", "account-beta", "chat-beta", "thread-beta"),
    ] {
        let root = store.root().join(channel);
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("config.yaml"), "enabled: true\n").unwrap();
        std::fs::write(
            root.join("accounts.json"),
            serde_json::json!({"accounts":[{
                "channel": channel, "accountId": account, "enabled": true,
                "dmPolicy": "pairing", "allowedUsers": [], "config": {"adapter": "fake"},
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            }]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join("routing.yaml"),
            serde_json::json!({"routes":[{
                "accountId": account, "chatId": chat, "threadId": thread,
                "agentId": "agent-task78", "conversationId": "conversation-task78",
                "enabled": true, "createdAt": "2026-01-01T00:00:00Z",
                "updatedAt": "2026-01-01T00:00:00Z"
            }]})
            .to_string(),
        )
        .unwrap();
    }
    (home, store)
}

async fn run_management_parent(
    stream: tokio::io::DuplexStream,
    bootstrap: ParentFrame,
    manager: Arc<ChannelExternalToolManager>,
    mut shutdown: oneshot::Receiver<()>,
) -> Vec<&'static str> {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let mut plane = ControlPlane::new("task78-owner".to_owned(), 1, manager).unwrap();
    plane
        .register_parent_request("bootstrap-task78", 10_000, 100)
        .unwrap();
    write_line(&mut writer, &bootstrap).await.unwrap();
    let mut trace = Vec::new();
    let mut shutdown_sent = false;
    loop {
        tokio::select! {
            result = read_line::<_, ChildFrame>(&mut reader) => {
                let frame = result.unwrap().expect("child management frame");
                let label = match &frame {
                    ChildFrame::Ready { .. } => "ready",
                    ChildFrame::Channels { .. } => "channels",
                    ChildFrame::PublishRuntimeTools { .. } => "publish",
                    ChildFrame::ReleaseRuntimeTools { .. } => "release",
                    ChildFrame::ShutdownComplete { .. } => "shutdown_complete",
                };
                trace.push(label);
                let response = plane.dispatch_at(frame, &[], 101).unwrap();
                if let Some(response) = response {
                    let _ = write_line(&mut writer, &response).await;
                    plane.complete_parent_response_at(&response, 102).unwrap();
                }
                if label == "shutdown_complete" { break; }
            }
            _ = &mut shutdown, if !shutdown_sent => {
                shutdown_sent = true;
                plane.register_parent_request("shutdown-task78", 10_000, 103).unwrap();
                write_line(&mut writer, &ParentFrame::Shutdown {
                    metadata: FrameMetadata::new(1),
                    owner: "task78-owner".to_owned(),
                    request_id: "shutdown-task78".to_owned(),
                }).await.unwrap();
            }
        }
    }
    assert_eq!(plane.in_flight_len(), 0);
    trace
}

async fn next_event(client: &ChannelRuntimeClient) -> serde_json::Value {
    let event: AdapterRuntimeEvent = tokio::time::timeout(DEADLINE, client.receive_event())
        .await
        .expect("adapter event deadline")
        .expect("adapter event channel");
    serde_json::to_value(event).unwrap()
}

#[tokio::test]
async fn task78_real_runtime_data_plane() {
    let (home, store) = fixture();
    let channels_root = store.root().to_path_buf();
    let server_storage = home.join("server");
    let server_workspace = home.join("workspace");
    let authenticator = lotta_app_server::auth::channel_session::ChannelSessionAuthenticator::new();
    let prepared = ServerArgs {
        listen: Some("ws://127.0.0.1:0/channel-runtime".to_owned()),
        listen_enabled: true,
        storage_dir: Some(server_storage.clone()),
        workspace_dir: Some(server_workspace.clone()),
        ..ServerArgs::default()
    }
    .prepare()
    .unwrap()
    .for_channel_session(authenticator.clone())
    .unwrap();

    let registry = Arc::new(ToolRegistry::new([]).unwrap());
    let manager = Arc::new(ChannelExternalToolManager::new(Arc::clone(&registry)));
    let bridges =
        SharedGroupBridges::new(&server_storage, &server_workspace, Arc::new(FixedClock)).unwrap();
    bridges.register_channel_tools(Some(Arc::clone(&manager)));
    let service = Arc::new(DataPlaneService::new());
    let tool_results = Arc::new(Mutex::new(Vec::new()));
    let turns = Arc::new(DataPlaneTurns {
        manager: Arc::clone(&manager),
        registry: Arc::clone(&registry),
        tool_results: Arc::clone(&tool_results),
    });
    let mut listener = start_listener_with_runtime_service_controller_observer_and_bridges(
        prepared,
        Arc::new(FixedClock),
        service.clone(),
        turns,
        Arc::new(InertRuntimeBroadcastObserver),
        bridges,
    )
    .await
    .unwrap();

    let capability = authenticator
        .install_scoped(
            "task78-owner",
            1,
            std::process::id(),
            channels_root.to_str().unwrap(),
            &[("agent-task78".to_owned(), "conversation-task78".to_owned())],
        )
        .unwrap();
    let bootstrap = ParentFrame::Bootstrap {
        metadata: FrameMetadata::new(1),
        request_id: "bootstrap-task78".to_owned(),
        owner: "task78-owner".to_owned(),
        websocket_url: listener.websocket_url().to_owned(),
        token: capability.expose_for_pipe().to_owned(),
        channels_root: channels_root.to_str().unwrap().to_owned(),
    };

    let factory = Arc::new(CapturingFactory::default());
    let mut context = HostContext::production();
    context
        .register_factory("task78-alpha", factory.clone())
        .unwrap();
    context
        .register_factory("task78-beta", factory.clone())
        .unwrap();
    let (host_management, parent_management) = tokio::io::duplex(256 * 1024);
    let (host_reader, host_writer) = tokio::io::split(host_management);
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let parent = tokio::spawn(run_management_parent(
        parent_management,
        bootstrap,
        Arc::clone(&manager),
        shutdown_receiver,
    ));
    let mut host = tokio::spawn(lotta_channels::host::run_composed_with_management(
        context,
        host_reader,
        host_writer,
    ));

    let clients = tokio::select! {
        clients = factory.clients() => clients,
        result = &mut host => panic!("host exited before adapter composition: {result:?}"),
        () = tokio::time::sleep(DEADLINE) => panic!("adapter composition deadline"),
    };
    assert_eq!(clients.len(), 2);
    let mut ordered = clients.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(key, _)| key.channel_id.clone());
    let (route1_key, route1) = ordered[0];
    let (route2_key, route2) = ordered[1];
    assert_eq!(route1_key.account_id, "account-alpha");
    assert_eq!(route1_key.chat_id, "chat-alpha");
    assert_eq!(route1_key.thread_id, Some(Some("thread-alpha".to_owned())));
    assert_eq!(route2_key.account_id, "account-beta");
    assert_eq!(route2_key.chat_id, "chat-beta");
    assert_eq!(route2_key.thread_id, Some(Some("thread-beta".to_owned())));
    let routes = lotta_channels::state_store::ChannelStateStore::new(&store)
        .restorable_routes()
        .unwrap();
    let route1_domain = routes
        .iter()
        .find(|route| route.channel_id.as_str() == "task78-alpha")
        .unwrap()
        .clone();
    let route2_domain = routes
        .iter()
        .find(|route| route.channel_id.as_str() == "task78-beta")
        .unwrap()
        .clone();

    route1.submit_inbound(InboundChannelMessage {
        route: route1_domain,
        client_message_id: "route1-inbound".to_owned(),
        payload: serde_json::json!({
            "kind": "create_message",
            "messages": [{"role": "user", "content": "inbound-one", "client_message_id": "route1-inbound"}]
        }),
    }).unwrap();
    let route1_ack = tokio::select! {
        event = next_event(route1) => event,
        result = &mut host => panic!("host exited after route1 input: {result:?}"),
    };
    let route1_delta = next_event(route1).await;
    assert_eq!(route1_ack["type"], "input_accepted");
    assert_eq!(route1_delta["type"], "stream_delta");
    assert_eq!(route1_delta["request_id"], route1_ack["request_id"]);
    assert_eq!(route1_delta["event_seq"], 1);
    let delivery1 = tokio::time::timeout(DEADLINE, route1.receive_outbound())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivery1.route.channel_id.as_str(), "task78-alpha");
    assert_eq!(delivery1.tool_call_id, "tool-call-1");
    assert_eq!(delivery1.request_id, "external-tool-1-1");
    assert_eq!(delivery1.message, "outbound-1");
    delivery1.complete(MessageChannelResult::Delivered);
    let route1_state = next_event(route1).await;
    let route1_terminal = next_event(route1).await;
    assert_eq!(route1_state["type"], "update_loop_status");
    assert_eq!(route1_state["request_id"], route1_ack["request_id"]);
    assert_eq!(route1_state["event_seq"], 2);
    assert_eq!(route1_terminal["type"], "runtime_error");
    assert_eq!(route1_terminal["request_id"], route1_ack["request_id"]);
    assert_eq!(route1_terminal["error"], "runtime service unavailable");

    route2.submit_inbound(InboundChannelMessage {
        route: route2_domain,
        client_message_id: "route2-inbound".to_owned(),
        payload: serde_json::json!({
            "kind": "create_message",
            "messages": [{"role": "user", "content": "inbound-two", "client_message_id": "route2-inbound"}]
        }),
    }).unwrap();
    let route2_ack = next_event(route2).await;
    let route2_delta = next_event(route2).await;
    assert_eq!(route2_ack["type"], "input_accepted");
    assert_eq!(route2_delta["type"], "stream_delta");
    assert_ne!(route2_ack["request_id"], route1_ack["request_id"]);
    assert_eq!(route2_delta["request_id"], route2_ack["request_id"]);
    assert_eq!(route2_delta["event_seq"], 3);
    let delivery2 = tokio::time::timeout(DEADLINE, route2.receive_outbound())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivery2.route.channel_id.as_str(), "task78-beta");
    assert_eq!(delivery2.tool_call_id, "tool-call-2");
    assert_eq!(delivery2.request_id, "external-tool-1-2");
    assert_eq!(delivery2.message, "outbound-2");
    delivery2.complete(MessageChannelResult::Delivered);
    let route2_state = next_event(route2).await;
    let route2_terminal = next_event(route2).await;
    assert_eq!(route2_state["type"], "update_loop_status");
    assert_eq!(route2_state["request_id"], route2_ack["request_id"]);
    assert_eq!(route2_state["event_seq"], 4);
    assert_eq!(route2_terminal["type"], "turn_finished");
    assert_eq!(route2_terminal["request_id"], route2_ack["request_id"]);
    assert_eq!(route2_terminal["event_seq"], 5);

    assert!(
        tokio::time::timeout(Duration::from_millis(100), route1.receive_event())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), route1.receive_outbound())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), route2.receive_event())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), route2.receive_outbound())
            .await
            .is_err()
    );
    assert_eq!(service.starts.load(Ordering::SeqCst), 1);
    let exact_results = tool_results.lock().unwrap().clone();
    assert_eq!(exact_results.len(), 2);
    for result in exact_results {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result).unwrap(),
            serde_json::json!({
                "content": [{"type": "text", "text": "delivered"}],
                "is_error": false
            })
        );
    }

    shutdown_sender.send(()).unwrap();
    let host_result = tokio::time::timeout(DEADLINE, host).await.unwrap().unwrap();
    assert!(host_result.is_ok(), "host shutdown: {host_result:?}");
    let management_trace = tokio::time::timeout(DEADLINE, parent)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        management_trace
            .iter()
            .filter(|entry| **entry == "ready")
            .count(),
        1
    );
    assert_eq!(
        management_trace
            .iter()
            .filter(|entry| **entry == "channels")
            .count(),
        1
    );
    assert_eq!(
        management_trace
            .iter()
            .filter(|entry| **entry == "publish")
            .count(),
        1
    );
    assert_eq!(
        management_trace
            .iter()
            .filter(|entry| **entry == "release")
            .count(),
        1
    );
    assert_eq!(management_trace.last(), Some(&"shutdown_complete"));
    assert!(!manager.contains(&ChannelRuntimeKey {
        agent_id: "agent-task78".to_owned(),
        conversation_id: "conversation-task78".to_owned(),
    }));
    assert!(authenticator.revoke("task78-owner", 1, std::process::id()));
    let listener_address = listener.address();
    listener.shutdown();
    tokio::time::timeout(DEADLINE, listener.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::net::TcpStream::connect(listener_address)
            .await
            .is_err()
    );
    std::fs::remove_dir_all(home).unwrap();
}

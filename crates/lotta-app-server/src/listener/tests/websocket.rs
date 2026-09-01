use futures_util::{SinkExt, StreamExt};
use lotta_domain::Clock;
use lotta_domain::Timestamp;
use lotta_testkit::clock::FakeClock;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::start_listener_for_test;
use crate::config::ServerArgs;
use crate::ws::{EventIdGenerator, RuntimeEvent};

const FRAME_CAP: usize = 1024;
const LONG_PING_MS: u64 = 3_600_000;

fn clock() -> Arc<FakeClock> {
    let timestamp = Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z")
        .unwrap_or_else(|error| panic!("timestamp: {error}"));
    Arc::new(FakeClock::new(timestamp))
}

fn inert_terminal_forwarder() -> crate::ws::terminal::TerminalForwarder {
    Arc::new(|_, _| Ok(()))
}

async fn listener(frame_cap: usize) -> super::ListenerHandle {
    let args = ServerArgs {
        listen_enabled: true,
        ..ServerArgs::default()
    };
    let prepared = args
        .prepare()
        .unwrap_or_else(|error| panic!("prepare: {error}"));
    start_listener_for_test(prepared, clock(), frame_cap, LONG_PING_MS)
        .await
        .unwrap_or_else(|error| panic!("listener: {error}"))
}

async fn next_message<S>(socket: &mut S) -> Message
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    socket
        .next()
        .await
        .unwrap_or_else(|| panic!("socket ended"))
        .unwrap_or_else(|error| panic!("websocket receive: {error}"))
}

struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56+00:00").expect("test timestamp")
    }

    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, lotta_domain::DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

#[tokio::test]
async fn exact_frame_cap_returns_protocol_error_and_socket_stays_usable() {
    let handle = listener(FRAME_CAP).await;
    let (mut socket, _) = connect_async(handle.websocket_url())
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"));
    socket
        .send(Message::Text("x".repeat(FRAME_CAP).into()))
        .await
        .unwrap();
    let Message::Text(body) = next_message(&mut socket).await else {
        panic!("expected text protocol error");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["code"],
        "malformed_json"
    );
    socket.send(Message::Text("{}".into())).await.unwrap();
    socket.send(Message::Ping(Vec::new().into())).await.unwrap();
    assert!(matches!(next_message(&mut socket).await, Message::Pong(_)));
    handle.wait().await.unwrap();
}

#[tokio::test]
async fn frame_cap_plus_one_closes_with_1009() {
    let handle = listener(FRAME_CAP).await;
    let (mut socket, _) = connect_async(handle.websocket_url()).await.unwrap();
    socket
        .send(Message::Text("x".repeat(FRAME_CAP + 1).into()))
        .await
        .unwrap();
    let Message::Close(Some(frame)) = next_message(&mut socket).await else {
        panic!("expected close frame");
    };
    assert_eq!(u16::from(frame.code), 1009);
    assert_eq!(frame.reason, "message too large");
    handle.wait().await.unwrap();
}

#[tokio::test]
async fn nested_request_id_257_returns_correlated_error_and_remains_usable() {
    let handle = listener(4096).await;
    let (mut socket, _) = connect_async(handle.websocket_url()).await.unwrap();
    let outer = "outer-request";
    let text = serde_json::json!({
        "type": "input",
        "request_id": outer,
        "payload": {
            "kind": "approval_response",
            "request_id": "n".repeat(257),
        },
    })
    .to_string();
    socket.send(Message::Text(text.into())).await.unwrap();
    let Message::Text(body) = next_message(&mut socket).await else {
        panic!("expected protocol error");
    };
    let value: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["type"], "protocol_error");
    assert_eq!(value["code"], "request_id_too_long");
    assert_eq!(value["message"], "request_id is too long");
    assert_eq!(value["request_id"], outer);
    socket.send(Message::Ping(Vec::new().into())).await.unwrap();
    assert!(matches!(next_message(&mut socket).await, Message::Pong(_)));
    handle.wait().await.unwrap();
}

#[tokio::test]
async fn outbound_registry_fans_out_actual_broadcast_to_both_peers() {
    let clock = clock();
    let ids: Arc<dyn EventIdGenerator> = Arc::new(crate::ws::RandomEventIdGenerator);
    let mut router = crate::ws::RuntimeRouter::new(clock, ids);
    let first = router.connections.open().unwrap();
    router.connections.initialize(first).unwrap();
    let second = router.connections.open().unwrap();
    router.connections.initialize(second).unwrap();
    let scope = lotta_domain::RuntimeScope::new(
        lotta_domain::AgentId::accept("agent-1").unwrap(),
        lotta_domain::ConversationId::accept("conversation-1").unwrap(),
        None,
    );
    router.connections.subscribe(first, scope.clone()).unwrap();
    router.connections.subscribe(second, scope.clone()).unwrap();
    let deliveries = router
        .broadcast(
            &scope,
            &RuntimeEvent::UpdateQueue {
                queue: Vec::new(),
                removed: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(
        deliveries
            .as_slice()
            .iter()
            .map(|d| d.connection_id)
            .collect::<Vec<_>>(),
        vec![first, second]
    );
    let (tx1, mut rx1) = mpsc::channel(1);
    let (tx2, mut rx2) = mpsc::channel(1);
    let outbound = Mutex::new(HashMap::from([(first, tx1), (second, tx2)]));
    super::dispatch_deliveries(&outbound, &deliveries).unwrap();
    let one: Value = serde_json::from_str(&rx1.recv().await.unwrap()[0]).unwrap();
    let two: Value = serde_json::from_str(&rx2.recv().await.unwrap()[0]).unwrap();
    for value in [&one, &two] {
        assert_eq!(value["type"], "update_queue");
        assert_eq!(value["queue"], serde_json::json!([]));
        assert_eq!(value["event_seq"], 1);
    }
    assert_ne!(one["idempotency_key"], two["idempotency_key"]);
}

/// Storage-root-backed bridges shared by the typed-failure state builder.
struct StorageBridges {
    files: Arc<crate::ws::files::FilesBridge>,
    memories: Arc<crate::ws::memory::MemoryBridge>,
    agents: Arc<crate::ws::agents::AgentsBridge>,
    conversations: Arc<crate::ws::conversations::ConversationsBridge>,
    models: Arc<crate::ws::models::ModelsBridge>,
    schedules: Arc<crate::ws::schedules::SchedulesBridge>,
    skills: Arc<crate::ws::skills::SkillsBridge>,
    settings: Arc<crate::ws::settings::SettingsBridge>,
    devices: Arc<crate::ws::device::DeviceBridge>,
}

/// Composes every storage-backed bridge over one prepared root set.
fn storage_bridges(prepared: &crate::config::PreparedServer) -> StorageBridges {
    let catalog = inert_catalog_bridges(&prepared.storage_dir);
    StorageBridges {
        files: Arc::new(
            crate::ws::files::FilesBridge::new(
                crate::ws::files::inert_forwarder(),
                &prepared.workspace_dir,
                &prepared.storage_dir.join("artifacts"),
            )
            .expect("files bridge"),
        ),
        memories: Arc::new(
            crate::ws::memory::MemoryBridge::new(
                crate::ws::memory::inert_forwarder(),
                &prepared.storage_dir,
                clock(),
            )
            .expect("memory bridge"),
        ),
        agents: Arc::new(
            crate::ws::agents::AgentsBridge::new(
                crate::ws::agents::inert_forwarder(),
                &prepared.storage_dir,
                clock(),
            )
            .expect("agents bridge"),
        ),
        conversations: Arc::new(
            crate::ws::conversations::ConversationsBridge::new(
                crate::ws::conversations::inert_forwarder(),
                &prepared.storage_dir,
                clock(),
                None,
            )
            .expect("conversations bridge"),
        ),
        models: catalog.0,
        schedules: catalog.1,
        skills: Arc::new(crate::ws::skills::SkillsBridge::new(
            crate::ws::skills::inert_forwarder(),
            &prepared.storage_dir,
            clock(),
        )),
        settings: Arc::new(
            crate::ws::settings::SettingsBridge::new(
                crate::ws::settings::inert_forwarder(),
                &prepared.storage_dir,
                &prepared.workspace_dir,
            )
            .expect("settings bridge"),
        ),
        devices: Arc::new(
            crate::ws::device::DeviceBridge::new(
                crate::ws::device::inert_forwarder(),
                &prepared.workspace_dir,
                &prepared.storage_dir,
            )
            .expect("device bridge"),
        ),
    }
}

/// Catalog-side bridges over one storage root.
fn inert_catalog_bridges(
    storage_dir: &std::path::Path,
) -> (
    Arc<crate::ws::models::ModelsBridge>,
    Arc<crate::ws::schedules::SchedulesBridge>,
) {
    (
        Arc::new(
            crate::ws::models::ModelsBridge::new(
                crate::ws::models::inert_forwarder(),
                storage_dir,
                Arc::new(TestClock),
            )
            .expect("models bridge"),
        ),
        Arc::new(
            crate::ws::schedules::SchedulesBridge::new(
                crate::ws::schedules::inert_forwarder(),
                storage_dir,
                Arc::new(TestClock),
            )
            .expect("schedules bridge"),
        ),
    )
}

/// Builds the listener state used by the typed-failure assertions.
fn typed_failure_state(
    prepared: crate::config::PreparedServer,
    router: Arc<Mutex<crate::ws::RuntimeRouter>>,
    origin: crate::ws::ConnectionId,
    sender: mpsc::Sender<Vec<String>>,
) -> super::ListenerState {
    let storage = storage_bridges(&prepared);
    let shutdown = tokio_util::sync::CancellationToken::new();
    super::ListenerState {
        auth: prepared.auth,
        openai_api: false,
        channel_host_protocol_only: false,
        channel_session: None,
        listener_instance: "test-listener".to_owned(),
        openai_chat: super::test_openai_chat(
            &storage.agents,
            &storage.conversations,
            clock(),
            shutdown.clone(),
        ),
        openai_responses: super::test_openai_responses(
            &storage.agents,
            &storage.conversations,
            clock(),
            shutdown.clone(),
        ),
        clock: clock(),
        shutdown: shutdown.clone(),
        limits: super::SocketLimits::default(),
        runtime_router: router,
        runtime_service: Arc::new(crate::ws::UnsupportedRuntimeCommandService),
        turn_controller: Arc::new(crate::ws::UnsupportedRuntimeCommandService),
        turns: super::turn_supervisor::RuntimeTurnSupervisor::new(shutdown),
        observer: Arc::new(crate::observer::InertRuntimeBroadcastObserver),
        external_tools: Arc::new(crate::ws::external_tools::ExternalToolBridge::new(
            crate::ws::external_tools::inert_forwarder(),
        )),
        channel_tools: None,
        teleports: Arc::new(crate::ws::teleport::TeleportBridge::new(
            crate::ws::teleport::inert_forwarder(),
        )),
        terminals: Arc::new(crate::ws::terminal::TerminalBridge::new(
            inert_terminal_forwarder(),
            clock(),
        )),
        files: storage.files,
        memories: storage.memories,
        agents: storage.agents,
        conversations: storage.conversations,
        models: storage.models,
        schedules: storage.schedules,
        skills: storage.skills,
        settings: storage.settings,
        devices: storage.devices,
        introspection: Arc::new(crate::ws::introspection::IntrospectionBridge::new(
            crate::ws::introspection::inert_forwarder(),
        )),
        next_observation: std::sync::atomic::AtomicU64::new(1),
        outbound: Arc::new(Mutex::new(HashMap::from([(origin, sender)]))),
    }
}

#[tokio::test]
async fn typed_runtime_failures_are_unstamped_and_sent_to_origin() {
    let args = ServerArgs {
        listen_enabled: true,
        ..ServerArgs::default()
    };
    let prepared = args.prepare().unwrap();
    let mut runtime_router =
        crate::ws::RuntimeRouter::new(clock(), Arc::new(crate::ws::RandomEventIdGenerator));
    let origin = runtime_router.connections.open().unwrap();
    runtime_router.connections.initialize(origin).unwrap();
    let router = Arc::new(Mutex::new(runtime_router));
    let (sender, mut receiver) = mpsc::channel::<Vec<String>>(4);
    let state = typed_failure_state(prepared, router, origin, sender);
    for (wire, expected, false_field) in [
        (
            serde_json::json!({"type":"runtime_start","request_id":"r"}),
            "runtime_start_response",
            "success",
        ),
        (
            serde_json::json!({
                "type":"input", "request_id":"r",
                "runtime":{"agent_id":"a","conversation_id":"c"}, "payload":{}
            }),
            "input_accepted",
            "accepted",
        ),
        (
            serde_json::json!({
                "type":"sync", "request_id":"r",
                "runtime":{"agent_id":"a","conversation_id":"c"}
            }),
            "sync_response",
            "success",
        ),
        (
            serde_json::json!({
                "type":"abort_message", "request_id":"r",
                "runtime":{"agent_id":"a","conversation_id":"c"}
            }),
            "abort_message_response",
            "success",
        ),
    ] {
        let frame = crate::framing::decode_text(&wire.to_string()).unwrap();
        super::dispatch_typed_failure(&state, origin, &frame).unwrap();
        let value: Value = serde_json::from_str(&receiver.recv().await.unwrap()[0]).unwrap();
        assert_eq!(value["type"], expected);
        assert_eq!(value[false_field], false);
        assert_eq!(value["error"], "runtime service unavailable");
        for field in ["event_seq", "emitted_at", "idempotency_key"] {
            assert!(value.get(field).is_none(), "{field}");
        }
    }
}

#[test]
fn runtime_event_survives_transport_disappearing_after_routing() {
    let args = ServerArgs {
        listen_enabled: true,
        ..ServerArgs::default()
    };
    let prepared = args.prepare().expect("prepared server");
    let mut runtime_router =
        crate::ws::RuntimeRouter::new(clock(), Arc::new(crate::ws::RandomEventIdGenerator));
    let origin = runtime_router.connections.open().expect("open connection");
    runtime_router
        .connections
        .initialize(origin)
        .expect("initialize connection");
    let scope = lotta_domain::RuntimeScope::new(
        lotta_domain::AgentId::accept("agent-transport-race").expect("agent id"),
        lotta_domain::ConversationId::accept("conversation-transport-race")
            .expect("conversation id"),
        None,
    );
    runtime_router
        .connections
        .subscribe(origin, scope.clone())
        .expect("subscribe connection");
    let router = Arc::new(Mutex::new(runtime_router));
    let (sender, receiver) = mpsc::channel::<Vec<String>>(1);
    let state = Arc::new(typed_failure_state(prepared, router, origin, sender));
    drop(receiver);

    let result = super::event_sink(&state).emit(
        &scope,
        RuntimeEvent::UpdateQueue {
            queue: Vec::new(),
            removed: Vec::new(),
        },
    );

    assert!(result.is_ok());
}

#[test]
fn websocket_error_classifier_maps_concrete_classes() {
    use axum::extract::ws::close_code;
    use tungstenite::{Error, error::ProtocolError};
    let classify =
        |error| super::websocket_error_close(axum::Error::new(error)).map(|value| value.0);
    assert_eq!(
        classify(Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)),
        Some(close_code::PROTOCOL)
    );
    assert_eq!(
        classify(Error::Utf8("bad".into())),
        Some(close_code::INVALID)
    );
    assert_eq!(
        classify(Error::Capacity(
            tungstenite::error::CapacityError::MessageTooLong {
                size: 2,
                max_size: 1
            }
        )),
        Some(close_code::SIZE)
    );
    assert_eq!(classify(Error::Io(std::io::Error::other("closed"))), None);
}

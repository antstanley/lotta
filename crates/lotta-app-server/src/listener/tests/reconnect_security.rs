use std::{fs, sync::{Arc, atomic::{AtomicU64, Ordering}}};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use lotta_domain::{
    AgentId, BoundedJsonValue, BoundedVec, ConversationId, InputDisposition, NonEmptyString,
    RuntimeScope, Timestamp,
};
use lotta_testkit::clock::FakeClock;
use serde_json::{Value, json};
use sha2::Sha256;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use super::{ListenerHandle, ListenerState, start_listener_with_state_for_test};
use crate::{
    config::ServerArgs,
    error::AppServerError,
    ws::{
        RuntimeCommandService, RuntimeEvent, RuntimeEventSink,
        service::{
            AbortOutcome, DeviceStateOutcome, InputAdmission, RuntimeEventBatch,
            RuntimeStartOutcome, ServiceFuture, SyncOutcome,
        },
    },
};

const SECRET: &[u8] = b"task-73-actual-websocket-reconnect-secret";
const TTL: i64 = crate::ws::connection::SUSPENDED_CONNECTION_TTL_SECONDS;
type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct Harness {
    handle: Option<ListenerHandle>,
    state: Arc<ListenerState>,
    clock: Arc<FakeClock>,
    url: String,
}

impl Harness {
    async fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let secret = std::env::temp_dir().join(format!(
            "lotta-reconnect-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&secret, SECRET).expect("secret");
        let args = ServerArgs {
            listen_enabled: true,
            ws_auth: Some("signed-bearer-token".into()),
            ws_shared_secret_file: Some(secret.clone()),
            ..ServerArgs::default()
        };
        let prepared = args.prepare().expect("prepared listener");
        let _ = fs::remove_file(secret);
        let start = Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z").expect("time");
        let clock = Arc::new(FakeClock::new(start));
        let (handle, state) = start_listener_with_state_for_test(
            prepared,
            clock.clone(),
            Arc::new(ReconnectService),
        )
        .await
        .expect("listener");
        let url = handle.websocket_url().to_owned();
        Self { handle: Some(handle), state, clock, url }
    }

    async fn connect(&self, sub: &str, serial: u64, client_id: Option<&str>) -> Client {
        let mut request = self.url.clone().into_client_request().expect("request");
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {}", token(sub, serial)).parse().expect("auth header"),
        );
        if let Some(id) = client_id {
            request.headers_mut().insert("x-lotta-reconnect-id", id.parse().expect("id header"));
        }
        connect_async(request).await.expect("upgrade").0
    }

    async fn subscribe(&self, socket: &mut Client, suffix: &str) {
        socket.send(Message::Text(json!({
            "type":"runtime_start", "request_id":format!("start-{suffix}"),
            "agent_id":format!("agent-{suffix}"), "conversation_id":format!("conversation-{suffix}")
        }).to_string().into())).await.expect("start send");
        let response = recv_json(socket).await;
        assert_eq!(response["type"], "runtime_start_response");
    }

    async fn sync_seq(&self, socket: &mut Client, suffix: &str) -> u64 {
        socket.send(Message::Text(json!({
            "type":"sync", "request_id":format!("sync-{suffix}"),
            "runtime":{"agent_id":format!("agent-{suffix}"),"conversation_id":format!("conversation-{suffix}")}
        }).to_string().into())).await.expect("sync send");
        let event = recv_json(socket).await;
        assert_eq!(event["type"], "update_queue");
        let seq = event["event_seq"].as_u64().expect("sequence");
        assert_eq!(recv_json(socket).await["type"], "sync_response");
        seq
    }

    async fn close(&self, mut socket: Client) {
        socket.close(None).await.expect("close");
        self.barrier().await;
    }

    async fn barrier(&self) {
        let mut socket = self.connect("barrier", 1, None).await;
        socket.send(Message::Ping(Vec::new().into())).await.expect("ping");
        assert!(matches!(socket.next().await.expect("pong").expect("pong frame"), Message::Pong(_)));
        socket.close(None).await.expect("barrier close");
    }

    fn active(&self) -> Vec<(u64, usize, u64)> {
        let router = self.state.runtime_router.lock().expect("router");
        router.connections.inspect_active()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.as_mut() { handle.shutdown(); }
    }
}

struct ReconnectService;

impl RuntimeCommandService for ReconnectService {
    fn runtime_start(&self, command: crate::ws::command::RuntimeStartCommand) -> ServiceFuture<'_, RuntimeStartOutcome> {
        let agent = command.agent_id.expect("agent").as_str().to_owned();
        let conversation = command.conversation_id.expect("conversation").as_str().to_owned();
        Box::pin(async move { Ok(RuntimeStartOutcome {
            runtime: scope(&agent, &conversation), created_agent: false, created_conversation: false,
            agent: None, conversation: None, broadcasts: events(Vec::new()),
        }) })
    }
    fn admit_input(&self, _: crate::ws::command::InputCommand) -> ServiceFuture<'_, InputAdmission> {
        Box::pin(async { Ok(InputAdmission { disposition: InputDisposition::Started, error: None, continuation: None, after_ack: events(Vec::new()) }) })
    }
    fn continue_input(&self, _: RuntimeScope, _: Option<BoundedJsonValue>, _: Arc<dyn RuntimeEventSink>) -> ServiceFuture<'_, ()> { Box::pin(async { Ok(()) }) }
    fn compact(&self, _: RuntimeScope, _: lotta_runtime::CompactionMode, _: NonEmptyString, _: lotta_runtime::ports::ProviderRequest) -> ServiceFuture<'_, lotta_runtime::turn::CompactionProgress> { Box::pin(async { Err(AppServerError::Unavailable) }) }
    fn sync(&self, _: crate::ws::command::SyncCommand) -> ServiceFuture<'_, SyncOutcome> {
        Box::pin(async { Ok(SyncOutcome { broadcasts: events(vec![RuntimeEvent::UpdateQueue { queue: Vec::new(), removed: Vec::new() }]) }) })
    }
    fn abort_message(&self, _: crate::ws::command::AbortMessageCommand) -> ServiceFuture<'_, AbortOutcome> { Box::pin(async { Ok(AbortOutcome { aborted: false }) }) }
    fn change_device_state(&self, _: crate::ws::command::ChangeDeviceStateCommand) -> ServiceFuture<'_, DeviceStateOutcome> { Box::pin(async { Ok(DeviceStateOutcome { broadcasts: events(Vec::new()) }) }) }
}

fn scope(agent: &str, conversation: &str) -> RuntimeScope {
    RuntimeScope::new(AgentId::accept(agent).expect("agent"), ConversationId::accept(conversation).expect("conversation"), None)
}

fn events(values: Vec<RuntimeEvent>) -> RuntimeEventBatch { BoundedVec::new(values).expect("events") }

fn token(sub: &str, serial: u64) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = URL_SAFE_NO_PAD.encode(json!({"sub":sub,"serial":serial,"exp":2_000_000_000i64}).to_string());
    let message = format!("{header}.{claims}");
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET).expect("key");
    mac.update(message.as_bytes());
    format!("{message}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

async fn recv_json(socket: &mut Client) -> Value {
    loop {
        match socket.next().await.expect("socket open").expect("receive") {
            Message::Text(text) => return serde_json::from_str(&text).expect("json"),
            Message::Ping(body) => socket.send(Message::Pong(body)).await.expect("pong"),
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

#[tokio::test]
async fn same_bearer_distinct_reconnect_ids_stay_isolated() {
    let h = Harness::new().await;
    let mut a = h.connect("same", 1, Some("a")).await;
    let b = h.connect("same", 1, Some("b")).await;
    h.subscribe(&mut a, "a").await;
    assert_eq!(h.sync_seq(&mut a, "a").await, 1);
    h.close(a).await;
    h.close(b).await;
    let mut b = h.connect("same", 1, Some("b")).await;
    assert_eq!(h.sync_seq(&mut b, "a").await, 1);
}

#[tokio::test]
async fn renewed_bearer_same_principal_and_client_resumes_subscription_and_sequence() {
    let h = Harness::new().await;
    let mut old = h.connect("principal", 1, Some("device")).await;
    h.subscribe(&mut old, "resume").await;
    assert_eq!(h.sync_seq(&mut old, "resume").await, 1);
    h.close(old).await;
    let mut renewed = h.connect("principal", 2, Some("device")).await;
    assert_eq!(h.sync_seq(&mut renewed, "resume").await, 2);
    assert_eq!(h.active().into_iter().find(|(_, n, _)| *n == 1).map(|(_, _, seq)| seq), Some(2));
}

#[tokio::test]
async fn principals_and_colon_ids_cannot_collide_or_inherit() {
    let h = Harness::new().await;
    let mut first = h.connect("a:b", 1, Some("c")).await;
    h.subscribe(&mut first, "colon").await;
    assert_eq!(h.sync_seq(&mut first, "colon").await, 1);
    h.close(first).await;
    let mut second = h.connect("a", 1, Some("b:c")).await;
    assert_eq!(h.sync_seq(&mut second, "colon").await, 1);
}

#[tokio::test]
async fn malformed_empty_and_oversized_reconnect_ids_are_non_resumable() {
    let h = Harness::new().await;
    let oversized = "x".repeat(257);
    for (index, id) in [None, Some("   "), Some("bad\tvalue"), Some(oversized.as_str())].into_iter().enumerate() {
        let suffix = format!("invalid-{index}");
        let mut first = h.connect("invalid", 1, id).await;
        h.subscribe(&mut first, &suffix).await;
        assert_eq!(h.sync_seq(&mut first, &suffix).await, 1);
        h.close(first).await;
        let mut second = h.connect("invalid", 2, id).await;
        assert_eq!(h.sync_seq(&mut second, &suffix).await, 1, "case {index}");
        h.close(second).await;
    }
}

#[tokio::test]
async fn live_takeover_is_rejected_without_replacing_owner() {
    let h = Harness::new().await;
    let mut owner = h.connect("live", 1, Some("device")).await;
    h.subscribe(&mut owner, "live").await;
    let mut takeover = h.connect("live", 2, Some("device")).await;
    assert!(matches!(takeover.next().await, None | Some(Ok(Message::Close(_)) | Err(_))));
    assert_eq!(h.sync_seq(&mut owner, "live").await, 1);
}

#[tokio::test]
async fn stale_old_socket_close_cannot_overwrite_new_owner() {
    let h = Harness::new().await;
    let mut old = h.connect("stale", 1, Some("device")).await;
    h.subscribe(&mut old, "stale").await;
    assert_eq!(h.sync_seq(&mut old, "stale").await, 1);
    drop(old);
    h.barrier().await;
    let mut owner = h.connect("stale", 2, Some("device")).await;
    assert_eq!(h.sync_seq(&mut owner, "stale").await, 2);
    h.barrier().await;
    assert_eq!(h.sync_seq(&mut owner, "stale").await, 3);
}

#[tokio::test]
async fn deterministic_clock_ttl_exact_boundary_and_expiry() {
    let h = Harness::new().await;
    let mut before = h.connect("ttl", 1, Some("before")).await;
    h.subscribe(&mut before, "before").await;
    assert_eq!(h.sync_seq(&mut before, "before").await, 1);
    h.close(before).await;
    h.clock.advance(chrono::Duration::seconds(TTL - 1)).expect("advance");
    let mut before = h.connect("ttl", 2, Some("before")).await;
    assert_eq!(h.sync_seq(&mut before, "before").await, 2);
    h.close(before).await;
    let mut exact = h.connect("ttl", 1, Some("exact")).await;
    h.subscribe(&mut exact, "exact").await;
    assert_eq!(h.sync_seq(&mut exact, "exact").await, 1);
    h.close(exact).await;
    h.clock.advance(chrono::Duration::seconds(TTL)).expect("advance");
    let mut exact = h.connect("ttl", 2, Some("exact")).await;
    assert_eq!(h.sync_seq(&mut exact, "exact").await, 1);
}

#[tokio::test]
async fn suspended_count_cap_evicts_oldest_deterministically_and_protects_resumed_entry() {
    let h = Harness::new().await;
    for index in 0..crate::ws::connection::SUSPENDED_CONNECTIONS_MAX {
        let id = format!("cap-{index:04}");
        let socket = h.connect("cap", 1, Some(&id)).await;
        h.close(socket).await;
        h.clock.advance(chrono::Duration::milliseconds(1)).expect("advance");
    }
    let mut protected = h.connect("cap", 2, Some("cap-0001")).await;
    h.subscribe(&mut protected, "protected").await;
    assert_eq!(h.sync_seq(&mut protected, "protected").await, 1);
    let overflow = h.connect("cap", 1, Some("cap-overflow")).await;
    h.close(overflow).await;
    let evicted = h.connect("cap", 2, Some("cap-0000")).await;
    assert!(h.active().iter().any(|(_, subscriptions, _)| *subscriptions == 1));
    h.close(evicted).await;
    assert_eq!(h.sync_seq(&mut protected, "protected").await, 2);
}

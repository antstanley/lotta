//! `ws::device::remove_queue_item` — removal routes through the registered
//! [`QueueAuthority`] port over the exact Task 18 [`ConversationQueue`] the
//! turn pipeline uses, so exactly one wire transition plus one authoritative
//! snapshot back the answer, and the snapshot is rebroadcast as a pinned-shape
//! `RuntimeEvent::UpdateQueue` through the runtime router.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use lotta_domain::{AgentId, BoundedJsonValue, ConversationId, NonEmptyString, RuntimeScope};
use lotta_runtime::{ListenerRuntime, QueueMutation, RuntimeError};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{DeviceBridge, DeviceForwarder, DeviceMessage, QueueAuthority};
use crate::{framing, ws::ConnectionId, ws::event::RuntimeEvent, ws::service::RuntimeEventSink};

const CONNECTION: ConnectionId = 71;

static TEST_OWNER: AtomicU64 = AtomicU64::new(1);

fn next_test_owner() -> uuid::Uuid {
    uuid::Uuid::from_u128(u128::from(TEST_OWNER.fetch_add(1, Ordering::SeqCst)))
}

/// Test-double authority over one real [`ListenerRuntime`] instance.
struct RegistryQueueAuthority {
    registry: Arc<Mutex<ListenerRuntime>>,
}

impl QueueAuthority for RegistryQueueAuthority {
    fn remove_queued<'a>(
        &'a self,
        scope: &'a RuntimeScope,
        item_id: &'a NonEmptyString,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<QueueMutation>, RuntimeError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let handle = registry.get_or_create(scope, next_test_owner())?;
            registry.cancel_queued(&handle, item_id)
        })
    }
}

/// Records every runtime event routed to scope subscribers.
#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<RuntimeEvent>>,
}

impl RuntimeEventSink for RecordingSink {
    fn emit(
        &self,
        _scope: &RuntimeScope,
        event: RuntimeEvent,
    ) -> Result<(), crate::error::AppServerError> {
        self.events.lock().expect("sink lock").push(event);
        Ok(())
    }
}

struct Harness {
    bridge: Arc<DeviceBridge>,
    registry: Arc<Mutex<ListenerRuntime>>,
    scope: RuntimeScope,
    messages: Arc<Mutex<Vec<(ConnectionId, DeviceMessage)>>>,
    sink: Arc<RecordingSink>,
}

fn harness() -> Harness {
    let ordinal = unique_ordinal();
    let parent =
        std::env::temp_dir().join(format!("lotta-device-rq-{}-{ordinal}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture root");
    let root = parent.canonicalize().expect("canonical root");
    let workspace = root.join("workspace");
    let storage = root.join("storage");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&storage).expect("storage");
    let messages: Arc<Mutex<Vec<(ConnectionId, DeviceMessage)>>> = Arc::default();
    let sink_messages = Arc::clone(&messages);
    let forward: DeviceForwarder = Arc::new(move |connection, message| {
        sink_messages
            .lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    let bridge = Arc::new(DeviceBridge::new(forward, &workspace, &storage).expect("bridge"));
    let registry = Arc::new(Mutex::new(ListenerRuntime::new()));
    bridge.register_queue_authority(Arc::new(RegistryQueueAuthority {
        registry: Arc::clone(&registry),
    }));
    let sink = Arc::new(RecordingSink::default());
    bridge.register_event_sink(Arc::clone(&sink) as Arc<dyn RuntimeEventSink>);
    Harness {
        bridge,
        registry,
        scope: RuntimeScope::new(
            AgentId::accept("agent-device").expect("agent"),
            ConversationId::accept("conversation-device").expect("conversation"),
            None,
        ),
        messages,
        sink,
    }
}

static ROOT_ORDINAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn unique_ordinal() -> usize {
    ROOT_ORDINAL.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

impl Harness {
    async fn send(&self, command: &Value) {
        let frame = framing::decode_text(&command.to_string()).expect("bounded frame");
        let decoded = super::decode(&frame)
            .expect("wellformed device command")
            .expect("device command routed");
        self.bridge.apply(CONNECTION, &decoded).await;
    }

    /// Enqueues one real Task 18 queue item on the shared runtime registry and
    /// returns the wire item id (`queue-<client_message_id>`).
    fn enqueue_on_registry(&self, client_message_id: &str) -> String {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let handle = registry
            .get_or_create(&self.scope, next_test_owner())
            .expect("entry");
        let item = lotta_domain::QueueItem {
            id: NonEmptyString::new(format!("queue-{client_message_id}")).expect("item id"),
            client_message_id: NonEmptyString::new(client_message_id.to_owned())
                .expect("client id"),
            kind: lotta_domain::QueueItemKind::Message,
            source: lotta_domain::QueueItemSource::User,
            content: BoundedJsonValue::new(json!({"text": "queued"})).expect("content"),
            enqueued_at: lotta_domain::Timestamp::parse_persisted_rfc3339("2026-08-14T00:00:00Z")
                .expect("timestamp"),
            extras: lotta_domain::EntityExtras::default(),
        };
        let _mutation = registry
            .enqueue_retained(&handle, item)
            .expect("bounded queue");
        format!("queue-{client_message_id}")
    }

    async fn send_removal(&self, item_id: &str) {
        self.send(&json!({
            "type": "remove_queue_item",
            "request_id": format!("rq-{item_id}"),
            "runtime": {
                "agent_id": self.scope.agent_id.as_str(),
                "conversation_id": self.scope.conversation_id.as_str(),
            },
            "item_id": item_id,
        }))
        .await;
    }

    fn kinds(&self) -> Vec<&'static str> {
        self.messages
            .lock()
            .expect("message lock")
            .iter()
            .filter(|(owner, _)| *owner == CONNECTION)
            .map(|(_, message)| match message {
                DeviceMessage::ExecuteCommand(_) => "execute_command_response",
                DeviceMessage::RemoveQueueItem(_) => "remove_queue_item_response",
                DeviceMessage::SearchBranches(_) => "search_branches_response",
                DeviceMessage::CheckoutBranch(_) => "checkout_branch_response",
                DeviceMessage::SecretList(_) => "secret_list_response",
                DeviceMessage::SecretApply(_) => "secret_apply_response",
            })
            .collect()
    }

    fn encoded(&self) -> Vec<Value> {
        self.messages
            .lock()
            .expect("message lock")
            .iter()
            .filter(|(owner, _)| *owner == CONNECTION)
            .map(|(_, message)| serde_json::to_value(message.clone()).expect("encodes"))
            .collect()
    }

    /// The authoritative queue contents on the shared runtime after mutation.
    fn registry_queue_client_ids(&self) -> Vec<String> {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let handle = registry
            .get_or_create(&self.scope, next_test_owner())
            .expect("entry");
        registry
            .queue(&handle)
            .map(|queue| {
                queue
                    .items()
                    .map(|item| item.client_message_id.as_str().to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    /// The single recorded `update_queue` event body.
    fn update_queue_event(&self) -> Value {
        let events = self.sink.events.lock().expect("sink lock");
        assert_eq!(events.len(), 1, "exactly one listener state event");
        let event = &events[0];
        assert_eq!(event.discriminant(), "update_queue");
        match event {
            RuntimeEvent::UpdateQueue { queue, removed } => json!({
                "queue": queue.as_value(),
                "removed": removed.as_value(),
            }),
            other => panic!("unexpected event discriminant {}", other.discriminant()),
        }
    }
}

#[tokio::test]
async fn routes_through_the_real_queue_operation() {
    let fixture = harness();
    // Two real Task 18 queue items on the shared runtime registry.
    let first = fixture.enqueue_on_registry("cm-1");
    let second = fixture.enqueue_on_registry("cm-2");

    fixture.send_removal(&second).await;

    assert_eq!(
        fixture.kinds(),
        vec!["remove_queue_item_response"],
        "one direct answer; state travels as a runtime event"
    );
    let encoded = fixture.encoded();
    assert_eq!(encoded[0]["success"], true, "the existing item was removed");
    assert_eq!(encoded[0]["item_id"], second);

    // The authoritative broadcast carries the pinned update_queue shape:
    // remaining items plus ordered removal transitions keyed by client id.
    let event = fixture.update_queue_event();
    let removed = event["removed"].as_array().expect("transitions");
    assert_eq!(removed.len(), 1, "exactly one removal transition");
    assert_eq!(removed[0]["client_message_id"], json!("cm-2"));
    assert_eq!(
        removed[0]["disposition"],
        json!("cancelled"),
        "queue cancellation carries the cancelled disposition"
    );
    let queue = event["queue"].as_array().expect("remaining items");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["client_message_id"], json!("cm-1"));
    assert_eq!(queue[0]["id"], json!(first));

    // Sync: the shared production queue really lost the removed item.
    assert_eq!(fixture.registry_queue_client_ids(), vec!["cm-1".to_owned()]);
}

#[tokio::test]
async fn unknown_items_answer_failure_without_state() {
    let fixture = harness();
    fixture.enqueue_on_registry("cm-1");

    fixture.send_removal("absent-item").await;

    assert_eq!(
        fixture.kinds(),
        vec!["remove_queue_item_response"],
        "failed removals emit the pinned response and no state broadcast"
    );
    assert_eq!(fixture.encoded()[0]["success"], false);
    assert!(
        fixture.sink.events.lock().expect("sink lock").is_empty(),
        "no authoritative broadcast without a real mutation"
    );
}

#[tokio::test]
async fn malformed_payloads_are_rejected_at_decode() {
    let frame_text = json!({
        "type": "remove_queue_item",
        "request_id": "rq-bad",
    })
    .to_string();
    let frame = framing::decode_text(&frame_text).expect("bounded frame");
    let error = super::decode(&frame).expect_err("malformed known command rejected");
    assert_eq!(error.code, "device_command_invalid");
}

/// Keeps the payload struct referenced so decode coverage stays honest.
#[tokio::test]
async fn payload_decode_matches_pinned_fields() {
    let value = json!({
        "type": "remove_queue_item",
        "request_id": "rq-fields",
        "runtime": {"agent_id": "a", "conversation_id": "c"},
        "item_id": "item-9",
    });
    let command: RemoveQueueItemPayload = serde_json::from_value(value).expect("typed");
    assert_eq!(command.item_id, "item-9");
}

use super::RemoveQueueItemPayload;

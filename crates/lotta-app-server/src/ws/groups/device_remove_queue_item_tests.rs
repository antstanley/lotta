//! `ws::device::remove_queue_item` — removal routes through the Task 18
//! [`ConversationQueue`] operation, so exactly one wire transition plus one
//! authoritative snapshot back the answer, and the snapshot is rebroadcast as
//! an `update_queue` listener state message.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use lotta_domain::{AgentId, ConversationId, RuntimeScope};

use super::{DeviceBridge, DeviceForwarder, DeviceMessage, RemoveQueueItemPayload};
use crate::{framing, ws::ConnectionId};

const CONNECTION: ConnectionId = 71;

struct Harness {
    bridge: Arc<DeviceBridge>,
    scope: RuntimeScope,
    messages: Arc<Mutex<Vec<(ConnectionId, DeviceMessage)>>>,
}

fn harness() -> Harness {
    let ordinal = unique_ordinal();
    let parent =
        std::env::temp_dir().join(format!("lotta-device-rq-{}-{ordinal}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("fixture root");
    let root = parent.canonicalize().expect("canonical root");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let storage = root.join("storage");
    std::fs::create_dir_all(&storage).expect("storage");
    let messages: Arc<Mutex<Vec<(ConnectionId, DeviceMessage)>>> = Arc::default();
    let sink = Arc::clone(&messages);
    let forward: DeviceForwarder = Arc::new(move |connection, message| {
        sink.lock()
            .expect("message lock")
            .push((connection, message));
        Ok(())
    });
    Harness {
        bridge: Arc::new(DeviceBridge::new(forward, &workspace, &storage).expect("bridge")),
        scope: RuntimeScope::new(
            AgentId::accept("agent-device").expect("agent"),
            ConversationId::accept("conversation-device").expect("conversation"),
            None,
        ),
        messages,
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
                DeviceMessage::QueueUpdate(_) => "update_queue",
                DeviceMessage::StatusUpdate(_) => "update_device_status",
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
}

#[tokio::test]
async fn routes_through_the_real_queue_operation() {
    let fixture = harness();
    // Two real Task 18 queue items; the bridge's own queue map holds them.
    fixture.bridge.enqueue_for_test(&fixture.scope, "item-1");
    fixture.bridge.enqueue_for_test(&fixture.scope, "item-2");

    fixture.send_removal("item-2").await;

    let kinds = fixture.kinds();
    assert_eq!(
        kinds,
        vec!["remove_queue_item_response", "update_queue"],
        "one answer followed by one authoritative queue state broadcast"
    );
    let encoded = fixture.encoded();
    assert_eq!(encoded[0]["success"], true, "the existing item was removed");
    assert_eq!(encoded[0]["item_id"], "item-2");

    // Exactly one explicit ordered transition carrying the wire disposition.
    let removed = encoded[1]["removed"].as_array().expect("transitions");
    assert_eq!(removed.len(), 1, "exactly one removal transition");
    assert_eq!(removed[0]["item_id"], "item-2");
    assert_eq!(
        removed[0]["disposition"],
        json!("cancelled"),
        "queue cancellation carries the cancelled disposition"
    );

    // Exactly one authoritative post-mutation snapshot: revision advanced and
    // only the untouched item remains.
    let queue = &encoded[1]["queue"];
    assert_eq!(queue["revision"], 3_u64, "enqueue x2 + one mutation");
    let items = queue["items"].as_array().expect("snapshot items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "item-1");
}

#[tokio::test]
async fn unknown_items_answer_failure_without_state() {
    let fixture = harness();
    fixture.bridge.enqueue_for_test(&fixture.scope, "item-1");

    fixture.send_removal("absent-item").await;

    let kinds = fixture.kinds();
    assert_eq!(
        kinds,
        vec!["remove_queue_item_response"],
        "failed removals emit the pinned response and no state broadcast"
    );
    assert_eq!(fixture.encoded()[0]["success"], false);
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

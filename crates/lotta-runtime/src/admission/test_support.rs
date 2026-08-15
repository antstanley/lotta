use super::{AdmissionRequest, AdmissionRoute};
use crate::{ListenerRuntime, RuntimeError, RuntimeHandle};
use lotta_domain::{
    AgentId, BoundedJsonValue, ConversationId, EntityExtras, NonEmptyString, QueueItem,
    QueueItemKind, QueueItemSource, RuntimeScope, Timestamp,
};
use serde_json::json;

pub fn runtime() -> (ListenerRuntime, RuntimeHandle) {
    let mut runtime = ListenerRuntime::new();
    let scope = RuntimeScope::new(
        AgentId::accept("agent").unwrap_or_else(|error| panic!("agent: {error}")),
        ConversationId::generate(1).unwrap_or_else(|error| panic!("conversation: {error}")),
        None,
    );
    let handle = runtime
        .get_or_create(&scope, uuid::Uuid::from_u128(1))
        .unwrap_or_else(|error| fail(&error));
    (runtime, handle)
}

pub fn request(number: usize, kind: QueueItemKind, source: QueueItemSource) -> AdmissionRequest {
    AdmissionRequest {
        item: QueueItem {
            id: text(&format!("item-{number}")),
            client_message_id: text(&format!("client-{number}")),
            kind,
            source,
            content: BoundedJsonValue::new(json!({"number": number}))
                .unwrap_or_else(|error| panic!("content: {error}")),
            enqueued_at: Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56Z")
                .unwrap_or_else(|error| panic!("timestamp: {error}")),
            extras: EntityExtras::default(),
        },
        route: AdmissionRoute::Ordinary,
    }
}

pub fn text(value: &str) -> NonEmptyString {
    NonEmptyString::new(value).unwrap_or_else(|error| panic!("text: {error}"))
}

pub fn fail(error: &RuntimeError) -> ! {
    panic!("runtime mutation: {error}")
}

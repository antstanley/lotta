use super::ConversationQueue;
use lotta_domain::{
    BoundedJsonValue, EntityExtras, NonEmptyString, QueueItem, QueueItemKind, QueueItemSource,
    Timestamp,
};
use serde_json::json;

pub fn text(value: &str) -> NonEmptyString {
    NonEmptyString::new(value).unwrap_or_else(|error| panic!("test text: {error}"))
}

pub fn item(number: usize, kind: QueueItemKind) -> QueueItem {
    item_with(number, kind, QueueItemSource::User, number)
}

pub fn item_with(
    number: usize,
    kind: QueueItemKind,
    source: QueueItemSource,
    client: usize,
) -> QueueItem {
    QueueItem {
        id: text(&format!("item-{number}")),
        client_message_id: text(&format!("client-{client}")),
        kind,
        source,
        content: BoundedJsonValue::new(json!({"number": number}))
            .unwrap_or_else(|error| panic!("test content: {error}")),
        enqueued_at: Timestamp::parse_persisted_rfc3339("2026-08-14T12:34:56Z")
            .unwrap_or_else(|error| panic!("test timestamp: {error}")),
        extras: EntityExtras::default(),
    }
}

pub fn ids(queue: &ConversationQueue) -> Vec<String> {
    queue
        .items()
        .map(|stored| stored.id.as_str().to_owned())
        .collect()
}

use super::test_support::{ids, item, text};
use super::*;
use lotta_domain::QueueDropReason;

fn full() -> ConversationQueue {
    let mut queue = ConversationQueue::default();
    for number in 0..300 {
        let _mutation = queue
            .enqueue(item(number, QueueItemKind::ApprovalResult))
            .unwrap_or_else(|error| fail(&error));
    }
    queue
}

#[test]
fn rejects_at_hard_max_before_soft_for_every_kind() {
    for kind in [QueueItemKind::OverlayAction, QueueItemKind::Message] {
        let mut queue = full();
        let before = ids(&queue);
        let mutation = queue
            .enqueue(item(300, kind))
            .unwrap_or_else(|error| fail(&error));
        assert!(matches!(
            mutation.event(),
            QueueMutationEvent::Dropped(_, QueueDropReason::BufferLimit)
        ));
        assert_eq!(ids(&queue), before);
        assert_eq!(mutation.snapshot().items().len(), 300);
        assert_eq!(mutation.snapshot().revision(), 301);
    }
}

#[test]
fn same_id_at_hard_max_is_buffer_limit() {
    let mut queue = full();
    let duplicate = item(10, QueueItemKind::Message);
    let mutation = queue
        .enqueue(duplicate)
        .unwrap_or_else(|error| fail(&error));
    assert!(matches!(
        mutation.event(),
        QueueMutationEvent::Dropped(_, QueueDropReason::BufferLimit)
    ));
    assert_eq!(queue.len(), 300);
}

#[test]
fn reasons_serialize_exactly() {
    assert_eq!(
        serde_json::to_string(&QueueDropReason::BufferLimit).unwrap(),
        "\"buffer_limit\""
    );
    assert_eq!(
        serde_json::to_string(&QueueDropReason::StaleGeneration).unwrap(),
        "\"stale_generation\""
    );
}

#[test]
fn records_stale_generation_and_missing_is_atomic() {
    let mut queue = ConversationQueue::default();
    let _mutation = queue
        .enqueue(item(1, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    let mutation = queue
        .drop_stale(&text("item-1"))
        .unwrap_or_else(|error| fail(&error))
        .unwrap_or_else(|| panic!("stored item missing"));
    assert!(matches!(
        mutation.event(),
        QueueMutationEvent::Dropped(_, QueueDropReason::StaleGeneration)
    ));
    assert_eq!(mutation.snapshot().revision(), 2);
    assert_eq!(queue.len(), 0);
    assert!(
        queue
            .drop_stale(&text("missing"))
            .unwrap_or_else(|error| fail(&error))
            .is_none()
    );
    let next = queue
        .enqueue(item(2, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    assert_eq!(next.snapshot().revision(), 3);
}

fn fail(error: &RuntimeError) -> ! {
    panic!("queue mutation: {error}")
}

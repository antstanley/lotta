use super::test_support::{item, text};
use super::*;
use lotta_domain::{QueueDropReason, QueueRemovalDisposition};

#[test]
fn enqueue_enqueue_cancel_has_exact_revisions() {
    let mut queue = ConversationQueue::default();
    let first = queue
        .enqueue(item(1, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    let second = queue
        .enqueue(item(2, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    let third = queue
        .cancel(&text("item-1"))
        .unwrap_or_else(|error| fail(&error))
        .unwrap_or_else(|| panic!("cancel target missing"));
    assert_eq!(first.snapshot().revision(), 1);
    assert_eq!(second.snapshot().revision(), 2);
    assert_eq!(third.snapshot().revision(), 3);
    assert!(matches!(
        third.event(),
        QueueMutationEvent::Removed(_, QueueRemovalDisposition::Cancelled)
    ));
    assert_eq!(first.snapshot().items().len(), 1);
    assert_eq!(second.snapshot().items().len(), 2);
    assert_eq!(third.snapshot().items().len(), 1);
}

#[test]
fn dispositions_serialize_exactly() {
    assert_eq!(
        serde_json::to_string(&QueueRemovalDisposition::Dequeued).unwrap(),
        "\"dequeued\""
    );
    assert_eq!(
        serde_json::to_string(&QueueRemovalDisposition::Cancelled).unwrap(),
        "\"cancelled\""
    );
}

#[test]
fn remove_and_dequeue_emit_dequeued() {
    let mut queue = ConversationQueue::default();
    let _mutation = queue
        .enqueue(item(1, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    let _mutation = queue
        .enqueue(item(2, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    let removed = queue
        .remove(&text("item-2"))
        .unwrap_or_else(|error| fail(&error))
        .unwrap_or_else(|| panic!("remove target missing"));
    let dequeued = queue
        .dequeue()
        .unwrap_or_else(|error| fail(&error))
        .unwrap_or_else(|| panic!("dequeue target missing"));
    for mutation in [removed, dequeued] {
        assert!(matches!(
            mutation.event(),
            QueueMutationEvent::Removed(_, QueueRemovalDisposition::Dequeued)
        ));
    }
}

#[test]
fn replacement_and_hard_drop_each_advance_once() {
    let mut soft = ConversationQueue::default();
    for number in 0..100 {
        let _mutation = soft
            .enqueue(item(number, QueueItemKind::Message))
            .unwrap_or_else(|error| fail(&error));
    }
    let replacement = soft
        .enqueue(item(100, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    assert_eq!(replacement.snapshot().revision(), 101);
    assert!(matches!(
        replacement.event(),
        QueueMutationEvent::Replaced { .. }
    ));

    let mut hard = ConversationQueue::default();
    for number in 0..300 {
        let _mutation = hard
            .enqueue(item(number, QueueItemKind::ApprovalResult))
            .unwrap_or_else(|error| fail(&error));
    }
    let drop = hard
        .enqueue(item(300, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    assert_eq!(drop.snapshot().revision(), 301);
    assert!(matches!(
        drop.event(),
        QueueMutationEvent::Dropped(_, QueueDropReason::BufferLimit)
    ));
}

#[test]
fn duplicate_item_id_below_hard_is_atomic() {
    let mut queue = ConversationQueue::default();
    let _mutation = queue
        .enqueue(item(1, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    let result = queue.enqueue(item(1, QueueItemKind::OverlayAction));
    assert_eq!(
        result,
        Err(RuntimeError::InvalidData {
            context: "queue_item_id_duplicate".into()
        })
    );
    assert_eq!(queue.len(), 1);
    let mutation = queue
        .enqueue(item(2, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    assert_eq!(mutation.snapshot().revision(), 2);
}

fn fail(error: &RuntimeError) -> ! {
    panic!("queue mutation: {error}")
}

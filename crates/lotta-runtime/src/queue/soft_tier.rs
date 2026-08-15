use super::test_support::{ids, item, item_with};
use super::*;
use lotta_domain::{QueueDropReason, QueueItemSource};

#[test]
fn coalescable_replaces_oldest_coalescable() {
    let mut queue = ConversationQueue::default();
    for number in 0..100 {
        let kind = if number == 37 {
            QueueItemKind::CronPrompt
        } else {
            QueueItemKind::ApprovalResult
        };
        let _mutation = queue
            .enqueue(item(number, kind))
            .unwrap_or_else(|error| fail(&error));
    }
    let before = ids(&queue);
    let incoming = item(100, QueueItemKind::Message);
    let mutation = queue
        .enqueue(incoming.clone())
        .unwrap_or_else(|error| fail(&error));
    let QueueMutationEvent::Replaced {
        dropped,
        enqueued,
        reason,
    } = mutation.event()
    else {
        panic!("expected replacement");
    };
    assert_eq!(dropped.id.as_str(), "item-37");
    assert_eq!(enqueued, &incoming);
    assert_eq!(*reason, QueueDropReason::BufferLimit);
    let after = ids(&queue);
    assert_eq!(queue.len(), 100);
    assert_eq!(&after[..37], &before[..37]);
    assert_eq!(&after[37..99], &before[38..]);
    assert_eq!(after[99], "item-100");
    assert_eq!(mutation.snapshot().revision(), 101);
    assert_eq!(mutation.snapshot().items().len(), 100);
}

#[test]
fn barrier_passes_soft_level() {
    let mut queue = ConversationQueue::default();
    for number in 0..100 {
        let _mutation = queue
            .enqueue(item(number, QueueItemKind::ApprovalResult))
            .unwrap_or_else(|error| fail(&error));
    }
    let mutation = queue
        .enqueue(item(100, QueueItemKind::OverlayAction))
        .unwrap_or_else(|error| fail(&error));
    assert!(matches!(mutation.event(), QueueMutationEvent::Enqueued(_)));
    assert_eq!(queue.len(), 101);
}

#[test]
fn all_barriers_coalescable_falls_through() {
    let mut queue = ConversationQueue::default();
    for number in 0..100 {
        let _mutation = queue
            .enqueue(item(number, QueueItemKind::ApprovalResult))
            .unwrap_or_else(|error| fail(&error));
    }
    let mutation = queue
        .enqueue(item(100, QueueItemKind::TaskNotification))
        .unwrap_or_else(|error| fail(&error));
    assert!(matches!(mutation.event(), QueueMutationEvent::Enqueued(_)));
    assert_eq!(queue.len(), 101);
}

#[test]
fn classification_is_kind_only() {
    let mut queue = ConversationQueue::default();
    for number in 0..100 {
        let stored = item_with(
            number,
            if number == 21 {
                QueueItemKind::Message
            } else {
                QueueItemKind::ApprovalResult
            },
            QueueItemSource::Channel,
            number + 1000,
        );
        let _mutation = queue.enqueue(stored).unwrap_or_else(|error| fail(&error));
    }
    let incoming = item_with(100, QueueItemKind::Message, QueueItemSource::System, 9999);
    let mutation = queue.enqueue(incoming).unwrap_or_else(|error| fail(&error));
    let QueueMutationEvent::Replaced { dropped, .. } = mutation.event() else {
        panic!("expected replacement");
    };
    assert_eq!(dropped.id.as_str(), "item-21");
}

#[test]
fn boundary_is_99_then_100() {
    let mut queue = ConversationQueue::default();
    for number in 0..99 {
        let _mutation = queue
            .enqueue(item(number, QueueItemKind::Message))
            .unwrap_or_else(|error| fail(&error));
    }
    let at_soft = queue
        .enqueue(item(99, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    assert!(matches!(at_soft.event(), QueueMutationEvent::Enqueued(_)));
    let above_soft = queue
        .enqueue(item(100, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    assert!(matches!(
        above_soft.event(),
        QueueMutationEvent::Replaced { .. }
    ));
    assert_eq!(queue.len(), 100);
}

fn fail(error: &RuntimeError) -> ! {
    panic!("queue mutation: {error}")
}

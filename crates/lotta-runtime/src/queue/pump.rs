use super::test_support::item;
use super::*;
use crate::{ListenerRuntime, RuntimeError};
use lotta_domain::{AgentId, ConversationId, RuntimeScope};

#[test]
fn pump_requires_idle() {
    for state in [
        TurnStateKind::Active,
        TurnStateKind::Command,
        TurnStateKind::Cancelling,
    ] {
        let mut queue = ConversationQueue::default();
        let _mutation = queue
            .enqueue(item(1, QueueItemKind::Message))
            .unwrap_or_else(|error| fail(&error));
        assert!(
            queue
                .pump(state)
                .unwrap_or_else(|error| fail(&error))
                .is_none()
        );
        assert_eq!(queue.len(), 1);
        let next = queue
            .enqueue(item(2, QueueItemKind::Message))
            .unwrap_or_else(|error| fail(&error));
        assert_eq!(next.snapshot().revision(), 2);
    }
}

#[test]
fn yields_at_batch_max() {
    let mut queue = ConversationQueue::default();
    for number in 1..=65 {
        let _mutation = queue
            .enqueue(item(number, QueueItemKind::Message))
            .unwrap_or_else(|error| fail(&error));
    }
    let first = queue
        .pump(TurnStateKind::Idle)
        .unwrap_or_else(|error| fail(&error))
        .unwrap_or_else(|| panic!("first pump missing"));
    assert_eq!(first.directive(), PumpDirective::YieldBeforeNextBatch);
    assert_pumped_ids(first.mutation(), 1..=64);
    let second = queue
        .pump(TurnStateKind::Idle)
        .unwrap_or_else(|error| fail(&error))
        .unwrap_or_else(|| panic!("second pump missing"));
    assert_eq!(second.directive(), PumpDirective::Complete);
    assert_pumped_ids(second.mutation(), 65..=65);
}

#[test]
fn sixty_four_then_barrier_yields() {
    let mut queue = ConversationQueue::default();
    for number in 1..=64 {
        let _mutation = queue
            .enqueue(item(number, QueueItemKind::Message))
            .unwrap_or_else(|error| fail(&error));
    }
    let _mutation = queue
        .enqueue(item(65, QueueItemKind::ApprovalResult))
        .unwrap_or_else(|error| fail(&error));
    let first = queue
        .pump(TurnStateKind::Idle)
        .unwrap_or_else(|error| fail(&error))
        .unwrap();
    assert_eq!(first.directive(), PumpDirective::YieldBeforeNextBatch);
    assert_eq!(queue.len(), 1);
    let second = queue
        .pump(TurnStateKind::Idle)
        .unwrap_or_else(|error| fail(&error))
        .unwrap();
    assert_eq!(second.directive(), PumpDirective::Complete);
    assert_pumped_ids(second.mutation(), 65..=65);
}

#[test]
fn barrier_head_is_alone_and_not_crossed() {
    let mut queue = ConversationQueue::default();
    let _mutation = queue
        .enqueue(item(1, QueueItemKind::ApprovalResult))
        .unwrap_or_else(|error| fail(&error));
    let _mutation = queue
        .enqueue(item(2, QueueItemKind::Message))
        .unwrap_or_else(|error| fail(&error));
    let first = queue
        .pump(TurnStateKind::Idle)
        .unwrap_or_else(|error| fail(&error))
        .unwrap();
    assert_pumped_ids(first.mutation(), 1..=1);
    assert_eq!(
        queue.items().next().map(|stored| stored.id.as_str()),
        Some("item-2")
    );
}

#[test]
fn registry_pump_uses_live_state_and_rejects_stale_handle() {
    let (mut runtime, handle) = runtime();
    let command_lease = runtime
        .lifecycle_mut(&handle)
        .unwrap_or_else(|error| fail(&error))
        .begin_command()
        .unwrap_or_else(|error| fail(&error));
    let _outcome = runtime
        .admit(
            &handle,
            crate::AdmissionRequest {
                item: item(1, QueueItemKind::Message),
                route: crate::AdmissionRoute::Ordinary,
            },
        )
        .unwrap_or_else(|error| fail(&error));
    assert!(
        runtime
            .pump_queue(&handle)
            .unwrap_or_else(|error| fail(&error))
            .is_none()
    );
    assert_eq!(
        runtime
            .queue(&handle)
            .map(|queue| queue.lock().unwrap().len()),
        Some(1)
    );
    runtime
        .lifecycle_mut(&handle)
        .unwrap_or_else(|error| fail(&error))
        .finish_command(&command_lease)
        .unwrap_or_else(|error| fail(&error));
    assert!(
        runtime
            .pump_queue(&handle)
            .unwrap_or_else(|error| fail(&error))
            .is_some()
    );
    assert!(
        runtime
            .queue(&handle)
            .is_some_and(|queue| queue.lock().unwrap().is_empty())
    );
    let mut replacement = ListenerRuntime::new();
    let other_scope = RuntimeScope::new(
        AgentId::accept("other").unwrap(),
        ConversationId::generate(2).unwrap(),
        None,
    );
    let other = replacement
        .get_or_create(&other_scope, uuid::Uuid::from_u128(2))
        .unwrap();
    assert!(replacement.pump_queue(&other).unwrap().is_none());
    assert!(replacement.pump_queue(&handle).is_err());
}

fn runtime() -> (ListenerRuntime, crate::RuntimeHandle) {
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

fn assert_pumped_ids(mutation: &QueueMutation, expected: std::ops::RangeInclusive<usize>) {
    let QueueMutationEvent::Pumped(items) = mutation.event() else {
        panic!("expected pump")
    };
    let actual: Vec<_> = items.iter().map(|stored| stored.id.as_str()).collect();
    let expected: Vec<_> = expected.map(|number| format!("item-{number}")).collect();
    assert_eq!(
        actual,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
}

fn fail(error: &RuntimeError) -> ! {
    panic!("runtime mutation: {error}")
}

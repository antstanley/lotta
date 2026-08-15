use super::test_support::{fail, request, runtime};
use super::*;
use lotta_domain::{InputDisposition, QueueDropReason, QueueItemKind, QueueItemSource, RunId};

#[test]
fn start_idle_empty() {
    let (mut runtime, handle) = runtime();
    let outcome = runtime
        .admit(
            &handle,
            request(1, QueueItemKind::Message, QueueItemSource::User),
        )
        .unwrap_or_else(|error| fail(&error));
    assert!(matches!(outcome, AdmissionOutcome::Start(_)));
    assert_eq!(outcome.disposition(), InputDisposition::Started);
    assert!(runtime.queue(&handle).unwrap().is_empty());
}

#[test]
fn occupied_states_enqueue() {
    for state in [
        TurnStateKind::Active,
        TurnStateKind::Command,
        TurnStateKind::Cancelling,
    ] {
        let (mut runtime, handle) = runtime();
        let lease = runtime
            .lifecycle_mut(&handle)
            .unwrap_or_else(|error| fail(&error))
            .begin_turn("turn".into(), RunId::generate_sequence(1).unwrap())
            .unwrap_or_else(|error| fail(&error));
        if state == TurnStateKind::Cancelling {
            runtime
                .lifecycle_mut(&handle)
                .unwrap()
                .request_cancellation(&lease)
                .unwrap();
        } else if state == TurnStateKind::Command {
            runtime
                .lifecycle_mut(&handle)
                .unwrap()
                .finish_turn(&lease, lotta_domain::StopReason::new("completed").unwrap())
                .unwrap();
            runtime
                .lifecycle_mut(&handle)
                .unwrap()
                .begin_command()
                .unwrap();
        }
        let outcome = runtime
            .admit(
                &handle,
                request(1, QueueItemKind::Message, QueueItemSource::User),
            )
            .unwrap_or_else(|error| fail(&error));
        assert!(matches!(outcome, AdmissionOutcome::Queued(_)));
        assert_eq!(runtime.queue(&handle).unwrap().len(), 1);
    }
}

#[test]
fn idle_with_pending_enqueues_at_tail() {
    let (mut runtime, handle) = runtime();
    let _mutation = runtime
        .current_entry_mut(&handle)
        .unwrap()
        .queue
        .enqueue(request(1, QueueItemKind::Message, QueueItemSource::User).item)
        .unwrap();
    let outcome = runtime
        .admit(
            &handle,
            request(2, QueueItemKind::Message, QueueItemSource::User),
        )
        .unwrap();
    assert!(matches!(outcome, AdmissionOutcome::Queued(_)));
    let ids: Vec<_> = runtime
        .queue(&handle)
        .unwrap()
        .items()
        .map(|item| item.id.as_str())
        .collect();
    assert_eq!(ids, ["item-1", "item-2"]);
}

#[test]
fn current_continuation_and_control_are_direct() {
    for control in [false, true] {
        let (mut runtime, handle) = runtime();
        let lease = runtime
            .lifecycle_mut(&handle)
            .unwrap()
            .begin_turn("turn".into(), RunId::generate_sequence(1).unwrap())
            .unwrap();
        let mut input = request(1, QueueItemKind::Message, QueueItemSource::User);
        input.route = if control {
            AdmissionRoute::Control(lease)
        } else {
            AdmissionRoute::Continuation(lease)
        };
        let outcome = runtime.admit(&handle, input).unwrap();
        assert!(matches!(
            outcome,
            AdmissionOutcome::Control(_) | AdmissionOutcome::Continue(_)
        ));
        assert!(runtime.queue(&handle).unwrap().is_empty());
    }
}

#[test]
fn stale_routes_reject_without_lifecycle_mutation() {
    for control in [false, true] {
        let (mut runtime, handle) = runtime();
        let stale = runtime
            .lifecycle_mut(&handle)
            .unwrap()
            .begin_turn("turn".into(), RunId::generate_sequence(1).unwrap())
            .unwrap();
        runtime
            .lifecycle_mut(&handle)
            .unwrap()
            .request_cancellation(&stale)
            .unwrap();
        runtime
            .lifecycle_mut(&handle)
            .unwrap()
            .finish_turn(&stale, lotta_domain::StopReason::new("completed").unwrap())
            .unwrap();
        let before = runtime.lifecycle(&handle).unwrap().projection().state();
        let mut input = request(1, QueueItemKind::Message, QueueItemSource::User);
        input.route = if control {
            AdmissionRoute::Control(stale)
        } else {
            AdmissionRoute::Continuation(stale)
        };
        let outcome = runtime.admit(&handle, input).unwrap();
        assert!(matches!(
            outcome,
            AdmissionOutcome::Rejected {
                reason: QueueDropReason::StaleGeneration,
                ..
            }
        ));
        assert_eq!(
            runtime.lifecycle(&handle).unwrap().projection().state(),
            before
        );
    }
}

#[test]
fn stale_handle_is_atomic() {
    let (mut runtime, handle) = runtime();
    let mut other = ListenerRuntime::new();
    let stale_scope = lotta_domain::RuntimeScope::new(
        lotta_domain::AgentId::accept("other").unwrap(),
        lotta_domain::ConversationId::generate(2).unwrap(),
        None,
    );
    let stale = other
        .get_or_create(&stale_scope, uuid::Uuid::from_u128(2))
        .unwrap();
    assert!(
        runtime
            .admit(
                &stale,
                request(1, QueueItemKind::Message, QueueItemSource::User)
            )
            .is_err()
    );
    assert!(runtime.queue(&handle).unwrap().is_empty());
}

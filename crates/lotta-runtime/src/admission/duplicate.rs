use super::test_support::{request, runtime};
use super::*;
use lotta_domain::{InputDisposition, QueueItemKind, QueueItemSource, RunId};

#[test]
fn started_duplicate_replays_without_second_effect() {
    let (mut runtime, handle) = runtime();
    assert_replay(
        &mut runtime,
        &handle,
        request(1, QueueItemKind::Message, QueueItemSource::User),
        InputDisposition::Started,
    );
    assert!(runtime.queue(&handle).unwrap().lock().unwrap().is_empty());
    assert_eq!(
        runtime
            .current_entry(&handle)
            .unwrap()
            .admission_history
            .admission_count(),
        1
    );
}

#[test]
fn queued_duplicate_replays_without_second_item_or_revision() {
    let (mut runtime, handle) = runtime();
    runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .begin_turn("turn".into(), RunId::generate_sequence(1).unwrap())
        .unwrap();
    assert_replay(
        &mut runtime,
        &handle,
        request(1, QueueItemKind::Message, QueueItemSource::User),
        InputDisposition::Queued,
    );
    assert_eq!(runtime.queue(&handle).unwrap().lock().unwrap().len(), 1);
    let next = runtime
        .current_entry_mut(&handle)
        .unwrap()
        .queue
        .lock()
        .unwrap()
        .enqueue(request(2, QueueItemKind::Message, QueueItemSource::User).item)
        .unwrap();
    assert_eq!(next.snapshot().revision(), 2);
    assert_eq!(
        runtime
            .current_entry(&handle)
            .unwrap()
            .admission_history
            .admission_count(),
        1
    );
}

#[test]
fn hard_rejected_duplicate_replays_without_second_drop() {
    let (mut runtime, handle) = runtime();
    runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .begin_command()
        .unwrap();
    for number in 0..300 {
        let _mutation = runtime
            .current_entry_mut(&handle)
            .unwrap()
            .queue
            .lock()
            .unwrap()
            .enqueue(
                request(
                    number,
                    QueueItemKind::ApprovalResult,
                    QueueItemSource::System,
                )
                .item,
            )
            .unwrap();
    }
    assert_replay(
        &mut runtime,
        &handle,
        request(400, QueueItemKind::Message, QueueItemSource::User),
        InputDisposition::Rejected,
    );
    assert_eq!(runtime.queue(&handle).unwrap().lock().unwrap().len(), 300);
    let mutation = runtime
        .current_entry_mut(&handle)
        .unwrap()
        .queue
        .lock()
        .unwrap()
        .enqueue(request(401, QueueItemKind::Message, QueueItemSource::User).item)
        .unwrap();
    assert_eq!(mutation.snapshot().revision(), 302);
    assert_eq!(
        runtime
            .current_entry(&handle)
            .unwrap()
            .admission_history
            .admission_count(),
        1
    );
}

fn assert_replay(
    runtime: &mut ListenerRuntime,
    handle: &RuntimeHandle,
    input: AdmissionRequest,
    disposition: InputDisposition,
) {
    let duplicate = input.clone();
    let first = runtime.admit(handle, input).unwrap();
    assert_eq!(first.disposition(), disposition);
    let second = runtime.admit(handle, duplicate).unwrap();
    assert_eq!(second, AdmissionOutcome::Duplicate(disposition));
}

use super::test_support::{request, runtime};
use super::*;
use lotta_domain::{QueueItemKind, QueueItemSource};

#[test]
fn task_notification_uses_ordinary_queue() {
    assert_source(
        QueueItemKind::TaskNotification,
        QueueItemSource::TaskNotification,
    );
}

#[test]
fn cron_prompt_uses_ordinary_queue() {
    assert_source(QueueItemKind::CronPrompt, QueueItemSource::Cron);
}

#[test]
fn approval_result_uses_ordinary_queue() {
    assert_source(QueueItemKind::ApprovalResult, QueueItemSource::System);
}

#[test]
fn overlay_action_uses_ordinary_queue() {
    assert_source(QueueItemKind::OverlayAction, QueueItemSource::System);
}

#[test]
fn mod_continuation_uses_ordinary_queue() {
    assert_source(QueueItemKind::ModContinue, QueueItemSource::System);
}

#[test]
fn all_sources_preserve_fifo() {
    let (mut runtime, handle) = runtime();
    let cases = [
        (
            QueueItemKind::TaskNotification,
            QueueItemSource::TaskNotification,
        ),
        (QueueItemKind::CronPrompt, QueueItemSource::Cron),
        (QueueItemKind::ApprovalResult, QueueItemSource::System),
        (QueueItemKind::OverlayAction, QueueItemSource::System),
        (QueueItemKind::ModContinue, QueueItemSource::System),
    ];
    for (number, (kind, source)) in cases.into_iter().enumerate() {
        let outcome = runtime
            .admit(&handle, request(number, kind, source))
            .unwrap();
        assert!(matches!(outcome, AdmissionOutcome::Queued(_)));
    }
    let actual: Vec<_> = runtime
        .queue(&handle)
        .unwrap()
        .items()
        .map(|item| (item.kind, item.source))
        .collect();
    assert_eq!(actual, cases);
}

fn assert_source(kind: QueueItemKind, source: QueueItemSource) {
    let (mut runtime, handle) = runtime();
    let before = runtime.lifecycle(&handle).unwrap().projection().state();
    let outcome = runtime.admit(&handle, request(1, kind, source)).unwrap();
    assert!(matches!(outcome, AdmissionOutcome::Queued(_)));
    let stored = runtime.queue(&handle).unwrap().items().next().unwrap();
    assert_eq!((stored.kind, stored.source), (kind, source));
    assert_eq!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        before
    );
}

use super::test_support::*;
use super::*;
use crate::RuntimeError;
use crate::boundary::ProviderEventText;
use crate::ports::*;
use std::time::Duration;

#[tokio::test]
async fn stop_only_retries_exact_budget_and_forwards_one_terminal() {
    let time = FakeTime::default();
    let events = Events::default();
    let port = ScriptedPort::new(vec![vec![stop()], vec![stop()], vec![stop()]]);
    let (terminal, output) = run(RetryPolicy::default(), &time, &events, &port)
        .await
        .unwrap();
    assert!(matches!(terminal, RetryTerminal::Failure { .. }));
    assert_eq!(port.calls(), 3);
    assert_eq!(time.delays(), [500, 1_000]);
    assert_eq!(output.len(), 1);
    assert!(matches!(output[0], ProviderEvent::Stop { .. }));
}

#[tokio::test]
async fn no_events_is_empty_and_retries() {
    let time = FakeTime::default();
    let events = Events::default();
    let port = ScriptedPort::new(vec![Vec::new(), Vec::new(), Vec::new()]);
    let (terminal, output) = run(RetryPolicy::default(), &time, &events, &port)
        .await
        .unwrap();
    assert!(matches!(terminal, RetryTerminal::Failure { .. }));
    assert_eq!(port.calls(), 3);
    assert_eq!(time.delays(), [500, 1_000]);
    assert!(output.is_empty());
}

#[tokio::test]
async fn exact_retry_budget_and_one_terminal_output() {
    let time = FakeTime::default();
    let events = Events::default();
    let port = ScriptedPort::new(vec![
        vec![failure(ProviderFailureKind::Transient)],
        vec![failure(ProviderFailureKind::Transient)],
        vec![failure(ProviderFailureKind::Transient)],
        vec![failure(ProviderFailureKind::Transient)],
    ]);
    let (terminal, output) = run(RetryPolicy::default(), &time, &events, &port)
        .await
        .unwrap();
    assert!(matches!(terminal, RetryTerminal::Failure { .. }));
    assert!(output.is_empty());
    assert_eq!(port.calls(), 4);
    assert_eq!(events.values.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn cancellation_before_attempt_is_terminal() {
    let time = FakeTime::default();
    let events = Events::default();
    let port = ScriptedPort::new(vec![vec![stop()]]);
    let req = request(10_000);
    req.cancellation.cancel();
    let (sink, _receiver) = provider_event_channel(1, &req.cancellation).unwrap();
    let executor = RetryExecutor::new(&time, &time, &events, RetryPolicy::default(), None);
    let result = executor
        .execute(
            ProviderRoute::new("native", "openai"),
            &port,
            None,
            req,
            sink,
        )
        .await;
    assert!(matches!(result, Err(RuntimeError::Cancelled { .. })));
    assert_eq!(port.calls(), 0);
}

#[tokio::test]
async fn content_then_stop_forwards_live_in_order() {
    let time = FakeTime::default();
    let events = Events::default();
    let port = ScriptedPort::new(vec![vec![
        ProviderEvent::TextDelta {
            text: ProviderEventText::new("content".into()).unwrap(),
        },
        stop(),
    ]]);
    let (terminal, output) = run(RetryPolicy::default(), &time, &events, &port)
        .await
        .unwrap();
    assert_eq!(terminal, RetryTerminal::Success);
    assert!(matches!(output[0], ProviderEvent::TextDelta { .. }));
    assert!(matches!(output[1], ProviderEvent::Stop { .. }));
}

#[tokio::test]
async fn held_stop_then_content_is_ordered_schema_terminal() {
    let time = FakeTime::default();
    let events = Events::default();
    let port = ScriptedPort::new(vec![vec![
        stop(),
        ProviderEvent::TextDelta {
            text: ProviderEventText::new("late".into()).unwrap(),
        },
    ]]);
    let (terminal, output) = run(RetryPolicy::default(), &time, &events, &port)
        .await
        .unwrap();
    assert!(
        matches!(terminal, RetryTerminal::Failure { ref failure, .. }
        if failure.kind == ProviderFailureKind::Schema)
    );
    assert!(matches!(
        output.as_slice(),
        [ProviderEvent::Stop { .. }, ProviderEvent::TextDelta { .. }]
    ));
    assert_eq!(port.calls(), 1);
}

#[tokio::test]
async fn held_stop_then_stop_is_ordered_schema_terminal() {
    let time = FakeTime::default();
    let events = Events::default();
    let port = ScriptedPort::new(vec![vec![stop(), stop()]]);
    let (terminal, output) = run(RetryPolicy::default(), &time, &events, &port)
        .await
        .unwrap();
    assert!(
        matches!(terminal, RetryTerminal::Failure { ref failure, .. }
        if failure.kind == ProviderFailureKind::Schema)
    );
    assert!(matches!(
        output.as_slice(),
        [ProviderEvent::Stop { .. }, ProviderEvent::Stop { .. }]
    ));
    assert_eq!(port.calls(), 1);
}

#[tokio::test]
async fn partial_output_failure_is_forwarded_and_never_retried() {
    let time = FakeTime::default();
    let events = Events::default();
    let port = ScriptedPort::new(vec![vec![
        ProviderEvent::TextDelta {
            text: ProviderEventText::new("partial".into()).unwrap(),
        },
        failure(ProviderFailureKind::Transient),
    ]]);
    let (terminal, output) = run(RetryPolicy::default(), &time, &events, &port)
        .await
        .unwrap();
    assert!(matches!(terminal, RetryTerminal::Failure { .. }));
    assert_eq!(output.len(), 1);
    assert!(matches!(output[0], ProviderEvent::TextDelta { .. }));
    assert_eq!(port.calls(), 1);
    assert!(events.values.lock().unwrap().is_empty());
}

#[tokio::test]
async fn remaining_deadline_is_set_before_every_attempt() {
    let time = FakeTime::default();
    let events = Events::default();
    let port = ScriptedPort::new(vec![
        vec![failure(ProviderFailureKind::Transient)],
        vec![
            ProviderEvent::TextDelta {
                text: ProviderEventText::new("done".into()).unwrap(),
            },
            stop(),
        ],
    ]);
    run(
        RetryPolicy {
            retries_max: 1,
            deadline_ms: 10_000,
            ..RetryPolicy::default()
        },
        &time,
        &events,
        &port,
    )
    .await
    .unwrap();
    assert_eq!(port.deadlines(), [10_000, 9_000]);
}

#[tokio::test]
async fn delayed_event_is_visible_before_provider_completion() {
    struct DelayedPort;
    impl ProviderPort for DelayedPort {
        fn stream(&self, _: ProviderRequest, sink: ProviderEventSink) -> PortFuture<'_, ()> {
            Box::pin(async move {
                sink.send(ProviderEvent::TextDelta {
                    text: ProviderEventText::new("first".into()).unwrap(),
                })
                .await?;
                tokio::time::sleep(Duration::from_mins(1)).await;
                sink.send(stop()).await
            })
        }
    }
    tokio::time::pause();
    let time = FakeTime::default();
    let events = Events::default();
    let req = request(100_000);
    let (sink, mut receiver) = provider_event_channel(1, &req.cancellation).unwrap();
    let executor = RetryExecutor::new(&time, &time, &events, RetryPolicy::default(), None);
    let execution = executor.execute(
        ProviderRoute::new("native", "openai"),
        &DelayedPort,
        None,
        req,
        sink,
    );
    tokio::pin!(execution);
    let first = tokio::select! {
        event = receiver.receive() => event.unwrap().unwrap(),
        result = &mut execution => panic!("provider completed before first event: {result:?}"),
    };
    assert!(matches!(first, ProviderEvent::TextDelta { .. }));
    tokio::time::advance(Duration::from_mins(1)).await;
    assert_eq!(execution.await.unwrap(), RetryTerminal::Success);
}

#[test]
fn provider_retry_after_mapping_is_typed() {
    let context = ProviderErrorContext::new(
        crate::boundary::ProviderName::new("rate_limit".into()).unwrap(),
        ProviderEventText::new("safe".into()).unwrap(),
    )
    .with_retry_after(RetryAfter::Milliseconds(2_500));
    let failure = ProviderError::RateLimit(context).into_failure();
    assert_eq!(failure.retry_after, Some(RetryAfter::Milliseconds(2_500)));
}

#[test]
fn provider_error_mapping_is_canonical() {
    let mapped = [
        (
            ProviderError::Authentication(context()),
            ProviderFailureKind::Auth,
        ),
        (
            ProviderError::InvalidRequest(context()),
            ProviderFailureKind::Invalid,
        ),
        (
            ProviderError::RateLimit(context()),
            ProviderFailureKind::Busy,
        ),
        (
            ProviderError::Timeout(context()),
            ProviderFailureKind::Transient,
        ),
        (
            ProviderError::Protocol(context()),
            ProviderFailureKind::Schema,
        ),
        (
            ProviderError::Unknown(context()),
            ProviderFailureKind::Terminal,
        ),
    ];
    for (error, expected) in mapped {
        assert_eq!(error.into_failure().kind, expected);
    }
}

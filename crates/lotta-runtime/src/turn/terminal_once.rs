use super::test_support::*;
use super::*;
use crate::RuntimeError;
use crate::ports::{ProviderEvent, StopReason};
use lotta_domain::{RunId, TurnStateKind};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn exactly_one_terminal_after_final_delta_and_release() {
    let provider = ScriptedProvider::new(vec![vec![
        ProviderEvent::TextDelta { text: text("last") },
        ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(
        *effects.events.lock().unwrap(),
        [
            TurnEvent::StreamDelta(TurnProjection::new(ProjectionKind::Text, text("last"))),
            TurnEvent::Finished {
                reason: StopReason::EndTurn
            }
        ]
    );
    assert_eq!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}

#[tokio::test]
async fn retry_executor_composes_with_task19_and_finishes_once() {
    use crate::retry::{
        Clock, EventSink, ProviderRoute, RetryEvent, RetryExecutor, RetryPolicy, Sleeper,
    };
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;

    struct Time;
    impl Clock for Time {
        fn monotonic_ms(&self) -> u64 {
            0
        }
        fn unix_epoch_ms(&self) -> u64 {
            0
        }
    }
    impl Sleeper for Time {
        fn sleep<'a>(
            &'a self,
            _: Duration,
            _: &'a tokio_util::sync::CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    #[derive(Default)]
    struct Notices(std::sync::Mutex<Vec<RetryEvent>>);
    impl EventSink for Notices {
        fn emit(&self, event: RetryEvent) -> crate::ports::PortFuture<'_, ()> {
            Box::pin(async move {
                self.0.lock().unwrap().push(event);
                Ok(())
            })
        }
    }

    let provider = ScriptedProvider::new(vec![
        vec![ProviderEvent::Error {
            error: crate::ports::ProviderError::Unavailable(crate::ports::ProviderErrorContext {
                retry_after: None,
                code: crate::boundary::ProviderName::new("temporary".into()).unwrap(),
                context: text("retry"),
            }),
        }],
        vec![
            ProviderEvent::TextDelta { text: text("done") },
            ProviderEvent::Stop {
                reason: StopReason::EndTurn,
            },
        ],
    ]);
    let time = Time;
    let notices = Notices::default();
    let executor = RetryExecutor::new(&time, &time, &notices, RetryPolicy::default(), None);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let ports = TurnPorts {
        provider: TurnProvider::Retrying {
            route: ProviderRoute::new("native", "openai"),
            source: &provider,
            fallbacks: Vec::new(),
            executor: std::sync::Arc::new(executor),
        },
        provider_start: None,
        tools: &tool,
        controller_tools: None,
        approvals: None,
        catalog: &catalog(&[]),
        effects: &effects,
        compaction: None,
        request_refresh: None,
    };
    let outcome = run_turn(&mut runtime, handle, lease, request(), ports)
        .await
        .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(notices.0.lock().unwrap().len(), 1);
    assert_eq!(
        effects
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, TurnEvent::Finished { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn production_retry_event_is_emitted_once() {
    let provider = ScriptedProvider::new(vec![
        vec![ProviderEvent::Error {
            error: crate::ports::ProviderError::Unavailable(crate::ports::ProviderErrorContext {
                retry_after: Some(crate::retry::RetryAfter::Milliseconds(0)),
                code: crate::boundary::ProviderName::new("temporary".into()).unwrap(),
                context: text("retry"),
            }),
        }],
        vec![
            ProviderEvent::TextDelta { text: text("done") },
            ProviderEvent::Stop {
                reason: StopReason::EndTurn,
            },
        ],
    ]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle,
        lease,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    let events = effects.events.lock().unwrap();
    let retry_index = events
        .iter()
        .position(|event| matches!(event, TurnEvent::Retry(_)))
        .unwrap();
    let delta_index = events
        .iter()
        .position(|event| matches!(event, TurnEvent::StreamDelta(_)))
        .unwrap();
    assert!(retry_index < delta_index);
}

#[tokio::test]
async fn cancelled_lease_suppresses_production_retry_event() {
    let req = request();
    let provider = ScriptedProvider::configured(vec![ProviderScript::CancelAfterFirst(
        vec![ProviderEvent::Error {
            error: crate::ports::ProviderError::Unavailable(crate::ports::ProviderErrorContext {
                retry_after: None,
                code: crate::boundary::ProviderName::new("temporary".into()).unwrap(),
                context: text("retry"),
            }),
        }],
        req.cancellation.clone(),
    )]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle,
        lease,
        req,
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(
        effects
            .events
            .lock()
            .unwrap()
            .iter()
            .all(|event| !matches!(event, TurnEvent::Retry(_)))
    );
}

#[tokio::test]
async fn stale_real_replacement_is_suppressed_before_dispatch() {
    let (mut runtime, handle, stale) = runtime();
    runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .finish_turn(&stale, lotta_domain::StopReason::new("done").unwrap())
        .unwrap();
    let current = runtime
        .lifecycle_mut(&handle)
        .unwrap()
        .begin_turn("next".into(), RunId::generate_sequence(2).unwrap())
        .unwrap();
    let provider = ScriptedProvider::new(vec![vec![
        ProviderEvent::TextDelta {
            text: text("finished"),
        },
        ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        stale,
        request(),
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Suppressed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(tool.calls.load(Ordering::SeqCst), 0);
    assert!(effects.events.lock().unwrap().is_empty());
    assert!(runtime.lifecycle(&handle).unwrap().is_current(&current));
}

#[tokio::test]
async fn cancellation_during_tool_execute_suppresses_all_later_effects() {
    let req = request();
    let token = req.cancellation.clone();
    let mut script = call_events("cancel", "tool");
    script.push(ProviderEvent::Stop {
        reason: StopReason::ToolUse,
    });
    let provider = ScriptedProvider::new(vec![script]);
    let tool = SequencingTool::configured(vec![Ok(outcome("ignored"))], Some(token));
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        req,
        TurnPorts::new(&provider, &tool, &catalog(&["tool"]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(effects.results.lock().unwrap().is_empty());
    assert_eq!(
        effects.events.lock().unwrap().as_slice(),
        &[TurnEvent::Failed {
            reason: super::TurnStopReason::UserCancellation,
        }]
    );
    assert_eq!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}

#[tokio::test]
async fn cancellation_during_finished_effect_still_linearizes_completion() {
    let req = request();
    let effects = RecordingEffects::configured(Some(req.cancellation.clone()), false);
    let provider = ScriptedProvider::new(vec![vec![
        ProviderEvent::TextDelta { text: text("done") },
        ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]]);
    let tool = SequencingTool::new(Vec::new());
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        req,
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(
        effects
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, TurnEvent::Finished { .. }))
            .count(),
        1
    );
    assert_eq!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}

#[tokio::test]
async fn cancellation_before_terminal_helper_suppresses_without_release() {
    let req = request();
    let provider = ScriptedProvider::configured(vec![ProviderScript::CancelBefore(
        vec![ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        }],
        req.cancellation.clone(),
    )]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let outcome = run_turn(
        &mut runtime,
        handle.clone(),
        lease,
        req,
        TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
    )
    .await
    .unwrap();
    assert_eq!(outcome, TurnRunOutcome::Completed);
    assert_eq!(
        effects.events.lock().unwrap().as_slice(),
        &[TurnEvent::Failed {
            reason: super::TurnStopReason::UserCancellation,
        }]
    );
    assert_eq!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}

#[tokio::test]
async fn finished_effect_failure_retains_active_owner() {
    let provider = ScriptedProvider::new(vec![vec![
        ProviderEvent::TextDelta { text: text("done") },
        ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]]);
    let tool = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::configured(None, true);
    let (mut runtime, handle, lease) = runtime();
    assert!(
        run_turn(
            &mut runtime,
            handle.clone(),
            lease,
            request(),
            TurnPorts::new(&provider, &tool, &catalog(&[]), &effects),
        )
        .await
        .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        effects
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, TurnEvent::Finished { .. }))
            .count(),
        0
    );
    assert_ne!(
        runtime.lifecycle(&handle).unwrap().projection().state(),
        TurnStateKind::Idle
    );
}

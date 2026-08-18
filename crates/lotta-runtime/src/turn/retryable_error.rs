use super::*;
use crate::RuntimeError;
use crate::ports::{ProviderError, ProviderErrorContext, ProviderEvent, StopReason};
use crate::retry::{
    Clock, EventSink, ProviderRoute, RetryAfter, RetryEvent, RetryExecutor, RetryPolicy, Sleeper,
};
use crate::turn::test_support::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct RecordedTime {
    now: std::sync::atomic::AtomicU64,
    order: Mutex<Vec<String>>,
    cancel_on_sleep: bool,
}

impl RecordedTime {
    fn cancelling() -> Self {
        Self {
            cancel_on_sleep: true,
            ..Self::default()
        }
    }

    fn delays(&self) -> Vec<u64> {
        self.order
            .lock()
            .unwrap()
            .iter()
            .filter_map(|value| value.strip_prefix("delay:")?.parse().ok())
            .collect()
    }
}

impl Clock for RecordedTime {
    fn monotonic_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }

    fn unix_epoch_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

impl Sleeper for RecordedTime {
    fn sleep<'a>(
        &'a self,
        duration: Duration,
        cancellation: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let delay = u64::try_from(duration.as_millis()).unwrap();
            self.order.lock().unwrap().push(format!("delay:{delay}"));
            if self.cancel_on_sleep {
                cancellation.cancel();
            } else {
                self.now.fetch_add(delay, Ordering::SeqCst);
            }
            Ok(())
        })
    }
}

struct RecordedEvents<'a>(&'a RecordedTime);

impl EventSink for RecordedEvents<'_> {
    fn emit(&self, event: RetryEvent) -> crate::ports::PortFuture<'_, ()> {
        Box::pin(async move {
            self.0
                .order
                .lock()
                .unwrap()
                .push(format!("retry:{}:{}", event.attempt, event.delay_ms));
            Ok(())
        })
    }
}

fn error(error: ProviderError) -> Vec<ProviderEvent> {
    vec![ProviderEvent::Error { error }]
}

fn context(retry_after: Option<RetryAfter>) -> ProviderErrorContext {
    ProviderErrorContext {
        retry_after,
        code: crate::boundary::ProviderName::new("retryable".into()).unwrap(),
        context: text("temporary"),
    }
}

fn success() -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::TextDelta { text: text("done") },
        ProviderEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]
}

async fn run_with(
    provider: &ScriptedProvider,
    time: &RecordedTime,
    request: crate::ports::ProviderRequest,
) -> (Result<TurnRunOutcome, RuntimeError>, RecordingEffects) {
    run_with_policy(provider, time, request, RetryPolicy::default()).await
}

async fn run_with_policy(
    provider: &ScriptedProvider,
    time: &RecordedTime,
    request: crate::ports::ProviderRequest,
    policy: RetryPolicy,
) -> (Result<TurnRunOutcome, RuntimeError>, RecordingEffects) {
    let events = RecordedEvents(time);
    let executor = RetryExecutor::new(time, time, &events, policy, None);
    let tools = SequencingTool::new(Vec::new());
    let effects = RecordingEffects::default();
    let (mut runtime, handle, lease) = runtime();
    let catalog = catalog(&[]);
    let ports = TurnPorts {
        provider: TurnProvider::Retrying {
            route: ProviderRoute::new("native", "openai"),
            source: provider,
            fallbacks: Vec::new(),
            executor: std::sync::Arc::new(executor),
        },
        provider_start: None,
        tools: &tools,
        controller_tools: None,
        approvals: None,
        catalog: &catalog,
        effects: &effects,
        compaction: None,
        request_refresh: None,
    };
    (
        run_turn(&mut runtime, handle, lease, request, ports).await,
        effects,
    )
}

#[tokio::test]
async fn busy_uses_exact_task48_backoff_and_records_before_each_delay() {
    let provider = ScriptedProvider::new(vec![
        error(ProviderError::Overloaded(context(None))),
        error(ProviderError::Overloaded(context(None))),
        error(ProviderError::Overloaded(context(None))),
        success(),
    ]);
    let time = RecordedTime::default();
    let (outcome, effects) = run_with(&provider, &time, request()).await;
    assert_eq!(outcome.unwrap(), TurnRunOutcome::Completed);
    assert_eq!(time.delays(), [1_000, 2_000, 4_000]);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        *time.order.lock().unwrap(),
        [
            "retry:1:1000",
            "delay:1000",
            "retry:2:2000",
            "delay:2000",
            "retry:3:4000",
            "delay:4000"
        ]
    );
    let _ = effects;
}

#[tokio::test]
async fn empty_retries_exactly_twice_then_persists_empty_response() {
    let provider = ScriptedProvider::new(vec![Vec::new(), Vec::new(), Vec::new()]);
    let time = RecordedTime::default();
    let (outcome, effects) = run_with(&provider, &time, request()).await;
    assert_eq!(outcome.unwrap(), TurnRunOutcome::Completed);
    assert_eq!(time.delays(), [500, 1_000]);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        effects.stops.lock().unwrap()[0].reason,
        TurnStopReason::EmptyResponse
    );
}

#[tokio::test]
async fn retry_after_is_exact_and_capped_by_policy() {
    let provider = ScriptedProvider::new(vec![
        error(ProviderError::Overloaded(context(Some(
            RetryAfter::Milliseconds(2_345),
        )))),
        error(ProviderError::Overloaded(context(Some(
            RetryAfter::Milliseconds(u64::MAX),
        )))),
        success(),
    ]);
    let time = RecordedTime::default();
    let mut request = request();
    request.deadline = crate::ports::ProviderDeadline::new(Duration::from_secs(90)).unwrap();
    let (outcome, _) = run_with_policy(
        &provider,
        &time,
        request,
        RetryPolicy {
            deadline_ms: 90_000,
            ..RetryPolicy::default()
        },
    )
    .await;
    assert_eq!(outcome.unwrap(), TurnRunOutcome::Completed);
    assert_eq!(
        time.delays(),
        [2_345, crate::retry::PROVIDER_BACKOFF_MS_MAX]
    );
}

#[tokio::test]
async fn cancellation_during_delay_is_user_cancellation_and_never_resends() {
    let provider = ScriptedProvider::new(vec![
        error(ProviderError::Overloaded(context(None))),
        success(),
    ]);
    let time = RecordedTime::cancelling();
    let request = request();
    let (outcome, effects) = run_with(&provider, &time, request).await;
    assert_eq!(outcome.unwrap(), TurnRunOutcome::Completed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(time.delays(), [1_000]);
    assert_eq!(effects.stops.lock().unwrap().len(), 1);
    assert_eq!(
        effects.events.lock().unwrap().last(),
        Some(&TurnEvent::Failed {
            reason: super::TurnStopReason::UserCancellation,
        })
    );
}

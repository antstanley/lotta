use super::*;
use crate::RuntimeError;
use crate::boundary::{ProviderEventText, ProviderName};
use crate::ports::*;
use lotta_domain::{BoundedVec, ModelDescriptor, NonEmptyString};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(super) struct FakeTime {
    monotonic_ms: std::sync::atomic::AtomicU64,
    epoch_ms: std::sync::atomic::AtomicU64,
    delays: Mutex<Vec<u64>>,
}

impl FakeTime {
    pub(super) fn delays(&self) -> Vec<u64> {
        self.delays.lock().unwrap().clone()
    }
}

impl super::Clock for FakeTime {
    fn monotonic_ms(&self) -> u64 {
        self.monotonic_ms.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn unix_epoch_ms(&self) -> u64 {
        self.epoch_ms.load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn cancelled() -> RuntimeError {
    RuntimeError::Cancelled {
        context: "retry test".into(),
    }
}

impl Sleeper for FakeTime {
    fn sleep<'a>(
        &'a self,
        duration: Duration,
        cancellation: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(cancelled());
            }
            let delay_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
            self.delays.lock().unwrap().push(delay_ms);
            self.monotonic_ms
                .fetch_add(delay_ms, std::sync::atomic::Ordering::SeqCst);
            self.epoch_ms
                .fetch_add(delay_ms, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
    }
}

#[derive(Default)]
pub(super) struct Events {
    pub(super) values: Mutex<Vec<RetryEvent>>,
    pub(super) fail: bool,
}

impl EventSink for Events {
    fn emit(&self, event: RetryEvent) -> PortFuture<'_, ()> {
        Box::pin(async move {
            if self.fail {
                return Err(RuntimeError::AdapterFailure {
                    code: "retry_event",
                    context: "retry event sink".into(),
                });
            }
            self.values.lock().unwrap().push(event);
            Ok(())
        })
    }
}

pub(super) enum ScriptedAttempt {
    Events(Vec<ProviderEvent>),
    Error(RuntimeError),
}

impl From<Vec<ProviderEvent>> for ScriptedAttempt {
    fn from(value: Vec<ProviderEvent>) -> Self {
        Self::Events(value)
    }
}

pub(super) struct ScriptedPort {
    scripts: Mutex<VecDeque<ScriptedAttempt>>,
    models: Mutex<Vec<String>>,
    deadlines: Mutex<Vec<u64>>,
    calls: std::sync::atomic::AtomicUsize,
}

impl ScriptedPort {
    pub(super) fn new(scripts: Vec<Vec<ProviderEvent>>) -> Self {
        Self::attempts(scripts.into_iter().map(Into::into).collect())
    }

    pub(super) fn attempts(scripts: Vec<ScriptedAttempt>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            models: Mutex::new(Vec::new()),
            deadlines: Mutex::new(Vec::new()),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub(super) fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) fn models(&self) -> Vec<String> {
        self.models.lock().unwrap().clone()
    }

    pub(super) fn deadlines(&self) -> Vec<u64> {
        self.deadlines.lock().unwrap().clone()
    }
}

impl ProviderPort for ScriptedPort {
    fn stream(&self, request: ProviderRequest, sink: ProviderEventSink) -> PortFuture<'_, ()> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.models
            .lock()
            .unwrap()
            .push(request.model.handle.as_str().to_owned());
        self.deadlines
            .lock()
            .unwrap()
            .push(u64::try_from(request.deadline.get().as_millis()).unwrap());
        let script = self.scripts.lock().unwrap().pop_front();
        Box::pin(async move {
            match script.ok_or_else(|| RuntimeError::InvalidData {
                context: "retry test script".into(),
            })? {
                ScriptedAttempt::Events(events) => {
                    for event in events {
                        sink.send(event).await?;
                    }
                    Ok(())
                }
                ScriptedAttempt::Error(error) => Err(error),
            }
        })
    }
}

pub(super) fn request(deadline_ms: u64) -> ProviderRequest {
    ProviderRequest {
        model: ModelDescriptor {
            handle: NonEmptyString::new("source-model").unwrap(),
            provider_id: NonEmptyString::new("source-provider").unwrap(),
            available: true,
            context_window: Some(4_096),
            model_settings: None,
        },
        system_prompt: None,
        messages: BoundedVec::new(Vec::new()).unwrap(),
        tools: BoundedVec::new(Vec::new()).unwrap(),
        tool_choice: ProviderToolChoice::Auto,
        image_policy: ImagePolicy::Strict,
        context_tokens_max: TokenLimit::new(1_024).unwrap(),
        output_tokens_max: TokenLimit::new(1_024).unwrap(),
        reasoning: ReasoningControls {
            enabled: false,
            effort: None,
            tier: None,
        },
        cancellation: CancellationToken::new(),
        deadline: ProviderDeadline::new(Duration::from_millis(deadline_ms)).unwrap(),
    }
}

pub(super) fn context() -> ProviderErrorContext {
    ProviderErrorContext {
        retry_after: None,
        code: ProviderName::new("retryable".into()).unwrap(),
        context: ProviderEventText::new("temporary".into()).unwrap(),
    }
}

pub(super) fn failure(kind: ProviderFailureKind) -> ProviderEvent {
    let error = match kind {
        ProviderFailureKind::Transient => ProviderError::Unavailable(context()),
        ProviderFailureKind::Busy => ProviderError::Overloaded(context()),
        ProviderFailureKind::Auth => ProviderError::Authentication(context()),
        ProviderFailureKind::Invalid => ProviderError::InvalidRequest(context()),
        ProviderFailureKind::Unsupported => ProviderError::Unknown(context()),
        ProviderFailureKind::Schema => ProviderError::Protocol(context()),
        ProviderFailureKind::Empty | ProviderFailureKind::Terminal => {
            ProviderError::Unknown(context())
        }
    };
    ProviderEvent::Error { error }
}

pub(super) fn stop() -> ProviderEvent {
    ProviderEvent::Stop {
        reason: StopReason::EndTurn,
    }
}

pub(super) async fn run(
    policy: RetryPolicy,
    time: &FakeTime,
    events: &Events,
    port: &ScriptedPort,
) -> Result<(RetryTerminal, Vec<ProviderEvent>), RuntimeError> {
    let req = request(PROVIDER_RETRY_DEADLINE_MS_DEFAULT);
    let (sink, mut receiver) = provider_event_channel(8, &req.cancellation)?;
    let executor = RetryExecutor::new(time, time, events, policy, None);
    let route = ProviderRoute::new("native", "openai");
    let terminal = executor.execute(route, port, None, req, sink).await?;
    let mut output = Vec::new();
    while let Some(event) = receiver.receive().await? {
        output.push(event);
    }
    Ok((terminal, output))
}

use super::fallback::{EventSink, FallbackRoute, ProviderRoute, RetryEvent, RetryReason};
use super::policy::{Clock, ProviderFailure, RetryPolicy, Sleeper};
use crate::RuntimeError;
use crate::ports::{ProviderEvent, ProviderPort, ProviderRequest, provider_event_channel};
use std::collections::VecDeque;
use std::time::Duration;

/// Bounded internal event channel capacity for a provider attempt.
pub const RETRY_EVENT_CHANNEL_CAPACITY: usize = 16;
/// Maximum pre-content non-visible events retained by one attempt.
pub const PENDING_NON_VISIBLE_ITEMS_MAX: usize = 16;
/// Maximum estimated bytes retained by pre-content non-visible events.
pub const PENDING_NON_VISIBLE_BYTES_MAX: usize = 16_384;

/// Sole terminal result of centrally executing provider attempts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RetryTerminal {
    /// Provider stream completed successfully.
    Success,
    /// Retry policy ended with one normalized provider failure and exact send count.
    Failure {
        /// Last normalized failure.
        failure: ProviderFailure,
        /// Total provider sends performed across the execution.
        attempt_count: u32,
    },
}

/// Runtime-owned provider attempt orchestrator.
pub struct RetryExecutor<'a, C: ?Sized, S: ?Sized, E: ?Sized> {
    clock: &'a C,
    sleeper: &'a S,
    events: &'a E,
    policy: RetryPolicy,
    fallback: Option<&'a FallbackRoute>,
}

impl<'a, C, S, E> RetryExecutor<'a, C, S, E>
where
    C: Clock + ?Sized,
    S: Sleeper + ?Sized,
    E: EventSink + ?Sized,
{
    /// Creates an executor with immutable policy and optional one-way fallback.
    #[must_use]
    pub const fn new(
        clock: &'a C,
        sleeper: &'a S,
        events: &'a E,
        policy: RetryPolicy,
        fallback: Option<&'a FallbackRoute>,
    ) -> Self {
        Self {
            clock,
            sleeper,
            events,
            policy,
            fallback,
        }
    }

    /// Executes ordered fallback candidates under one bounded central policy.
    ///
    /// # Errors
    /// Returns typed cancellation, deadline, provider adapter, or event delivery failures.
    pub async fn execute_candidates(
        &self,
        source: ProviderRoute,
        source_port: &dyn ProviderPort,
        fallback_ports: &[&dyn ProviderPort],
        request: ProviderRequest,
        output: crate::ports::ProviderEventSink,
    ) -> Result<RetryTerminal, RuntimeError> {
        if self.fallback.is_some() && fallback_ports.len() > 1 {
            return Err(RuntimeError::InvalidData {
                context: "single fallback route with multiple providers".into(),
            });
        }
        self.execute(
            source,
            source_port,
            fallback_ports.as_ref().split_first().map(|(port, _)| *port),
            request,
            output,
        )
        .await
    }

    /// Executes single-attempt provider adapters under one central budget and deadline.
    ///
    /// Events are forwarded as they arrive. Once model-visible output has escaped, any attempt
    /// failure is terminal and neither retry nor fallback is permitted.
    ///
    /// # Errors
    /// Returns cancellation, deadline, adapter, event-sink, or downstream channel failures.
    pub async fn execute(
        &self,
        source: ProviderRoute,
        source_port: &dyn ProviderPort,
        fallback_port: Option<&dyn ProviderPort>,
        request: ProviderRequest,
        output: crate::ports::ProviderEventSink,
    ) -> Result<RetryTerminal, RuntimeError> {
        let started_ms = self.clock.monotonic_ms();
        let budget_ms = self
            .policy
            .deadline_ms
            .min(duration_ms(request.deadline.get()));
        let deadline_ms = started_ms.saturating_add(budget_ms);
        if self.fallback.is_some_and(|route| route.source() != &source) {
            return Err(RuntimeError::InvalidData {
                context: "provider fallback source route".into(),
            });
        }
        let mut state = ExecutionState::new(source, source_port);
        let mut retry_attempt = 0;
        let mut empty_attempt = 0;
        let mut attempt_count = 0_u32;
        loop {
            attempt_count = attempt_count.saturating_add(1);
            let attempt = self
                .execute_attempt(&state, &request, output.clone(), deadline_ms)
                .await?;
            match attempt.terminal {
                AttemptTerminal::Success if attempt.model_output => {
                    return Ok(RetryTerminal::Success);
                }
                AttemptTerminal::Success => {
                    let failure = empty_failure();
                    if empty_attempt == retries_max(&self.policy, &failure) {
                        return Ok(RetryTerminal::Failure {
                            failure,
                            attempt_count,
                        });
                    }
                    empty_attempt += 1;
                    self.prepare_retry(
                        &mut state,
                        &request,
                        fallback_port,
                        deadline_ms,
                        empty_attempt,
                        &failure,
                    )
                    .await?;
                }
                AttemptTerminal::Failure(failure) => {
                    if attempt.model_output
                        || retry_attempt == retries_max(&self.policy, &failure)
                        || !failure.is_retryable()
                    {
                        forward_terminal(attempt.stop, &output).await?;
                        return Ok(RetryTerminal::Failure {
                            failure,
                            attempt_count,
                        });
                    }
                    let next_attempt = retry_attempt + 1;
                    self.prepare_retry(
                        &mut state,
                        &request,
                        fallback_port,
                        deadline_ms,
                        next_attempt,
                        &failure,
                    )
                    .await?;
                    retry_attempt = next_attempt;
                }
            }
        }
    }

    async fn execute_attempt(
        &self,
        state: &ExecutionState<'_>,
        request: &ProviderRequest,
        output: crate::ports::ProviderEventSink,
        deadline_ms: u64,
    ) -> Result<AttemptOutcome, RuntimeError> {
        self.check_ready(request, deadline_ms)?;
        let remaining_ms = deadline_ms.saturating_sub(self.clock.monotonic_ms());
        let mut attempt_request = request.clone();
        attempt_request.deadline =
            crate::ports::ProviderDeadline::new(Duration::from_millis(remaining_ms))?;
        if let Some(model) = &state.model {
            attempt_request.model = model.clone();
        }
        run_attempt(state.port, attempt_request, output, deadline_ms, self.clock).await
    }

    async fn prepare_retry(
        &self,
        state: &mut ExecutionState<'a>,
        request: &ProviderRequest,
        fallback_port: Option<&'a dyn ProviderPort>,
        deadline_ms: u64,
        next_attempt: u32,
        failure: &ProviderFailure,
    ) -> Result<(), RuntimeError> {
        let plan = self.plan_retry(state, failure, next_attempt)?;
        let now_ms = self.clock.monotonic_ms();
        if now_ms >= deadline_ms || plan.delay_ms >= deadline_ms.saturating_sub(now_ms) {
            return Err(deadline());
        }
        self.emit(state, &plan.destination, failure, next_attempt, &plan)
            .await?;
        if plan.use_fallback {
            let port = fallback_port.ok_or_else(|| RuntimeError::Unsupported {
                context: "provider fallback adapter".into(),
            })?;
            state.route = plan.destination;
            state.model = self.fallback.map(|route| route.destination_model().clone());
            state.port = port;
            state.used_fallback = true;
        }
        self.wait(request, deadline_ms, plan.delay_ms).await
    }

    fn plan_retry(
        &self,
        state: &ExecutionState<'_>,
        failure: &ProviderFailure,
        retry_attempt: u32,
    ) -> Result<RetryPlan, RuntimeError> {
        if !state.used_fallback
            && self.fallback.is_some_and(|route| {
                route.source() == &state.route && FallbackRoute::eligible(failure, retry_attempt)
            })
        {
            let route = self.fallback.ok_or_else(|| RuntimeError::InvalidData {
                context: "provider fallback route".into(),
            })?;
            return Ok(RetryPlan {
                destination: route.destination().clone(),
                delay_ms: 0,
                reason: RetryReason::TransportFallback,
                use_fallback: true,
            });
        }
        let delay_ms = self
            .policy
            .delay_ms(failure, retry_attempt, self.clock.unix_epoch_ms())
            .ok_or_else(|| RuntimeError::InvalidData {
                context: "provider retry delay".into(),
            })?;
        Ok(RetryPlan {
            destination: state.route.clone(),
            delay_ms,
            reason: RetryReason::ProviderRetry,
            use_fallback: false,
        })
    }

    async fn emit(
        &self,
        state: &ExecutionState<'_>,
        destination: &ProviderRoute,
        failure: &ProviderFailure,
        retry_attempt: u32,
        plan: &RetryPlan,
    ) -> Result<(), RuntimeError> {
        self.events
            .emit(RetryEvent::new(
                state.route.clone(),
                destination.clone(),
                plan.reason,
                retry_attempt,
                plan.delay_ms,
                &failure.reason,
            ))
            .await
    }

    async fn wait(
        &self,
        request: &ProviderRequest,
        deadline_ms: u64,
        delay_ms: u64,
    ) -> Result<(), RuntimeError> {
        if delay_ms > 0 {
            self.sleeper
                .sleep(Duration::from_millis(delay_ms), &request.cancellation)
                .await?;
        }
        self.check_ready(request, deadline_ms)
    }

    fn check_ready(&self, request: &ProviderRequest, deadline_ms: u64) -> Result<(), RuntimeError> {
        if request.cancellation.is_cancelled() {
            return Err(RuntimeError::Cancelled {
                context: "provider retry".into(),
            });
        }
        if self.clock.monotonic_ms() >= deadline_ms {
            return Err(deadline());
        }
        Ok(())
    }
}

struct ExecutionState<'a> {
    route: ProviderRoute,
    port: &'a dyn ProviderPort,
    used_fallback: bool,
    model: Option<lotta_domain::ModelDescriptor>,
}

impl<'a> ExecutionState<'a> {
    fn new(route: ProviderRoute, port: &'a dyn ProviderPort) -> Self {
        Self {
            route,
            port,
            used_fallback: false,
            model: None,
        }
    }
}

struct RetryPlan {
    destination: ProviderRoute,
    delay_ms: u64,
    reason: RetryReason,
    use_fallback: bool,
}

enum AttemptTerminal {
    Success,
    Failure(ProviderFailure),
}

struct AttemptOutcome {
    terminal: AttemptTerminal,
    model_output: bool,
    stop: Option<ProviderEvent>,
}

async fn run_attempt<C: Clock + ?Sized>(
    port: &dyn ProviderPort,
    request: ProviderRequest,
    output: crate::ports::ProviderEventSink,
    deadline_ms: u64,
    clock: &C,
) -> Result<AttemptOutcome, RuntimeError> {
    let cancellation = request.cancellation.clone();
    let channel = provider_event_channel(RETRY_EVENT_CHANNEL_CAPACITY, &cancellation)?;
    let (sink, mut receiver) = channel;
    let future = port.stream(request, sink);
    tokio::pin!(future);
    let mut state = AttemptState::default();
    while !state.complete() {
        let remaining_ms = deadline_ms.saturating_sub(clock.monotonic_ms());
        if remaining_ms == 0 {
            return Err(deadline());
        }
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(cancelled_attempt()),
            () = tokio::time::sleep(Duration::from_millis(remaining_ms)) => {
                return Err(deadline());
            }
            result = &mut future, if !state.completion.port_done => match result {
                Err(error @ RuntimeError::Cancelled { .. }) => return Err(error),
                Err(error) => return Ok(state.failure(runtime_failure(&error))),
                Ok(()) => state.completion.port_done = true,
            },
            result = receiver.receive(), if !state.completion.channel_done => match result? {
                Some(event) => {
                    if let Some(outcome) = state.handle(event, &output).await? {
                        return Ok(outcome);
                    }
                }
                None => state.completion.channel_done = true,
            }
        }
    }
    state.finish(&output).await
}

#[derive(Clone, Copy, Default)]
enum AttemptPhase {
    #[default]
    BeforeContent,
    Streaming,
}

#[derive(Default)]
struct AttemptCompletion {
    port_done: bool,
    channel_done: bool,
}

#[derive(Default)]
struct AttemptState {
    pending: VecDeque<ProviderEvent>,
    pending_bytes: usize,
    phase: AttemptPhase,
    held_stop: Option<ProviderEvent>,
    completion: AttemptCompletion,
}

impl AttemptState {
    const fn complete(&self) -> bool {
        self.completion.port_done && self.completion.channel_done
    }

    async fn handle(
        &mut self,
        event: ProviderEvent,
        output: &crate::ports::ProviderEventSink,
    ) -> Result<Option<AttemptOutcome>, RuntimeError> {
        if let Some(stop) = self.held_stop.take() {
            output.send(stop).await?;
            output.send(event).await?;
            self.phase = AttemptPhase::Streaming;
            return Ok(Some(self.failure(schema_failure())));
        }
        if let ProviderEvent::Error { error } = event {
            return Ok(Some(self.failure(error.into())));
        }
        if matches!(event, ProviderEvent::Stop { .. })
            && matches!(self.phase, AttemptPhase::BeforeContent)
        {
            self.held_stop = Some(event);
            return Ok(None);
        }
        if is_usable_content(&event) {
            self.flush_pending(output).await?;
            self.phase = AttemptPhase::Streaming;
            output.send(event).await?;
        } else if matches!(self.phase, AttemptPhase::Streaming) {
            output.send(event).await?;
        } else {
            self.push_pending(event)?;
        }
        Ok(None)
    }

    fn push_pending(&mut self, event: ProviderEvent) -> Result<(), RuntimeError> {
        let bytes = pending_event_bytes(&event)?;
        let next_items = self
            .pending
            .len()
            .checked_add(1)
            .ok_or_else(pending_limit)?;
        let next_bytes = self
            .pending_bytes
            .checked_add(bytes)
            .ok_or_else(pending_limit)?;
        if next_items > PENDING_NON_VISIBLE_ITEMS_MAX || next_bytes > PENDING_NON_VISIBLE_BYTES_MAX
        {
            return Err(pending_limit());
        }
        self.pending.try_reserve(1).map_err(|_| pending_limit())?;
        self.pending.push_back(event);
        self.pending_bytes = next_bytes;
        Ok(())
    }

    async fn flush_pending(
        &mut self,
        output: &crate::ports::ProviderEventSink,
    ) -> Result<(), RuntimeError> {
        while let Some(event) = self.pending.pop_front() {
            output.send(event).await?;
        }
        self.pending_bytes = 0;
        Ok(())
    }

    fn failure(&self, failure: ProviderFailure) -> AttemptOutcome {
        AttemptOutcome {
            terminal: AttemptTerminal::Failure(failure),
            model_output: matches!(self.phase, AttemptPhase::Streaming),
            stop: self.held_stop.clone(),
        }
    }

    async fn finish(
        mut self,
        output: &crate::ports::ProviderEventSink,
    ) -> Result<AttemptOutcome, RuntimeError> {
        if matches!(self.phase, AttemptPhase::Streaming) {
            if let Some(stop) = self.held_stop.take() {
                output.send(stop).await?;
            }
            return Ok(AttemptOutcome {
                terminal: AttemptTerminal::Success,
                model_output: true,
                stop: None,
            });
        }
        Ok(self.failure(empty_failure()))
    }
}

fn pending_event_bytes(event: &ProviderEvent) -> Result<usize, RuntimeError> {
    match event {
        ProviderEvent::Usage { .. } => Ok(std::mem::size_of_val(event)),
        ProviderEvent::ProviderMetadata { metadata } => {
            let debug = format!("{metadata:?}");
            Ok(debug.len())
        }
        _ => Err(pending_limit()),
    }
}

fn pending_limit() -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: "provider_pending_non_visible_event_limit".into(),
    }
}

fn empty_failure() -> ProviderFailure {
    ProviderFailure::new(
        super::policy::ProviderFailureKind::Empty,
        "provider empty response",
    )
}

fn schema_failure() -> ProviderFailure {
    ProviderFailure::new(
        super::policy::ProviderFailureKind::Schema,
        "provider event after held terminal",
    )
}

fn cancelled_attempt() -> RuntimeError {
    RuntimeError::Cancelled {
        context: "provider retry attempt".into(),
    }
}

fn retries_max(policy: &RetryPolicy, failure: &ProviderFailure) -> u32 {
    match failure.kind {
        super::policy::ProviderFailureKind::Empty => super::policy::EMPTY_RESPONSE_RETRIES_MAX,
        _ => policy.retries_max,
    }
}

fn is_usable_content(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::TextDelta { .. }
            | ProviderEvent::ReasoningDelta { .. }
            | ProviderEvent::RedactedReasoning { .. }
            | ProviderEvent::ToolCallStart { .. }
    )
}

async fn forward_terminal(
    stop: Option<ProviderEvent>,
    output: &crate::ports::ProviderEventSink,
) -> Result<(), RuntimeError> {
    if let Some(stop) = stop {
        output.send(stop).await?;
    }
    Ok(())
}

fn runtime_failure(error: &RuntimeError) -> ProviderFailure {
    use super::policy::ProviderFailureKind;
    let kind = match error {
        RuntimeError::Unsupported { .. } => ProviderFailureKind::Unsupported,
        RuntimeError::InvalidData { .. } => ProviderFailureKind::Schema,
        RuntimeError::ContextOverflow { .. } => ProviderFailureKind::ContextOverflow,
        RuntimeError::Timeout { .. } | RuntimeError::AdapterFailure { .. } => {
            ProviderFailureKind::Transient
        }
        _ => ProviderFailureKind::Terminal,
    };
    let failure = ProviderFailure::new(kind, error.to_string());
    match error {
        RuntimeError::ContextOverflow { detail } => failure.with_context_overflow(detail.clone()),
        _ => failure,
    }
}

fn duration_ms(value: Duration) -> u64 {
    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
}

fn deadline() -> RuntimeError {
    RuntimeError::Timeout {
        context: "provider retry deadline".into(),
    }
}

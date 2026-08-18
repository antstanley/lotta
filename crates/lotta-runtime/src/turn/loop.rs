use super::step::tool_message;
use super::{ProjectionKind, ToolResultRecord, TurnEffectPort, TurnEvent, TurnToolCatalog};
use crate::bounds::{PROVIDER_MESSAGES_MAX, TURN_STEPS_MAX, TURN_TOOL_CALLS_MAX};
use crate::ports::{
    ParallelSafety, ProviderEvent, ProviderMessage, ProviderMessages, ProviderPort,
    ProviderRequest, StopReason, ToolApprovalPolicy, ToolCallAccumulator, ToolCallId,
    ToolExecutionOwner, ToolExecutionRequest, ToolPort, ValidatedToolInput, provider_event_channel,
};
use crate::retry::{
    Clock, EventSink, FallbackRoute, ProviderRoute, RETRY_EVENT_CHANNEL_CAPACITY, RetryEvent,
    RetryExecutor, RetryPolicy, RetryTerminal, Sleeper,
};
use crate::{
    CancellationPolicy, LeaseEffect, LeaseGuard, ListenerRuntime, RuntimeError, RuntimeHandle,
};
use lotta_domain::TurnLease;
use std::collections::BTreeMap;

/// Outcome of an admitted turn loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TurnRunOutcome {
    /// Terminal effect was emitted and the lifecycle released.
    Completed,
    /// A stale lease or cancellation suppressed all later effects.
    Suppressed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Flow {
    Continue,
    Suppressed,
}

struct ProviderStepState {
    accumulator: ToolCallAccumulator,
    names: BTreeMap<ToolCallId, crate::boundary::ProviderEventText>,
    completed: Vec<ToolResultRecord>,
    stop: Option<StopReason>,
    terminal: bool,
}

impl ProviderStepState {
    fn new(remaining: usize) -> Result<Self, RuntimeError> {
        let mut completed = Vec::new();
        completed
            .try_reserve(remaining)
            .map_err(|_| limit(TURN_TOOL_CALLS_MAX.name))?;
        Ok(Self {
            accumulator: ToolCallAccumulator::default(),
            names: BTreeMap::new(),
            completed,
            stop: None,
            terminal: false,
        })
    }
}

/// Borrowed ports used by one turn run.
pub struct TurnPorts<'ports, 'catalog> {
    /// Runtime retry composition over the normalized provider boundary.
    pub provider: TurnProvider<'ports>,
    /// Optional callbacks fired around the provider's first poll.
    pub provider_start: Option<&'catalog dyn ProviderStartPort>,
    /// Local tool execution boundary.
    pub tools: &'ports dyn ToolPort,
    /// Tools admitted for this turn.
    pub catalog: &'catalog TurnToolCatalog,
    /// Owner-local projection and event effects.
    pub effects: &'ports dyn TurnEffectPort,
}

/// Provider lifecycle callbacks straddling the stream future's first poll.
pub trait ProviderStartPort: Send + Sync {
    /// Runs immediately before the provider stream future is first polled.
    ///
    /// # Errors
    /// Returns a typed failure that occurs after durable input admission.
    fn provider_start(&self) -> crate::ports::PortFuture<'_, ()>;
    /// Runs after the first poll only when the provider remains pending.
    ///
    /// # Errors
    /// Returns a typed failure that occurs after durable input admission.
    fn provider_waiting(&self) -> crate::ports::PortFuture<'_, ()>;
}

impl<'ports, 'catalog> TurnPorts<'ports, 'catalog> {
    /// Groups production ports with an explicitly unconfigured fallback.
    #[must_use]
    pub fn new(
        provider: &'ports dyn ProviderPort,
        tools: &'ports dyn ToolPort,
        catalog: &'catalog TurnToolCatalog,
        effects: &'ports dyn TurnEffectPort,
    ) -> Self {
        Self::configured(
            ProviderRoute::new("native", "configured"),
            provider,
            None,
            tools,
            catalog,
            effects,
        )
    }

    /// Builds production retry composition from an immutable validated fallback option.
    #[must_use]
    pub fn configured(
        route: ProviderRoute,
        provider: &'ports dyn ProviderPort,
        fallback: Option<ConfiguredFallback<'ports>>,
        tools: &'ports dyn ToolPort,
        catalog: &'catalog TurnToolCatalog,
        effects: &'ports dyn TurnEffectPort,
    ) -> Self {
        let fallback_port = fallback.as_ref().map(|value| value.provider);
        let fallback_route = fallback.map(|value| value.route);
        Self {
            provider: TurnProvider::Retrying {
                route,
                source: provider,
                fallback: fallback_port,
                executor: std::sync::Arc::new(ProductionRetryExecutor {
                    clock: crate::retry::SystemClock::default(),
                    sleeper: crate::retry::TokioSleeper,
                    events: TurnRetryEventSink::new(),
                    fallback: fallback_route,
                }),
            },
            provider_start: None,
            tools,
            catalog,
            effects,
        }
    }

    /// Installs a callback for the first provider stream admission boundary.
    #[must_use]
    pub fn with_provider_start(mut self, callback: &'catalog dyn ProviderStartPort) -> Self {
        self.provider_start = Some(callback);
        self
    }

    /// Explicit single-attempt compatibility composition for tests and legacy callers.
    #[must_use]
    pub const fn direct(
        provider: &'ports dyn ProviderPort,
        tools: &'ports dyn ToolPort,
        catalog: &'catalog TurnToolCatalog,
        effects: &'ports dyn TurnEffectPort,
    ) -> Self {
        Self {
            provider: TurnProvider::Direct(provider),
            provider_start: None,
            tools,
            catalog,
            effects,
        }
    }
}

/// Typed immutable fallback composition supplied by runtime/model configuration.
pub struct ConfiguredFallback<'a> {
    /// Validated one-way route and destination request model.
    pub route: FallbackRoute,
    /// Destination single-attempt provider adapter.
    pub provider: &'a dyn ProviderPort,
}

/// Task19 provider execution boundary, either direct or centrally retried.
#[derive(Clone)]
pub enum TurnProvider<'a> {
    /// Direct single-attempt port retained for compatibility callers.
    Direct(&'a dyn ProviderPort),
    /// Production central retry composition.
    Retrying {
        /// Initial immutable route.
        route: ProviderRoute,
        /// Initial single-attempt adapter.
        source: &'a dyn ProviderPort,
        /// Optional destination single-attempt adapter.
        fallback: Option<&'a dyn ProviderPort>,
        /// Runtime retry executor.
        executor: std::sync::Arc<dyn ProviderTurnExecutorPort + 'a>,
    },
}

/// Object-safe execution boundary implemented by a configured [`RetryExecutor`].
pub trait ProviderTurnExecutorPort: Send + Sync {
    /// Creates the retry-event channel for one provider step, when supported.
    ///
    /// # Errors
    /// Returns a typed channel installation error.
    fn begin_retry_step(
        &self,
    ) -> Result<Option<tokio::sync::mpsc::Receiver<RetryEvent>>, RuntimeError> {
        Ok(None)
    }

    /// Closes this provider step's retry-event sender.
    ///
    /// # Errors
    /// Returns a typed channel closure error.
    fn end_retry_step(&self) -> Result<(), RuntimeError> {
        Ok(())
    }

    /// Executes attempts and emits one successful stream or one terminal failure.
    fn execute<'a>(
        &'a self,
        route: ProviderRoute,
        source: &'a dyn ProviderPort,
        fallback: Option<&'a dyn ProviderPort>,
        request: ProviderRequest,
        output: crate::ports::ProviderEventSink,
    ) -> crate::ports::PortFuture<'a, RetryTerminal>;
}

impl<C, S, E> ProviderTurnExecutorPort for RetryExecutor<'_, C, S, E>
where
    C: Clock + ?Sized,
    S: Sleeper + ?Sized,
    E: EventSink + ?Sized,
{
    fn execute<'a>(
        &'a self,
        route: ProviderRoute,
        source: &'a dyn ProviderPort,
        fallback: Option<&'a dyn ProviderPort>,
        request: ProviderRequest,
        output: crate::ports::ProviderEventSink,
    ) -> crate::ports::PortFuture<'a, RetryTerminal> {
        Box::pin(self.execute(route, source, fallback, request, output))
    }
}

struct TurnRetryEventSink {
    sender: std::sync::Mutex<Option<tokio::sync::mpsc::Sender<RetryEvent>>>,
}

impl TurnRetryEventSink {
    const fn new() -> Self {
        Self {
            sender: std::sync::Mutex::new(None),
        }
    }

    fn begin_step(&self) -> Result<tokio::sync::mpsc::Receiver<RetryEvent>, RuntimeError> {
        let (sender, receiver) = tokio::sync::mpsc::channel(RETRY_EVENT_CHANNEL_CAPACITY);
        let mut current = self.sender.lock().map_err(retry_channel_lock)?;
        if current.replace(sender).is_some() {
            return Err(RuntimeError::InvalidData {
                context: "retry event step already active".into(),
            });
        }
        Ok(receiver)
    }

    fn end_step(&self) -> Result<(), RuntimeError> {
        self.sender.lock().map_err(retry_channel_lock)?.take();
        Ok(())
    }
}

fn retry_channel_lock<T>(_: std::sync::PoisonError<T>) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "retry_event_channel",
        context: "retry event sender lock".into(),
    }
}

impl EventSink for TurnRetryEventSink {
    fn emit(&self, event: RetryEvent) -> crate::ports::PortFuture<'_, ()> {
        let sender = self
            .sender
            .lock()
            .map_err(retry_channel_lock)
            .map(|value| value.clone());
        Box::pin(async move {
            let sender = sender?.ok_or_else(|| RuntimeError::InvalidData {
                context: "retry event step is not active".into(),
            })?;
            sender
                .send(event)
                .await
                .map_err(|_| RuntimeError::AdapterFailure {
                    code: "retry_event_channel",
                    context: "retry event receiver closed".into(),
                })
        })
    }
}

struct ProductionRetryExecutor {
    clock: crate::retry::SystemClock,
    sleeper: crate::retry::TokioSleeper,
    events: TurnRetryEventSink,
    fallback: Option<FallbackRoute>,
}

impl ProviderTurnExecutorPort for ProductionRetryExecutor {
    fn begin_retry_step(
        &self,
    ) -> Result<Option<tokio::sync::mpsc::Receiver<RetryEvent>>, RuntimeError> {
        self.events.begin_step().map(Some)
    }

    fn end_retry_step(&self) -> Result<(), RuntimeError> {
        self.events.end_step()
    }

    fn execute<'a>(
        &'a self,
        route: ProviderRoute,
        source: &'a dyn ProviderPort,
        fallback: Option<&'a dyn ProviderPort>,
        request: ProviderRequest,
        output: crate::ports::ProviderEventSink,
    ) -> crate::ports::PortFuture<'a, RetryTerminal> {
        Box::pin(async move {
            RetryExecutor::new(
                &self.clock,
                &self.sleeper,
                &self.events,
                RetryPolicy::default(),
                self.fallback.as_ref(),
            )
            .execute(route, source, fallback, request, output)
            .await
        })
    }
}

struct TurnContext<'ports, 'catalog> {
    runtime: &'ports mut ListenerRuntime,
    guard: LeaseGuard,
    provider: TurnProvider<'ports>,
    provider_start: Option<&'catalog dyn ProviderStartPort>,
    tools: &'ports dyn ToolPort,
    catalog: &'catalog TurnToolCatalog,
    effects: &'ports dyn TurnEffectPort,
    request: ProviderRequest,
    total_tool_calls: usize,
}

/// Runs one bounded provider turn and sequential local-tool continuations.
///
/// # Errors
/// Returns a typed provider, protocol, effect, bound, or tool failure.
pub async fn run_turn(
    runtime: &mut ListenerRuntime,
    handle: RuntimeHandle,
    lease: TurnLease,
    request: ProviderRequest,
    ports: TurnPorts<'_, '_>,
) -> Result<TurnRunOutcome, RuntimeError> {
    let guard = LeaseGuard::new(
        handle,
        lease,
        request.cancellation.clone(),
        CancellationPolicy::SuppressWhenCancelled,
    );
    let TurnPorts {
        provider,
        provider_start,
        tools,
        catalog,
        effects,
    } = ports;
    let mut turn = TurnContext {
        runtime,
        guard,
        provider,
        provider_start,
        tools,
        catalog,
        effects,
        request,
        total_tool_calls: 0,
    };
    run_loop(&mut turn).await
}

async fn run_loop(turn: &mut TurnContext<'_, '_>) -> Result<TurnRunOutcome, RuntimeError> {
    if turn.is_suppressed() {
        turn.request.cancellation.cancel();
        return Ok(TurnRunOutcome::Suppressed);
    }
    for step_index in 0..TURN_STEPS_MAX.value {
        let remaining = TURN_TOOL_CALLS_MAX.value - turn.total_tool_calls;
        let mut state = ProviderStepState::new(remaining)?;
        if run_provider_step(turn, &mut state).await? == Flow::Suppressed {
            turn.request.cancellation.cancel();
            return Ok(TurnRunOutcome::Suppressed);
        }
        if turn.is_suppressed() {
            turn.request.cancellation.cancel();
            return Ok(TurnRunOutcome::Suppressed);
        }
        let reason = state
            .stop
            .ok_or_else(|| protocol("provider stream closed without terminal"))?;
        if reason == StopReason::ToolUse {
            continue_after_tools(turn, &state.completed, step_index)?;
            continue;
        }
        if !state.completed.is_empty() {
            return Err(protocol("provider tool result with non-tool stop"));
        }
        return finish_turn(turn, reason);
    }
    Err(limit(TURN_STEPS_MAX.name))
}

fn continue_after_tools(
    turn: &mut TurnContext<'_, '_>,
    completed: &[ToolResultRecord],
    step_index: usize,
) -> Result<(), RuntimeError> {
    if completed.is_empty() {
        return Err(protocol("provider tool use without completed result"));
    }
    append_messages(&mut turn.request, completed)?;
    if step_index + 1 == TURN_STEPS_MAX.value {
        return Err(limit(TURN_STEPS_MAX.name));
    }
    Ok(())
}

fn finish_turn(
    turn: &mut TurnContext<'_, '_>,
    reason: StopReason,
) -> Result<TurnRunOutcome, RuntimeError> {
    let domain = domain_stop(reason)?;
    match turn
        .guard
        .finish_turn_with_effect_after_await(turn.runtime, domain, || {
            turn.effects.emit(TurnEvent::Finished { reason })
        })? {
        LeaseEffect::Applied(()) => Ok(TurnRunOutcome::Completed),
        LeaseEffect::Suppressed(_) => Ok(TurnRunOutcome::Suppressed),
    }
}

fn execute_provider(
    provider: TurnProvider<'_>,
    request: ProviderRequest,
    sink: crate::ports::ProviderEventSink,
) -> Result<
    (
        crate::ports::PortFuture<'_, RetryTerminal>,
        Option<tokio::sync::mpsc::Receiver<RetryEvent>>,
    ),
    RuntimeError,
> {
    match provider {
        TurnProvider::Direct(port) => Ok((
            Box::pin(async move {
                port.stream(request, sink).await?;
                Ok(RetryTerminal::Success)
            }),
            None,
        )),
        TurnProvider::Retrying {
            route,
            source,
            fallback,
            executor,
        } => {
            let retry_events = executor.begin_retry_step()?;
            Ok((
                Box::pin(async move {
                    executor
                        .execute(route, source, fallback, request, sink)
                        .await
                }),
                retry_events,
            ))
        }
    }
}

async fn run_provider_step(
    turn: &mut TurnContext<'_, '_>,
    state: &mut ProviderStepState,
) -> Result<Flow, RuntimeError> {
    turn.request.validate_bytes()?;
    let (sink, mut output) = provider_event_channel(1, &turn.request.cancellation)?;
    let (future, mut retry_events) =
        execute_provider(turn.provider.clone(), turn.request.clone(), sink)?;
    let callback = turn.provider_start.take();
    if let Some(callback) = callback {
        callback.provider_start().await?;
    }
    tokio::pin!(future);
    let mut first_poll_pending = false;
    let first = std::future::poll_fn(|context| match future.as_mut().poll(context) {
        std::task::Poll::Ready(value) => std::task::Poll::Ready(Some(value)),
        std::task::Poll::Pending => {
            first_poll_pending = true;
            std::task::Poll::Ready(None)
        }
    })
    .await;
    if first_poll_pending && let Some(callback) = callback {
        callback.provider_waiting().await?;
    }
    let terminal = if let Some(result) = first {
        result
    } else {
        loop {
            tokio::select! {
                biased;
                event = receive_retry(&mut retry_events), if retry_events.is_some() => {
                    if let Some(event) = event && apply_retry(turn, event)? == Flow::Suppressed {
                        return suppress_provider(turn, &mut output);
                    }
                }
                result = &mut future => break result,
                result = output.receive() => {
                    if drain_ready_retries(turn, &mut retry_events)? == Flow::Suppressed {
                        return suppress_provider(turn, &mut output);
                    }
                    if turn.is_suppressed() {
                        return suppress_provider(turn, &mut output);
                    }
                    match result? {
                        Some(event) => {
                            if handle_event(turn, state, event).await? == Flow::Suppressed {
                                return suppress_provider(turn, &mut output);
                            }
                        }
                        None => return Err(protocol("provider stream closed before executor")),
                    }
                }
            }
        }
    };
    close_retry_step(&turn.provider)?;
    if turn.is_suppressed() || drain_retries(turn, &mut retry_events).await? == Flow::Suppressed {
        return suppress_provider(turn, &mut output);
    }
    handle_terminal(terminal?, turn, state, &mut output).await
}

fn drain_ready_retries(
    turn: &mut TurnContext<'_, '_>,
    receiver: &mut Option<tokio::sync::mpsc::Receiver<RetryEvent>>,
) -> Result<Flow, RuntimeError> {
    let Some(receiver) = receiver.as_mut() else {
        return Ok(Flow::Continue);
    };
    loop {
        match receiver.try_recv() {
            Ok(event) => {
                if apply_retry(turn, event)? == Flow::Suppressed {
                    return Ok(Flow::Suppressed);
                }
            }
            Err(
                tokio::sync::mpsc::error::TryRecvError::Empty
                | tokio::sync::mpsc::error::TryRecvError::Disconnected,
            ) => {
                return Ok(Flow::Continue);
            }
        }
    }
}

async fn receive_retry(
    receiver: &mut Option<tokio::sync::mpsc::Receiver<RetryEvent>>,
) -> Option<RetryEvent> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => None,
    }
}

fn apply_retry(turn: &mut TurnContext<'_, '_>, event: RetryEvent) -> Result<Flow, RuntimeError> {
    match turn
        .guard
        .apply_after_await(turn.runtime, || turn.effects.emit(TurnEvent::Retry(event)))
    {
        LeaseEffect::Applied(result) => result.map(|()| Flow::Continue),
        LeaseEffect::Suppressed(_) => Ok(Flow::Suppressed),
    }
}

async fn drain_retries(
    turn: &mut TurnContext<'_, '_>,
    receiver: &mut Option<tokio::sync::mpsc::Receiver<RetryEvent>>,
) -> Result<Flow, RuntimeError> {
    while let Some(event) = receive_retry(receiver).await {
        if apply_retry(turn, event)? == Flow::Suppressed {
            return Ok(Flow::Suppressed);
        }
    }
    Ok(Flow::Continue)
}

fn close_retry_step(provider: &TurnProvider<'_>) -> Result<(), RuntimeError> {
    if let TurnProvider::Retrying { executor, .. } = provider {
        executor.end_retry_step()?;
    }
    Ok(())
}

fn suppress_provider(
    turn: &TurnContext<'_, '_>,
    output: &mut crate::ports::ProviderEventReceiver,
) -> Result<Flow, RuntimeError> {
    output.cancel();
    turn.request.cancellation.cancel();
    close_retry_step(&turn.provider)?;
    Ok(Flow::Suppressed)
}

async fn handle_terminal(
    terminal: RetryTerminal,
    turn: &mut TurnContext<'_, '_>,
    state: &mut ProviderStepState,
    output: &mut crate::ports::ProviderEventReceiver,
) -> Result<Flow, RuntimeError> {
    if let RetryTerminal::Failure(failure) = terminal {
        return Err(RuntimeError::AdapterFailure {
            code: "provider_terminal_error",
            context: failure.reason,
        });
    }
    while let Some(event) = output.receive().await? {
        if handle_event(turn, state, event).await? == Flow::Suppressed {
            return suppress_provider(turn, output);
        }
    }
    state.accumulator.terminal_stop()?;
    state.stop.map_or_else(
        || Err(protocol("provider stream closed without terminal")),
        |_| Ok(Flow::Continue),
    )
}

async fn handle_event(
    turn: &mut TurnContext<'_, '_>,
    state: &mut ProviderStepState,
    event: ProviderEvent,
) -> Result<Flow, RuntimeError> {
    if state.terminal {
        state.accumulator.terminal_error();
        return Err(protocol("provider event after terminal"));
    }
    match event {
        ProviderEvent::TextDelta { text } => turn.project(ProjectionKind::Text, text),
        ProviderEvent::ReasoningDelta { text } => turn.project(ProjectionKind::Reasoning, text),
        ProviderEvent::RedactedReasoning { marker } => {
            turn.project(ProjectionKind::RedactedReasoning, marker)
        }
        ProviderEvent::ToolCallStart { call_id, name } => start_call(turn, state, call_id, name),
        ProviderEvent::ToolCallArgumentsDelta { call_id, chunk } => {
            state.accumulator.append(&call_id, chunk.as_slice())?;
            Ok(Flow::Continue)
        }
        ProviderEvent::ToolCallEnd { call_id } => execute_call(turn, state, call_id).await,
        ProviderEvent::Usage { .. } | ProviderEvent::ProviderMetadata { .. } => Ok(Flow::Continue),
        ProviderEvent::Stop { reason } => terminal_stop(state, reason),
        ProviderEvent::Error { .. } => {
            state.accumulator.terminal_error();
            Err(RuntimeError::AdapterFailure {
                code: "provider_terminal_error",
                context: "turn provider stream".into(),
            })
        }
    }
}

impl TurnContext<'_, '_> {
    fn is_suppressed(&self) -> bool {
        matches!(
            self.guard.apply_after_await(self.runtime, || ()),
            LeaseEffect::Suppressed(_)
        )
    }

    fn project(
        &self,
        kind: ProjectionKind,
        value: crate::boundary::ProviderEventText,
    ) -> Result<Flow, RuntimeError> {
        let projection = super::TurnProjection::new(kind, value);
        match self.guard.apply_after_await(self.runtime, || {
            self.effects.persist_projection(projection.clone())?;
            self.effects.emit(TurnEvent::StreamDelta(projection))
        }) {
            LeaseEffect::Applied(result) => result.map(|()| Flow::Continue),
            LeaseEffect::Suppressed(_) => Ok(Flow::Suppressed),
        }
    }
}

fn start_call(
    turn: &mut TurnContext<'_, '_>,
    state: &mut ProviderStepState,
    call_id: ToolCallId,
    name: crate::boundary::ProviderEventText,
) -> Result<Flow, RuntimeError> {
    if turn.total_tool_calls == TURN_TOOL_CALLS_MAX.value {
        return Err(limit(TURN_TOOL_CALLS_MAX.name));
    }
    state.accumulator.start(call_id.clone())?;
    if state.names.insert(call_id, name).is_some() {
        state.accumulator.terminal_error();
        return Err(protocol("duplicate provider tool call name"));
    }
    turn.total_tool_calls += 1;
    Ok(Flow::Continue)
}

async fn execute_call(
    turn: &mut TurnContext<'_, '_>,
    state: &mut ProviderStepState,
    call_id: ToolCallId,
) -> Result<Flow, RuntimeError> {
    let value = state.accumulator.end(&call_id)?;
    let Some(name) = state.names.remove(&call_id) else {
        state.accumulator.terminal_error();
        return Err(protocol("provider tool call name missing"));
    };
    let definition = turn
        .catalog
        .get(name.as_str())
        .ok_or_else(|| protocol("provider tool definition missing"))?;
    require_minimal_tool(definition)?;
    let execution = ToolExecutionRequest {
        definition: definition.clone(),
        input: ValidatedToolInput::new(value)?,
        cancellation: turn.request.cancellation.clone(),
        deadline: definition.timeout,
    };
    let outcome = turn.tools.execute(execution).await;
    if turn.is_suppressed() {
        return Ok(Flow::Suppressed);
    }
    let result = ToolResultRecord {
        call_id,
        outcome: outcome?,
    };
    if state.completed.len() == TURN_TOOL_CALLS_MAX.value {
        return Err(limit(TURN_TOOL_CALLS_MAX.name));
    }
    state
        .completed
        .try_reserve(1)
        .map_err(|_| limit(TURN_TOOL_CALLS_MAX.name))?;
    match turn.guard.apply_after_await(turn.runtime, || {
        turn.effects.append_tool_result(result.clone())?;
        turn.effects.emit(TurnEvent::ToolResult(result.clone()))
    }) {
        LeaseEffect::Applied(effect) => effect?,
        LeaseEffect::Suppressed(_) => return Ok(Flow::Suppressed),
    }
    state.completed.push(result);
    Ok(Flow::Continue)
}

fn require_minimal_tool(definition: &crate::ports::ToolDefinition) -> Result<(), RuntimeError> {
    match definition.execution_owner {
        ToolExecutionOwner::Rust => {}
        _ => return Err(unsupported("turn tool execution owner")),
    }
    match definition.approval_policy {
        ToolApprovalPolicy::Never => {}
        ToolApprovalPolicy::Always => return Err(unsupported("turn tool approval")),
    }
    match definition.parallel_safety {
        ParallelSafety::Sequential => Ok(()),
        ParallelSafety::CertifiedParallel(_) => Err(unsupported("turn parallel tool")),
    }
}

fn terminal_stop(state: &mut ProviderStepState, reason: StopReason) -> Result<Flow, RuntimeError> {
    if state.stop.is_some() {
        state.accumulator.terminal_error();
        return Err(protocol("multiple provider terminals"));
    }
    state.accumulator.terminal_stop()?;
    state.stop = Some(reason);
    state.terminal = true;
    Ok(Flow::Continue)
}

fn append_messages(
    request: &mut ProviderRequest,
    completed: &[ToolResultRecord],
) -> Result<(), RuntimeError> {
    let next = request
        .messages
        .len()
        .checked_add(completed.len())
        .ok_or_else(|| limit(PROVIDER_MESSAGES_MAX.name))?;
    if next > PROVIDER_MESSAGES_MAX.value {
        return Err(limit(PROVIDER_MESSAGES_MAX.name));
    }
    let mut messages: Vec<ProviderMessage> = request.messages.as_slice().to_vec();
    messages
        .try_reserve(completed.len())
        .map_err(|_| limit(PROVIDER_MESSAGES_MAX.name))?;
    for result in completed {
        messages.push(tool_message(result)?);
    }
    request.messages =
        ProviderMessages::new(messages).map_err(|_| limit(PROVIDER_MESSAGES_MAX.name))?;
    request.validate_bytes()
}

fn domain_stop(reason: StopReason) -> Result<lotta_domain::StopReason, RuntimeError> {
    let value = match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::OutputLimit => "output_limit",
        StopReason::ToolUse => "tool_use",
        StopReason::ContentFilter => "content_filter",
        StopReason::Other => "other",
    };
    lotta_domain::StopReason::new(value).map_err(|_| protocol("domain stop reason"))
}

fn protocol(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}

fn limit(context: &'static str) -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: context.into(),
    }
}

fn unsupported(context: &'static str) -> RuntimeError {
    RuntimeError::Unsupported {
        context: context.into(),
    }
}

use super::step::tool_message;
use super::{
    ControlRequest, ControllerToolRequestRecord, ProjectionKind, ProviderFailureDetail,
    ToolResultRecord, TurnEffectPort, TurnEvent, TurnStopReason, TurnStopRecord, TurnToolCatalog,
};
use crate::bounds::{PROVIDER_MESSAGES_MAX, TURN_STEPS_MAX, TURN_TOOL_CALLS_MAX};
use crate::ports::{
    ParallelSafety, ProviderEvent, ProviderMessage, ProviderMessages, ProviderPort,
    ProviderRequest, StopReason, ToolApprovalPolicy, ToolCallAccumulator, ToolCallId,
    ToolExecutionOwner, ToolExecutionRequest, ToolOutcome, ToolPort, ValidatedToolInput,
    provider_event_channel,
};
use crate::retry::{
    Clock, EventSink, FallbackRoute, ProviderRoute, RETRY_EVENT_CHANNEL_CAPACITY, RetryEvent,
    RetryExecutor, RetryPolicy, RetryTerminal, Sleeper,
};
use crate::{
    CancellationPolicy, LeaseEffect, LeaseGuard, ListenerRuntime, RuntimeError, RuntimeHandle,
};
use lotta_domain::{BoundedJsonValue, NonEmptyString, RunId, TurnLease};
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
    Failed,
    Suppressed,
}

/// Maximum context-overflow compactions before the fourth overflow is terminal.
pub const CONTEXT_OVERFLOW_COMPACTIONS_MAX: u8 = 3;

/// Safe before/after size observations returned by the injected Task58 seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompactionProgress {
    /// Estimated model-visible tokens before compaction.
    pub tokens_before: u64,
    /// Estimated model-visible tokens after compaction.
    pub tokens_after: u64,
    /// Model-visible messages before compaction.
    pub messages_before: usize,
    /// Model-visible messages after compaction.
    pub messages_after: usize,
}

impl CompactionProgress {
    fn progressed(self) -> bool {
        self.tokens_after < self.tokens_before || self.messages_after < self.messages_before
    }
}

/// Object-safe Task58 compaction seam. Implementations own persistence and lifecycle emission.
pub trait CompactionPort: Send + Sync {
    /// Compacts once for the exact request, lease, cancellation, and overflow detail.
    fn compact(
        &self,
        request: ProviderRequest,
        lease: TurnLease,
        detail: crate::ports::ProviderContextOverflowDetail,
    ) -> crate::ports::PortFuture<'_, CompactionProgress>;
}

/// Object-safe refresh/recompile/rebuild seam invoked after successful compaction.
pub trait RequestRefreshPort: Send + Sync {
    /// Rebuilds the provider request from current durable state.
    fn refresh(
        &self,
        request: ProviderRequest,
        lease: TurnLease,
    ) -> crate::ports::PortFuture<'static, ProviderRequest>;
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
    /// Local Rust tool execution boundary.
    pub tools: &'ports dyn ToolPort,
    /// External non-Rust execution boundary.
    pub controller_tools: Option<&'ports dyn ControllerToolPort>,
    /// Interactive approval boundary.
    pub approvals: Option<&'ports dyn ApprovalPort>,
    /// Tools admitted for this turn.
    pub catalog: &'catalog TurnToolCatalog,
    /// Owner-local projection and event effects.
    pub effects: &'ports dyn TurnEffectPort,
    /// Optional injected Task58 compaction seam.
    pub compaction: Option<&'ports dyn CompactionPort>,
    /// Optional request refresh/recompile/rebuild seam paired with compaction.
    pub request_refresh: Option<std::sync::Arc<dyn RequestRefreshPort>>,
}

/// Resolution returned by the runtime approval backend.
#[derive(Clone, Debug, PartialEq)]
pub enum ApprovalResolution {
    /// Execute with original or edited input.
    Allow(Option<BoundedJsonValue>),
    /// Append a structured user-denied result.
    Deny,
}

/// Approval backend for one exact persisted request and lease.
pub trait ApprovalPort: Send + Sync {
    /// Durably stores the exact request before any visible effect is emitted.
    ///
    /// # Errors
    /// Returns a journal conflict, capacity, or persistence failure.
    fn store_request(&self, request: ControlRequest) -> Result<(), RuntimeError>;
    /// Waits for a matching resolution, cancellation, or the approval deadline.
    fn await_resolution(
        &self,
        request: ControlRequest,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> crate::ports::PortFuture<'_, ApprovalResolution>;
    /// Finalizes one successfully executed allow claim exactly once.
    ///
    /// # Errors
    /// Returns a durable state conflict or persistence failure.
    fn mark_allowed(
        &self,
        request: &ControlRequest,
        outcome: &ToolOutcome,
    ) -> Result<(), RuntimeError>;
    /// Marks an executing claim interrupted when execution or persistence is uncertain.
    ///
    /// # Errors
    /// Returns a durable state conflict or persistence failure.
    fn mark_interrupted(&self, request: &ControlRequest) -> Result<(), RuntimeError>;
}

/// External execution backend for controller, MCP, sidecar, and channel owners.
pub trait ControllerToolPort: Send + Sync {
    /// Executes exactly one bounded externally-owned call.
    fn execute_external(
        &self,
        record: ControllerToolRequestRecord,
        request: ToolExecutionRequest,
    ) -> crate::ports::PortFuture<'_, ToolOutcome>;
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
            Vec::new(),
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
        fallbacks: Vec<ConfiguredFallback<'ports>>,
        tools: &'ports dyn ToolPort,
        catalog: &'catalog TurnToolCatalog,
        effects: &'ports dyn TurnEffectPort,
    ) -> Self {
        let fallback_ports = fallbacks
            .iter()
            .map(|value| value.provider)
            .collect::<Vec<_>>();
        let fallback_routes = fallbacks
            .into_iter()
            .map(|value| value.route)
            .collect::<Vec<_>>();
        Self {
            provider: TurnProvider::Retrying {
                route,
                source: provider,
                fallbacks: fallback_ports,
                executor: std::sync::Arc::new(ProductionRetryExecutor {
                    clock: crate::retry::SystemClock::default(),
                    sleeper: crate::retry::TokioSleeper,
                    events: TurnRetryEventSink::new(),
                    fallbacks: fallback_routes,
                }),
            },
            provider_start: None,
            tools,
            controller_tools: None,
            approvals: None,
            catalog,
            effects,
            compaction: None,
            request_refresh: None,
        }
    }

    /// Installs ordered immutable request-local fallback routes and providers atomically.
    #[must_use]
    pub fn with_fallbacks(mut self, configured: Vec<ConfiguredFallback<'ports>>) -> Self {
        if let TurnProvider::Retrying {
            fallbacks,
            executor,
            ..
        } = &mut self.provider
        {
            let (routes, providers): (Vec<_>, Vec<_>) = configured
                .into_iter()
                .map(|candidate| (candidate.route, candidate.provider))
                .unzip();
            *fallbacks = providers;
            *executor = std::sync::Arc::new(ProductionRetryExecutor {
                clock: crate::retry::SystemClock::default(),
                sleeper: crate::retry::TokioSleeper,
                events: TurnRetryEventSink::new(),
                fallbacks: routes,
            });
        }
        self
    }

    /// Preserves the legacy one-fallback composition wrapper.
    #[must_use]
    pub fn with_fallback(self, route: FallbackRoute, provider: &'ports dyn ProviderPort) -> Self {
        self.with_fallbacks(vec![ConfiguredFallback { route, provider }])
    }

    /// Installs compaction and refresh seams for context pressure/overflow.
    #[must_use]
    pub fn with_context_ports(
        mut self,
        compaction: &'ports dyn CompactionPort,
        refresh: std::sync::Arc<dyn RequestRefreshPort>,
    ) -> Self {
        self.compaction = Some(compaction);
        self.request_refresh = Some(refresh);
        self
    }

    /// Installs a callback for the first provider stream admission boundary.
    #[must_use]
    pub fn with_provider_start(mut self, callback: &'catalog dyn ProviderStartPort) -> Self {
        self.provider_start = Some(callback);
        self
    }

    /// Installs the external execution backend.
    #[must_use]
    pub fn with_controller_tools(mut self, port: &'ports dyn ControllerToolPort) -> Self {
        self.controller_tools = Some(port);
        self
    }

    /// Installs the approval backend.
    #[must_use]
    pub fn with_approvals(mut self, port: &'ports dyn ApprovalPort) -> Self {
        self.approvals = Some(port);
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
            controller_tools: None,
            approvals: None,
            catalog,
            effects,
            compaction: None,
            request_refresh: None,
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
        fallbacks: Vec<&'a dyn ProviderPort>,
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
        fallbacks: &'a [&'a dyn ProviderPort],
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
        fallbacks: &'a [&'a dyn ProviderPort],
        request: ProviderRequest,
        output: crate::ports::ProviderEventSink,
    ) -> crate::ports::PortFuture<'a, RetryTerminal> {
        Box::pin(self.execute_candidates(route, source, fallbacks, request, output))
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
    fallbacks: Vec<FallbackRoute>,
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
        fallbacks: &'a [&'a dyn ProviderPort],
        request: ProviderRequest,
        output: crate::ports::ProviderEventSink,
    ) -> crate::ports::PortFuture<'a, RetryTerminal> {
        Box::pin(async move {
            let mut current_route = route;
            let mut current_port = source;
            let mut current_request = request;
            let mut total_attempts = 0_u32;
            for candidate in self
                .fallbacks
                .iter()
                .zip(fallbacks.iter().copied())
                .map(Some)
                .chain(std::iter::once(None))
            {
                let terminal = RetryExecutor::new(
                    &self.clock,
                    &self.sleeper,
                    &self.events,
                    RetryPolicy::default(),
                    None,
                )
                .execute(
                    current_route.clone(),
                    current_port,
                    None,
                    current_request.clone(),
                    output.clone(),
                )
                .await?;
                match terminal {
                    RetryTerminal::Success => return Ok(RetryTerminal::Success),
                    RetryTerminal::Failure {
                        failure,
                        attempt_count,
                    } => {
                        total_attempts = total_attempts.saturating_add(attempt_count);
                        let Some((route, provider)) = candidate else {
                            return Ok(RetryTerminal::Failure {
                                failure,
                                attempt_count: total_attempts,
                            });
                        };
                        if !matches!(
                            failure.kind,
                            crate::retry::ProviderFailureKind::Transient
                                | crate::retry::ProviderFailureKind::Busy
                        ) {
                            return Ok(RetryTerminal::Failure {
                                failure,
                                attempt_count: total_attempts,
                            });
                        }
                        self.events
                            .emit(RetryEvent::new(
                                current_route.clone(),
                                route.destination().clone(),
                                crate::retry::RetryReason::TransportFallback,
                                total_attempts,
                                0,
                                &failure.reason,
                            ))
                            .await?;
                        current_route = route.destination().clone();
                        current_port = provider;
                        current_request.model = route.destination_model().clone();
                    }
                }
            }
            unreachable!("fallback chain includes terminal sentinel")
        })
    }
}

struct TurnContext<'ports, 'catalog> {
    runtime: &'ports mut ListenerRuntime,
    guard: LeaseGuard,
    provider: TurnProvider<'ports>,
    provider_start: Option<&'catalog dyn ProviderStartPort>,
    tools: &'ports dyn ToolPort,
    controller_tools: Option<&'ports dyn ControllerToolPort>,
    approvals: Option<&'ports dyn ApprovalPort>,
    catalog: &'catalog TurnToolCatalog,
    effects: &'ports dyn TurnEffectPort,
    request: ProviderRequest,
    compaction: Option<&'ports dyn CompactionPort>,
    request_refresh: Option<std::sync::Arc<dyn RequestRefreshPort>>,
    context_compactions: u8,
    total_tool_calls: usize,
    lease_generation: u64,
    turn_id: NonEmptyString,
    run_id: RunId,
    input_id: NonEmptyString,
    scope: lotta_domain::RuntimeScope,
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
    let lease_generation = lease.generation();
    let scope = lotta_domain::RuntimeScope::new(
        handle.key().agent_id().clone(),
        handle.key().conversation_id().clone(),
        None,
    );
    let lifecycle = runtime
        .lifecycle(&handle)
        .ok_or_else(|| protocol("runtime lifecycle missing"))?;
    let turn_id = NonEmptyString::new("turn").map_err(|_| protocol("turn id"))?;
    let run_id = lifecycle
        .projection()
        .active_run_ids()
        .first()
        .cloned()
        .ok_or_else(|| protocol("active run id missing"))?;
    let input_id = turn_id.clone();
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
        controller_tools,
        approvals,
        catalog,
        effects,
        compaction,
        request_refresh,
    } = ports;
    let mut turn = TurnContext {
        runtime,
        guard,
        provider,
        provider_start,
        tools,
        controller_tools,
        approvals,
        catalog,
        effects,
        request,
        compaction,
        request_refresh,
        context_compactions: 0,
        total_tool_calls: 0,
        lease_generation,
        turn_id,
        run_id,
        input_id,
        scope,
    };
    run_loop(&mut turn).await
}

async fn run_loop(turn: &mut TurnContext<'_, '_>) -> Result<TurnRunOutcome, RuntimeError> {
    if turn.is_suppressed() {
        if turn.request.cancellation.is_cancelled() {
            return cancel_turn(turn).map(|flow| match flow {
                Flow::Failed => TurnRunOutcome::Completed,
                Flow::Suppressed | Flow::Continue => TurnRunOutcome::Suppressed,
            });
        }
        turn.request.cancellation.cancel();
        return Ok(TurnRunOutcome::Suppressed);
    }
    for step_index in 0..TURN_STEPS_MAX.value {
        let remaining = TURN_TOOL_CALLS_MAX.value - turn.total_tool_calls;
        let mut state = ProviderStepState::new(remaining)?;
        let step = match run_provider_step(turn, &mut state).await {
            Ok(step) => step,
            Err(RuntimeError::Cancelled { .. }) => return cancelled_outcome(turn),
            Err(error) => return Err(error),
        };
        match step {
            Flow::Suppressed => {
                if turn.request.cancellation.is_cancelled() {
                    return cancel_turn(turn).map(|flow| match flow {
                        Flow::Failed => TurnRunOutcome::Completed,
                        Flow::Suppressed | Flow::Continue => TurnRunOutcome::Suppressed,
                    });
                }
                turn.request.cancellation.cancel();
                return Ok(TurnRunOutcome::Suppressed);
            }
            Flow::Failed => return Ok(TurnRunOutcome::Completed),
            Flow::Continue => {}
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

fn cancelled_outcome(turn: &mut TurnContext<'_, '_>) -> Result<TurnRunOutcome, RuntimeError> {
    cancel_turn(turn).map(|flow| match flow {
        Flow::Failed => TurnRunOutcome::Completed,
        Flow::Suppressed | Flow::Continue => TurnRunOutcome::Suppressed,
    })
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
            fallbacks,
            executor,
        } => {
            let retry_events = executor.begin_retry_step()?;
            Ok((
                Box::pin(async move {
                    executor
                        .execute(route, source, &fallbacks, request, sink)
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
    loop {
        if let Some(detail) = preflight_pressure(&turn.request) {
            compact_and_refresh(turn, detail).await?;
            continue;
        }
        match run_provider_attempt(turn, state).await? {
            ProviderStepResult::Flow(flow) => return Ok(flow),
            ProviderStepResult::Overflow(detail) => compact_and_refresh(turn, detail).await?,
        }
    }
}

enum ProviderStepResult {
    Flow(Flow),
    Overflow(crate::ports::ProviderContextOverflowDetail),
}

async fn run_provider_attempt(
    turn: &mut TurnContext<'_, '_>,
    state: &mut ProviderStepState,
) -> Result<ProviderStepResult, RuntimeError> {
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
                        return suppress_provider(turn, &mut output).map(ProviderStepResult::Flow);
                    }
                }
                result = &mut future => break result,
                result = output.receive() => {
                    if drain_ready_retries(turn, &mut retry_events)? == Flow::Suppressed {
                        return suppress_provider(turn, &mut output).map(ProviderStepResult::Flow);
                    }
                    if turn.is_suppressed() {
                        return suppress_provider(turn, &mut output).map(ProviderStepResult::Flow);
                    }
                    match result? {
                        Some(event) => match handle_event(turn, state, event).await? {
                            Flow::Suppressed => {
                                return suppress_provider(turn, &mut output)
                                    .map(ProviderStepResult::Flow);
                            }
                            Flow::Failed => {
                                output.cancel();
                                close_retry_step(&turn.provider)?;
                                return Ok(ProviderStepResult::Flow(Flow::Failed));
                            }
                            Flow::Continue => {}
                        },
                        None => return Err(protocol("provider stream closed before executor")),
                    }
                }
            }
        }
    };
    close_retry_step(&turn.provider)?;
    if turn.request.cancellation.is_cancelled() {
        output.cancel();
        return suppress_provider(turn, &mut output).map(ProviderStepResult::Flow);
    }
    if turn.is_suppressed() || drain_retries(turn, &mut retry_events).await? == Flow::Suppressed {
        return suppress_provider(turn, &mut output).map(ProviderStepResult::Flow);
    }
    let terminal = terminal?;
    if let RetryTerminal::Failure { failure, .. } = &terminal
        && failure.kind == crate::retry::ProviderFailureKind::ContextOverflow
    {
        let detail = failure.context_overflow.clone().unwrap_or_else(|| {
            overflow_detail(
                &turn.request,
                turn.context_compactions,
                estimate_request_tokens(&turn.request),
            )
        });
        return Ok(ProviderStepResult::Overflow(detail));
    }
    handle_terminal(terminal, turn, state, &mut output)
        .await
        .map(ProviderStepResult::Flow)
}

fn estimate_request_tokens(request: &ProviderRequest) -> u64 {
    let bytes = request.normalized_wire_bytes().unwrap_or(usize::MAX);
    u64::try_from(bytes).unwrap_or(u64::MAX).div_ceil(3)
}

fn effective_context_limit(request: &ProviderRequest) -> u64 {
    let context = request.context.unwrap_or_default();
    [
        context.server_max,
        context.catalog_max.or(request.model.context_window),
        context.agent_max,
        context.conversation_max,
        Some(request.context_tokens_max.get()),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(request.context_tokens_max.get())
}

fn overflow_detail(
    request: &ProviderRequest,
    compactions: u8,
    estimated: u64,
) -> crate::ports::ProviderContextOverflowDetail {
    crate::ports::ProviderContextOverflowDetail {
        measured: None,
        estimated: crate::ports::ProviderContextTokenCount {
            tokens: estimated,
            provenance: crate::ports::ProviderContextTokenProvenance::FallbackBytesPerToken,
        },
        limit: effective_context_limit(request),
        provider: request.model.provider_id.as_str().to_owned(),
        model: request.model.handle.as_str().to_owned(),
        attempt: compactions.saturating_add(1),
        compactions_completed: compactions,
    }
}

fn preflight_pressure(
    request: &ProviderRequest,
) -> Option<crate::ports::ProviderContextOverflowDetail> {
    let estimated = estimate_request_tokens(request);
    let context = request.context?;
    let observed = context.measured_input_tokens.unwrap_or(estimated);
    let limit = effective_context_limit(request);
    let reserve = request.output_tokens_max.get().min(limit / 4);
    if observed.saturating_add(reserve) > limit {
        Some(overflow_detail(
            request,
            context.compactions_completed,
            estimated,
        ))
    } else {
        None
    }
}

async fn compact_and_refresh(
    turn: &mut TurnContext<'_, '_>,
    mut detail: crate::ports::ProviderContextOverflowDetail,
) -> Result<(), RuntimeError> {
    if turn.context_compactions >= CONTEXT_OVERFLOW_COMPACTIONS_MAX {
        detail.compactions_completed = turn.context_compactions;
        detail.attempt = turn.context_compactions.saturating_add(1);
        return Err(RuntimeError::ContextOverflow { detail });
    }
    if turn.is_suppressed() {
        return Err(RuntimeError::Cancelled {
            context: "context compaction".into(),
        });
    }
    let compaction = turn.compaction.ok_or_else(|| RuntimeError::Unsupported {
        context: "transcript compaction unavailable".into(),
    })?;
    let refresh = turn
        .request_refresh
        .as_ref()
        .ok_or_else(|| RuntimeError::Unsupported {
            context: "provider request refresh unavailable".into(),
        })?;
    let original_detail = detail.clone();
    let progress = match compaction
        .compact(turn.request.clone(), turn.guard.lease().clone(), detail)
        .await
    {
        Ok(progress) => progress,
        Err(RuntimeError::CompactionUnavailable) => {
            return Err(RuntimeError::ContextOverflow {
                detail: original_detail,
            });
        }
        Err(error) => return Err(error),
    };
    ensure_live_after_context_await(turn)?;
    if !progress.progressed() {
        return Err(RuntimeError::ContextOverflow {
            detail: overflow_detail(
                &turn.request,
                turn.context_compactions,
                progress.tokens_after,
            ),
        });
    }
    turn.context_compactions = turn.context_compactions.saturating_add(1);
    turn.request = refresh
        .refresh(turn.request.clone(), turn.guard.lease().clone())
        .await?;
    ensure_live_after_context_await(turn)?;
    let context = turn.request.context.get_or_insert_default();
    context.compactions_completed = turn.context_compactions;
    context.measured_input_tokens = None;
    Ok(())
}

fn ensure_live_after_context_await(turn: &TurnContext<'_, '_>) -> Result<(), RuntimeError> {
    match turn.guard.apply_after_await(turn.runtime, || ()) {
        LeaseEffect::Applied(()) => Ok(()),
        LeaseEffect::Suppressed(_) => Err(RuntimeError::Cancelled {
            context: "context compaction lease".into(),
        }),
    }
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
    if let RetryTerminal::Failure {
        failure,
        attempt_count,
    } = terminal
    {
        let reason = TurnStopReason::from_failure(&failure);
        let detail =
            ProviderFailureDetail::from_failure(&failure, attempt_count, turn.context_compactions);
        return fail_turn(turn, reason, Some(detail));
    }
    while let Some(event) = output.receive().await? {
        match handle_event(turn, state, event).await? {
            Flow::Suppressed => return suppress_provider(turn, output),
            Flow::Failed => {
                output.cancel();
                return Ok(Flow::Failed);
            }
            Flow::Continue => {}
        }
    }
    state.accumulator.terminal_stop()?;
    state.stop.map_or_else(
        || Err(protocol("provider stream closed without terminal")),
        |_| Ok(Flow::Continue),
    )
}

fn cancel_turn(turn: &mut TurnContext<'_, '_>) -> Result<Flow, RuntimeError> {
    let reason = TurnStopReason::UserCancellation;
    let record = TurnStopRecord {
        turn_id: turn.turn_id.clone(),
        run_id: turn.run_id.clone(),
        input_id: turn.input_id.clone(),
        reason,
        provider_failure: None,
    };
    match turn.guard.finish_cancelled_turn_with_effect_after_await(
        turn.runtime,
        failure_domain_stop(reason),
        || {
            turn.effects.persist_stop_reason(record)?;
            turn.effects.emit(TurnEvent::Failed { reason })
        },
    )? {
        LeaseEffect::Applied(()) => Ok(Flow::Failed),
        LeaseEffect::Suppressed(_) => Ok(Flow::Suppressed),
    }
}

fn fail_turn(
    turn: &mut TurnContext<'_, '_>,
    reason: TurnStopReason,
    provider_failure: Option<ProviderFailureDetail>,
) -> Result<Flow, RuntimeError> {
    let domain = failure_domain_stop(reason);
    let record = TurnStopRecord {
        turn_id: turn.turn_id.clone(),
        run_id: turn.run_id.clone(),
        input_id: turn.input_id.clone(),
        reason,
        provider_failure,
    };
    match turn
        .guard
        .finish_turn_with_effect_after_await(turn.runtime, domain, || {
            turn.effects.persist_stop_reason(record)?;
            turn.effects.emit(TurnEvent::Failed { reason })
        })? {
        LeaseEffect::Applied(()) => Ok(Flow::Failed),
        LeaseEffect::Suppressed(_) => Ok(Flow::Suppressed),
    }
}

fn failure_domain_stop(reason: TurnStopReason) -> lotta_domain::StopReason {
    match lotta_domain::StopReason::new(reason.wire_value()) {
        Ok(reason) => reason,
        Err(_) => unreachable!("static stop reason is valid"),
    }
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
        ProviderEvent::Error { error } => {
            state.accumulator.terminal_error();
            let reason = TurnStopReason::from_provider(&error);
            let failure = error.into_failure();
            let detail = ProviderFailureDetail::from_failure(&failure, 1, turn.context_compactions);
            fail_turn(turn, reason, Some(detail))
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
    require_sequential_tool(definition)?;
    let input = ValidatedToolInput::new(value)?;
    let outcome = execute_branch(turn, &call_id, definition, input).await?;
    if turn.is_suppressed() {
        return Ok(Flow::Suppressed);
    }
    let result = ToolResultRecord { call_id, outcome };
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

async fn execute_branch(
    turn: &mut TurnContext<'_, '_>,
    call_id: &ToolCallId,
    definition: &crate::ports::ToolDefinition,
    mut input: ValidatedToolInput,
) -> Result<ToolOutcome, RuntimeError> {
    let executing_approval = if definition.approval_policy == ToolApprovalPolicy::Always {
        let Some((request, approved_input)) =
            approve_branch(turn, call_id, definition, input).await?
        else {
            return denied("Tool execution denied by user.");
        };
        input = approved_input;
        Some(request)
    } else {
        None
    };
    let execution = approved_execution_request(
        call_id,
        definition,
        input,
        turn,
        executing_approval.as_ref(),
    );
    let outcome = dispatch_tool_execution(turn, call_id, definition, execution).await;
    finalize_approved_execution(turn, outcome, executing_approval)
}

fn approved_execution_request(
    call_id: &ToolCallId,
    definition: &crate::ports::ToolDefinition,
    input: ValidatedToolInput,
    turn: &TurnContext<'_, '_>,
    approval: Option<&ControlRequest>,
) -> ToolExecutionRequest {
    let approval_grant = approval.map_or(crate::ports::ToolApprovalGrant::None, |_| {
        crate::ports::ToolApprovalGrant::granted(call_id.clone(), definition)
    });
    ToolExecutionRequest {
        tool_call_id: call_id.clone(),
        approval_grant,
        definition: definition.clone(),
        input,
        cancellation: turn.request.cancellation.clone(),
        deadline: definition.timeout,
    }
}

async fn dispatch_tool_execution(
    turn: &mut TurnContext<'_, '_>,
    call_id: &ToolCallId,
    definition: &crate::ports::ToolDefinition,
    execution: ToolExecutionRequest,
) -> Result<ToolOutcome, RuntimeError> {
    if definition.execution_owner == ToolExecutionOwner::Rust {
        return turn.tools.execute(execution).await;
    }
    let record = ControllerToolRequestRecord {
        scope: turn.scope.clone(),
        run_id: turn.run_id.clone(),
        lease_generation: turn.lease_generation,
        call_id: call_id.clone(),
        tool_name: NonEmptyString::new(definition.model_name.as_str().to_owned())
            .map_err(|_| protocol("controller tool name"))?,
        input: execution.input.clone(),
    };
    match turn.guard.apply_after_await(turn.runtime, || {
        turn.effects.persist_controller_request(record.clone())
    }) {
        LeaseEffect::Applied(effect) => effect?,
        LeaseEffect::Suppressed(_) => return Err(cancelled("controller request suppressed")),
    }
    turn.controller_tools
        .ok_or_else(|| unavailable("external tool backend"))?
        .execute_external(record, execution)
        .await
}

fn finalize_approved_execution(
    turn: &TurnContext<'_, '_>,
    outcome: Result<ToolOutcome, RuntimeError>,
    approval: Option<ControlRequest>,
) -> Result<ToolOutcome, RuntimeError> {
    match (outcome, approval) {
        (Ok(outcome), Some(request)) => {
            let port = turn
                .approvals
                .ok_or_else(|| unavailable("approval backend"))?;
            if let Err(error) = port.mark_allowed(&request, &outcome) {
                let _ = port.mark_interrupted(&request);
                return Err(error);
            }
            Ok(outcome)
        }
        (Err(error), Some(request)) => {
            if let Some(port) = turn.approvals {
                let _ = port.mark_interrupted(&request);
            }
            Err(error)
        }
        (outcome, None) => outcome,
    }
}

async fn approve_branch(
    turn: &mut TurnContext<'_, '_>,
    call_id: &ToolCallId,
    definition: &crate::ports::ToolDefinition,
    input: ValidatedToolInput,
) -> Result<Option<(ControlRequest, ValidatedToolInput)>, RuntimeError> {
    let request = ControlRequest {
        request_id: NonEmptyString::new(format!(
            "approval-{}-{}",
            turn.lease_generation,
            call_id.as_str()
        ))
        .map_err(|_| protocol("approval request id"))?,
        call_id: call_id.clone(),
        lease_generation: turn.lease_generation,
        tool_name: NonEmptyString::new(definition.model_name.as_str().to_owned())
            .map_err(|_| protocol("approval tool name"))?,
        input: input.clone(),
        schema: definition.input_schema.clone(),
    };
    let approval = turn
        .approvals
        .ok_or_else(|| unavailable("approval backend"))?;
    approval.store_request(request.clone())?;
    match turn.guard.apply_after_await(turn.runtime, || {
        turn.effects.persist_control_request(&request)?;
        turn.effects
            .emit(TurnEvent::ControlRequest(request.clone()))
    }) {
        LeaseEffect::Applied(effect) => effect?,
        LeaseEffect::Suppressed(_) => return Err(cancelled("approval suppressed")),
    }
    let resolution = approval
        .await_resolution(request.clone(), turn.request.cancellation.clone())
        .await?;
    let approved_input = match resolution {
        ApprovalResolution::Deny => return Ok(None),
        ApprovalResolution::Allow(Some(edited)) => ValidatedToolInput::new(edited)?,
        ApprovalResolution::Allow(None) => input,
    };
    Ok(Some((request, approved_input)))
}

fn denied(message: &str) -> Result<ToolOutcome, RuntimeError> {
    Ok(ToolOutcome::UserDenied {
        message: crate::ports::ToolOutcomeMessage::new(message.to_owned())?,
    })
}

fn require_sequential_tool(definition: &crate::ports::ToolDefinition) -> Result<(), RuntimeError> {
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

fn unavailable(context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "tool_branch_unavailable",
        context: context.into(),
    }
}

fn cancelled(context: &'static str) -> RuntimeError {
    RuntimeError::Cancelled {
        context: context.into(),
    }
}

fn unsupported(context: &'static str) -> RuntimeError {
    RuntimeError::Unsupported {
        context: context.into(),
    }
}

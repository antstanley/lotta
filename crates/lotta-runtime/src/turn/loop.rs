use super::step::tool_message;
use super::{ProjectionKind, ToolResultRecord, TurnEffectPort, TurnEvent, TurnToolCatalog};
use crate::bounds::{PROVIDER_MESSAGES_MAX, TURN_STEPS_MAX, TURN_TOOL_CALLS_MAX};
use crate::ports::{
    ParallelSafety, ProviderEvent, ProviderMessage, ProviderMessages, ProviderPort,
    ProviderRequest, StopReason, ToolApprovalPolicy, ToolCallAccumulator, ToolCallId,
    ToolExecutionOwner, ToolExecutionRequest, ToolPort, ValidatedToolInput, provider_event_channel,
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
pub struct TurnPorts<'a> {
    /// Normalized provider stream boundary.
    pub provider: &'a dyn ProviderPort,
    /// Local tool execution boundary.
    pub tools: &'a dyn ToolPort,
    /// Tools admitted for this turn.
    pub catalog: &'a TurnToolCatalog,
    /// Owner-local projection and event effects.
    pub effects: &'a dyn TurnEffectPort,
}

impl<'a> TurnPorts<'a> {
    /// Groups the four borrowed turn ports for an ergonomic [`run_turn`] call.
    #[must_use]
    pub const fn new(
        provider: &'a dyn ProviderPort,
        tools: &'a dyn ToolPort,
        catalog: &'a TurnToolCatalog,
        effects: &'a dyn TurnEffectPort,
    ) -> Self {
        Self {
            provider,
            tools,
            catalog,
            effects,
        }
    }
}

struct TurnContext<'a> {
    runtime: &'a mut ListenerRuntime,
    guard: LeaseGuard,
    ports: TurnPorts<'a>,
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
    ports: TurnPorts<'_>,
) -> Result<TurnRunOutcome, RuntimeError> {
    let guard = LeaseGuard::new(
        handle,
        lease,
        request.cancellation.clone(),
        CancellationPolicy::SuppressWhenCancelled,
    );
    let mut turn = TurnContext {
        runtime,
        guard,
        ports,
        request,
        total_tool_calls: 0,
    };
    run_loop(&mut turn).await
}

async fn run_loop(turn: &mut TurnContext<'_>) -> Result<TurnRunOutcome, RuntimeError> {
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
    turn: &mut TurnContext<'_>,
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
    turn: &mut TurnContext<'_>,
    reason: StopReason,
) -> Result<TurnRunOutcome, RuntimeError> {
    let domain = domain_stop(reason)?;
    match turn
        .guard
        .finish_turn_with_effect_after_await(turn.runtime, domain, || {
            turn.ports.effects.emit(TurnEvent::Finished { reason })
        })? {
        LeaseEffect::Applied(()) => Ok(TurnRunOutcome::Completed),
        LeaseEffect::Suppressed(_) => Ok(TurnRunOutcome::Suppressed),
    }
}

async fn run_provider_step(
    turn: &mut TurnContext<'_>,
    state: &mut ProviderStepState,
) -> Result<Flow, RuntimeError> {
    turn.request.validate_bytes()?;
    let (sink, mut receiver) = provider_event_channel(1, &turn.request.cancellation)?;
    let future = turn.ports.provider.stream(turn.request.clone(), sink);
    tokio::pin!(future);
    let mut provider_done = false;
    let mut channel_done = false;
    while !provider_done || !channel_done {
        tokio::select! {
            result = &mut future, if !provider_done => {
                provider_done = true;
                if turn.is_suppressed() {
                    receiver.cancel();
                    return Ok(Flow::Suppressed);
                }
                result?;
            }
            result = receiver.receive(), if !channel_done => {
                if turn.is_suppressed() {
                    receiver.cancel();
                    return Ok(Flow::Suppressed);
                }
                match result? {
                    Some(event) => {
                        if handle_event(turn, state, event).await? == Flow::Suppressed {
                            receiver.cancel();
                            turn.request.cancellation.cancel();
                            return Ok(Flow::Suppressed);
                        }
                    }
                    None => channel_done = true,
                }
            }
        }
    }
    state.accumulator.terminal_stop()?;
    if state.stop.is_none() {
        return Err(protocol("provider stream closed without terminal"));
    }
    Ok(Flow::Continue)
}

async fn handle_event(
    turn: &mut TurnContext<'_>,
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

impl TurnContext<'_> {
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
            self.ports.effects.persist_projection(projection.clone())?;
            self.ports.effects.emit(TurnEvent::StreamDelta(projection))
        }) {
            LeaseEffect::Applied(result) => result.map(|()| Flow::Continue),
            LeaseEffect::Suppressed(_) => Ok(Flow::Suppressed),
        }
    }
}

fn start_call(
    turn: &mut TurnContext<'_>,
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
    turn: &mut TurnContext<'_>,
    state: &mut ProviderStepState,
    call_id: ToolCallId,
) -> Result<Flow, RuntimeError> {
    let value = state.accumulator.end(&call_id)?;
    let Some(name) = state.names.remove(&call_id) else {
        state.accumulator.terminal_error();
        return Err(protocol("provider tool call name missing"));
    };
    let definition = turn
        .ports
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
    let outcome = turn.ports.tools.execute(execution).await;
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
        turn.ports.effects.append_tool_result(result.clone())?;
        turn.ports
            .effects
            .emit(TurnEvent::ToolResult(result.clone()))
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

//! Runtime-owned cancellation settlement.

use super::{ToolResultRecord, TurnEffectPort, TurnEvent, TurnStopRecord};
use crate::ports::{PortFuture, ToolCallId, ToolOutcome, ToolOutcomeMessage};
use crate::{CancellationClaim, LeaseEffect, LeaseGuard, ListenerRuntime, RuntimeError};
use lotta_domain::StopReason;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Cooperative cancellation grace before forceful child cleanup.
pub const TURN_CANCEL_GRACE_MS: u64 = 10_000;
/// SIGTERM-to-SIGKILL grace used by scoped child supervisors.
pub const CHILD_KILL_GRACE_MS: u64 = 2_000;

/// The six production cancellation stages in their required order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelStep {
    /// Claim the exact Active owner and cancel admitted work.
    ClaimAndCancel,
    /// Persist and emit interruptions for unfinished calls.
    NormalizeUnfinished,
    /// Suppress every non-cleanup lease effect.
    SuppressEffects,
    /// Wait the cooperative grace and reap scoped children.
    KillChildren,
    /// Enter the canonical terminal persistence transaction.
    PersistTerminal,
    /// Emit the terminal event and release the exact lease.
    EmitAndRelease,
}

/// One provider-ordered tool call and its cancellation token.
pub struct UnfinishedToolCall {
    /// Stable provider call identifier.
    pub call_id: ToolCallId,
    /// Local or externally-owned invocation token.
    pub cancellation: CancellationToken,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CallState {
    Pending,
    Completed,
    Interrupted,
}

/// Ordered state tracker for local and controller-owned calls.
#[derive(Default)]
pub struct UnfinishedCallTracker {
    calls: Vec<UnfinishedToolCall>,
    states: HashMap<ToolCallId, CallState>,
}

impl UnfinishedCallTracker {
    /// Registers one pending call in provider order; duplicate identifiers are ignored safely.
    pub fn register(&mut self, call: UnfinishedToolCall) -> bool {
        if self.states.contains_key(&call.call_id) {
            return false;
        }
        self.states.insert(call.call_id.clone(), CallState::Pending);
        self.calls.push(call);
        true
    }

    /// Marks a call completed before its result is persisted.
    pub fn mark_completed(&mut self, call_id: &ToolCallId) -> bool {
        match self.states.get_mut(call_id) {
            Some(state @ CallState::Pending) => {
                *state = CallState::Completed;
                true
            }
            Some(CallState::Completed | CallState::Interrupted) | None => false,
        }
    }

    /// Restores a pre-persistence completion marker after the effect is suppressed or fails.
    pub fn restore_pending(&mut self, call_id: &ToolCallId) {
        if let Some(state @ CallState::Completed) = self.states.get_mut(call_id) {
            *state = CallState::Pending;
        }
    }

    fn cancel_pending(&self) {
        for call in &self.calls {
            if self.states.get(&call.call_id) == Some(&CallState::Pending) {
                call.cancellation.cancel();
            }
        }
    }

    fn drain_pending(&mut self) -> Vec<ToolCallId> {
        let mut pending = Vec::new();
        for call in &self.calls {
            if self.states.get(&call.call_id) == Some(&CallState::Pending) {
                self.states
                    .insert(call.call_id.clone(), CallState::Interrupted);
                pending.push(call.call_id.clone());
            }
        }
        pending
    }
}

/// Runtime-owned child process scope used only during cancellation settlement.
pub trait TurnChildOwner: Send + Sync {
    /// Reports whether this exact turn scope currently owns operations.
    fn has_operations(&self) -> bool;
    /// Cancels, two-stage kills, joins, and reaps every child in this turn scope.
    fn terminate_and_reap(&self, kill_grace: Duration) -> PortFuture<'_, ()>;
}

/// Post-terminal work whose failure cannot alter the terminal outcome.
pub trait PostTurnPort: Send + Sync {
    /// Runs reflection and memory push after terminal projection and emission.
    fn run(&self) -> PortFuture<'_, ()>;
}

/// Complete cancellation inputs owned by one admitted turn.
pub(crate) struct CancelContext<'a> {
    pub(crate) runtime: &'a mut ListenerRuntime,
    pub(crate) guard: LeaseGuard,
    pub(crate) provider_cancellation: CancellationToken,
    pub(crate) unfinished: UnfinishedCallTracker,
    pub(crate) effects: &'a dyn TurnEffectPort,
    pub(crate) children: Option<&'a dyn TurnChildOwner>,
    pub(crate) post_turn: Option<&'a dyn PostTurnPort>,
    pub(crate) record: TurnStopRecord,
    pub(crate) stages: Option<Arc<dyn Fn(CancelStep) + Send + Sync>>,
}

impl CancelContext<'_> {
    fn stage(&self, step: CancelStep) {
        if let Some(record) = &self.stages {
            record(step);
        }
    }
}

/// Settles cancellation through the canonical six-stage owner sequence.
///
/// # Errors
/// Returns durable projection, child cleanup, or lifecycle failures.
pub(super) async fn cancel_turn(
    context: &mut CancelContext<'_>,
) -> Result<LeaseEffect<crate::CancellationReceipt>, RuntimeError> {
    context.stage(CancelStep::ClaimAndCancel);
    let Some(claim) = claim_and_cancel(context)? else {
        return Ok(LeaseEffect::Suppressed(
            crate::SuppressionReason::CancellationDenied,
        ));
    };
    context.stage(CancelStep::NormalizeUnfinished);
    normalize_unfinished(context)?;
    context.stage(CancelStep::SuppressEffects);
    assert_cleanup_permitted(context);
    context.stage(CancelStep::KillChildren);
    kill_children(context).await?;
    context.stage(CancelStep::PersistTerminal);
    context.stage(CancelStep::EmitAndRelease);
    let outcome = finish_terminal(context, claim)?;
    if matches!(outcome, LeaseEffect::Applied(_)) {
        run_post_turn(context).await;
    }
    Ok(outcome)
}

fn claim_and_cancel(
    context: &mut CancelContext<'_>,
) -> Result<Option<CancellationClaim>, RuntimeError> {
    match context.guard.claim_cancellation(context.runtime)? {
        LeaseEffect::Applied(claim) => {
            context.provider_cancellation.cancel();
            context.unfinished.cancel_pending();
            Ok(Some(claim))
        }
        LeaseEffect::Suppressed(_) => Ok(None),
    }
}

fn normalize_unfinished(context: &mut CancelContext<'_>) -> Result<(), RuntimeError> {
    for call_id in context.unfinished.drain_pending() {
        let result = ToolResultRecord {
            call_id,
            outcome: interruption()?,
        };
        context.effects.append_tool_result(result.clone())?;
        context.effects.emit(TurnEvent::ToolResult(result))?;
    }
    Ok(())
}

fn interruption() -> Result<ToolOutcome, RuntimeError> {
    Ok(ToolOutcome::Interruption {
        message: ToolOutcomeMessage::new("Tool execution interrupted by cancellation.".into())?,
    })
}

fn assert_cleanup_permitted(context: &CancelContext<'_>) {
    assert!(context.provider_cancellation.is_cancelled());
}

async fn kill_children(context: &CancelContext<'_>) -> Result<(), RuntimeError> {
    if let Some(children) = context.children
        && children.has_operations()
    {
        tokio::time::sleep(Duration::from_millis(TURN_CANCEL_GRACE_MS)).await;
        children
            .terminate_and_reap(Duration::from_millis(CHILD_KILL_GRACE_MS))
            .await?;
    }
    Ok(())
}

fn finish_terminal(
    context: &mut CancelContext<'_>,
    claim: CancellationClaim,
) -> Result<LeaseEffect<crate::CancellationReceipt>, RuntimeError> {
    let outcome = context
        .guard
        .finish_cancelled_turn_with_effect_after_await(
            context.runtime,
            claim,
            cancelled_stop_reason(),
            || {
                context
                    .effects
                    .persist_stop_reason(context.record.clone())?;
                context.effects.emit(TurnEvent::Cancelled)
            },
        )?;
    Ok(outcome)
}

fn cancelled_stop_reason() -> StopReason {
    match StopReason::new("user_cancellation") {
        Ok(reason) => reason,
        Err(_) => unreachable!("static cancellation reason is valid"),
    }
}

async fn run_post_turn(context: &CancelContext<'_>) {
    if let Some(post_turn) = context.post_turn
        && let Err(error) = post_turn.run().await
    {
        tracing::warn!(error = %error, "post-turn cancellation cleanup failed");
    }
}

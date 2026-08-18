//! Runtime lifecycle ownership and derived projections.

use crate::RuntimeError;
use lotta_domain::{
    DomainError, LoopStatus, RunId, RuntimeScope, StopReason, TurnLease, TurnLifecycle,
    TurnStateKind,
};
use std::fmt;
use uuid::Uuid;

/// Runtime owner for exactly one domain lifecycle and immutable scope.
pub struct LifecycleOwner {
    scope: RuntimeScope,
    lifecycle: TurnLifecycle,
}

impl fmt::Debug for LifecycleOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LifecycleOwner")
            .field("agent_id", &self.scope.agent_id)
            .field("conversation_id", &self.scope.conversation_id)
            .field("kind", &self.lifecycle.state().kind())
            .finish()
    }
}

impl LifecycleOwner {
    /// Creates an idle owner with a caller-injected unique identity.
    #[must_use]
    pub fn new(scope: RuntimeScope, owner_id: Uuid) -> Self {
        Self {
            scope,
            lifecycle: TurnLifecycle::new(owner_id),
        }
    }

    /// Returns immutable runtime identity.
    #[must_use]
    pub const fn scope(&self) -> &RuntimeScope {
        &self.scope
    }

    /// Derives the complete projection from domain lifecycle state.
    #[must_use]
    pub fn projection(&self) -> LifecycleProjection<'_> {
        LifecycleProjection {
            state: self.lifecycle.state().kind(),
            is_processing: self.lifecycle.state().is_processing(),
            loop_status: self.lifecycle.state().loop_status(),
            active_run_ids: self.lifecycle.state().active_run_ids(),
            last_stop_reason: self.lifecycle.last_stop_reason(),
        }
    }

    /// Returns whether an opaque lease is current for this owner.
    #[must_use]
    pub fn is_current(&self, lease: &TurnLease) -> bool {
        self.lifecycle.is_current(lease)
    }

    /// Starts command ownership from idle.
    ///
    /// # Errors
    /// Returns a stable runtime error if lease generation is exhausted.
    pub fn begin_command(&mut self) -> Result<TurnLease, RuntimeError> {
        let from = self.projection().state();
        match self.lifecycle.start_command() {
            Ok(lease) => Ok(lease),
            Err(DomainError::TurnTransition { .. }) => {
                invariant(&self.scope, from, TurnStateKind::Command)
            }
            Err(error) => Err(map_domain(&error)),
        }
    }

    /// Starts provider turn ownership from idle.
    ///
    /// # Errors
    /// Returns a stable runtime error for invalid input or generation exhaustion.
    pub fn begin_turn(
        &mut self,
        turn_id: String,
        run_id: RunId,
    ) -> Result<TurnLease, RuntimeError> {
        let from = self.projection().state();
        match self.lifecycle.start_turn(turn_id, run_id) {
            Ok(lease) => Ok(lease),
            Err(DomainError::TurnTransition { .. }) => {
                invariant(&self.scope, from, TurnStateKind::Active)
            }
            Err(error) => Err(map_domain(&error)),
        }
    }

    /// Moves a current active turn into cancellation settlement.
    ///
    /// # Errors
    /// Returns a stable runtime error when the lease is stale.
    pub fn request_cancellation(&mut self, lease: &TurnLease) -> Result<(), RuntimeError> {
        self.lifecycle
            .request_cancellation(lease)
            .map_err(|error| map_domain(&error))
    }

    /// Synchronously finishes a current command.
    ///
    /// # Errors
    /// Returns a stable runtime error when the lease is stale.
    pub fn finish_command(&mut self, lease: &TurnLease) -> Result<(), RuntimeError> {
        if !self.lifecycle.is_current(lease) {
            return Err(map_domain(&DomainError::StaleTurnLease));
        }
        let from = self.projection().state();
        match self.lifecycle.finish_command(lease) {
            Ok(()) => Ok(()),
            Err(DomainError::TurnTransition { .. }) => {
                invariant(&self.scope, from, TurnStateKind::Idle)
            }
            Err(error) => Err(map_domain(&error)),
        }
    }

    /// Finishes a current active or cancelling turn.
    ///
    /// # Errors
    /// Returns a stable runtime error when the lease is stale or the transition is invalid.
    pub fn finish_turn(
        &mut self,
        lease: &TurnLease,
        stop_reason: StopReason,
    ) -> Result<(), RuntimeError> {
        let from = self.projection().state();
        match self.lifecycle.finish_turn(lease, stop_reason) {
            Ok(()) => Ok(()),
            Err(DomainError::TurnTransition { .. }) => {
                invariant(&self.scope, from, TurnStateKind::Idle)
            }
            Err(error) => Err(map_domain(&error)),
        }
    }
}

/// Borrowed UI projection derived on demand from the domain lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifecycleProjection<'a> {
    state: TurnStateKind,
    is_processing: bool,
    loop_status: LoopStatus,
    active_run_ids: &'a [RunId],
    last_stop_reason: Option<&'a StopReason>,
}

impl<'a> LifecycleProjection<'a> {
    /// Returns the lifecycle kind.
    #[must_use]
    pub const fn state(self) -> TurnStateKind {
        self.state
    }
    /// Returns whether provider processing is active.
    #[must_use]
    pub const fn is_processing(self) -> bool {
        self.is_processing
    }
    /// Returns the baseline loop status.
    #[must_use]
    pub const fn loop_status(self) -> LoopStatus {
        self.loop_status
    }
    /// Returns zero or one active run IDs.
    #[must_use]
    pub const fn active_run_ids(self) -> &'a [RunId] {
        self.active_run_ids
    }
    /// Returns the last settled stop reason.
    #[must_use]
    pub const fn last_stop_reason(self) -> Option<&'a StopReason> {
        self.last_stop_reason
    }
}

fn map_domain(error: &DomainError) -> RuntimeError {
    RuntimeError::InvalidData {
        context: match error {
            DomainError::EmptyTurnId => "domain.runtime.empty_turn_id",
            DomainError::TurnLeaseGenerationExhausted => {
                "domain.runtime.lease_generation_exhausted"
            }
            DomainError::StaleTurnLease => "domain.runtime.stale_turn_lease",
            _ => "domain.runtime.turn_transition",
        }
        .into(),
    }
}

/// Centralized intentional production panic for impossible owner transitions.
#[cold]
#[track_caller]
fn invariant(scope: &RuntimeScope, from: TurnStateKind, to: TurnStateKind) -> ! {
    tracing::error!(
        agent_id = scope.agent_id.as_str(),
        conversation_id = scope.conversation_id.as_str(),
        ?from,
        ?to,
        "lifecycle invariant violation"
    );
    panic!(
        "lifecycle invariant violation agent={} conversation={} from={from:?} to={to:?}",
        scope.agent_id.as_str(),
        scope.conversation_id.as_str()
    )
}

#[cfg(test)]
include!("lifecycle/tests.rs");

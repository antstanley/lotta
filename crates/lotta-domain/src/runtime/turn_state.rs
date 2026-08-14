use crate::RunId;
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use uuid::Uuid;

/// Opaque owner-and-generation token minted by a lifecycle owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnLease {
    owner_id: Uuid,
    generation: u64,
}

/// Stable projection of the current turn state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnStateKind {
    /// No command or turn owns the runtime.
    Idle,
    /// A device, slash, or mod command owns the runtime.
    Command,
    /// A provider-backed turn owns the runtime.
    Active,
    /// The active turn is settling cancellation.
    Cancelling,
}

/// Baseline loop-status projection derived from the turn state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopStatus {
    /// The runtime accepts input.
    WaitingOnInput,
    /// A non-turn command is executing.
    ExecutingCommand,
    /// An active turn is sending its provider request.
    SendingApiRequest,
}

/// Opaque validated terminal stop reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StopReason(String);

impl StopReason {
    /// Validates a non-empty stop reason.
    ///
    /// # Errors
    /// Returns [`StopReasonError`] if `value` is empty.
    pub fn new(value: impl Into<String>) -> Result<Self, StopReasonError> {
        let value = value.into();
        if value.is_empty() {
            Err(StopReasonError)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the validated string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A stop reason was empty.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("stop reason must be non-empty")]
pub struct StopReasonError;

#[derive(Clone, Debug, Eq, PartialEq)]
enum TurnState {
    Idle,
    Command {
        lease: TurnLease,
    },
    Active {
        lease: TurnLease,
        turn_id: String,
        run_id: RunId,
    },
    Cancelling {
        lease: TurnLease,
        turn_id: String,
        run_id: RunId,
    },
}

impl TurnState {
    const fn kind(&self) -> TurnStateKind {
        match self {
            Self::Idle => TurnStateKind::Idle,
            Self::Command { .. } => TurnStateKind::Command,
            Self::Active { .. } => TurnStateKind::Active,
            Self::Cancelling { .. } => TurnStateKind::Cancelling,
        }
    }

    const fn lease(&self) -> Option<&TurnLease> {
        match self {
            Self::Idle => None,
            Self::Command { lease }
            | Self::Active { lease, .. }
            | Self::Cancelling { lease, .. } => Some(lease),
        }
    }
}

/// Read-only projection of lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurnStateView<'a> {
    state: &'a TurnState,
}

impl<'a> TurnStateView<'a> {
    /// Returns the stable state-kind projection.
    #[must_use]
    pub const fn kind(self) -> TurnStateKind {
        self.state.kind()
    }

    /// Returns whether a provider-backed turn is actively processing.
    #[must_use]
    pub const fn is_processing(self) -> bool {
        matches!(self.state, TurnState::Active { .. })
    }

    /// Returns the baseline loop-status projection.
    #[must_use]
    pub const fn loop_status(self) -> LoopStatus {
        match self.state {
            TurnState::Idle | TurnState::Cancelling { .. } => LoopStatus::WaitingOnInput,
            TurnState::Command { .. } => LoopStatus::ExecutingCommand,
            TurnState::Active { .. } => LoopStatus::SendingApiRequest,
        }
    }

    /// Returns the active or cancelling provider run identifier.
    #[must_use]
    pub fn active_run_ids(self) -> &'a [RunId] {
        match self.state {
            TurnState::Active { run_id, .. } | TurnState::Cancelling { run_id, .. } => {
                std::slice::from_ref(run_id)
            }
            TurnState::Idle | TurnState::Command { .. } => &[],
        }
    }
}

/// The sole owner of one runtime's turn lifecycle.
pub struct TurnLifecycle {
    owner_id: Uuid,
    generation: u64,
    state: TurnState,
    last_stop_reason: Option<StopReason>,
}

impl fmt::Debug for TurnLifecycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TurnLifecycle")
            .field("state", &self.state.kind())
            .field("last_stop_reason", &self.last_stop_reason)
            .finish_non_exhaustive()
    }
}

impl Default for TurnLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnLifecycle {
    /// Creates an idle lifecycle owner with a unique identity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            owner_id: Uuid::new_v4(),
            generation: 0,
            state: TurnState::Idle,
            last_stop_reason: None,
        }
    }

    /// Returns a read-only state projection.
    #[must_use]
    pub const fn state(&self) -> TurnStateView<'_> {
        TurnStateView { state: &self.state }
    }

    /// Returns the last successfully settled turn stop reason.
    #[must_use]
    pub const fn last_stop_reason(&self) -> Option<&StopReason> {
        self.last_stop_reason.as_ref()
    }

    /// Starts a command from idle and returns its opaque lease.
    ///
    /// # Errors
    /// Returns [`TurnTransitionError`] unless the lifecycle is idle.
    pub fn start_command(&mut self) -> Result<TurnLease, TurnLifecycleError> {
        self.require_state(TurnStateKind::Idle, TurnStateKind::Command)?;
        let lease = self.next_lease()?;
        self.state = TurnState::Command {
            lease: lease.clone(),
        };
        Ok(lease)
    }

    /// Finishes the command holding `lease`.
    ///
    /// # Errors
    /// Returns a typed error unless command state and the complete lease match.
    pub fn finish_command(&mut self, lease: &TurnLease) -> Result<(), TurnLifecycleError> {
        self.require_lease(lease, TurnStateKind::Command, TurnStateKind::Idle)?;
        self.state = TurnState::Idle;
        Ok(())
    }

    /// Starts a provider-backed turn from idle and returns its opaque lease.
    ///
    /// # Errors
    /// Returns a typed error unless the lifecycle is idle and `turn_id` is non-empty.
    pub fn start_turn(
        &mut self,
        turn_id: String,
        run_id: RunId,
    ) -> Result<TurnLease, TurnLifecycleError> {
        self.require_state(TurnStateKind::Idle, TurnStateKind::Active)?;
        if turn_id.is_empty() {
            return Err(TurnIdError.into());
        }
        let lease = self.next_lease()?;
        self.state = TurnState::Active {
            lease: lease.clone(),
            turn_id,
            run_id,
        };
        self.last_stop_reason = None;
        Ok(lease)
    }

    /// Requests cancellation of the active turn holding `lease`.
    ///
    /// # Errors
    /// Returns a typed error unless active state and the complete lease match.
    pub fn request_cancellation(&mut self, lease: &TurnLease) -> Result<(), TurnLifecycleError> {
        self.require_lease(lease, TurnStateKind::Active, TurnStateKind::Cancelling)?;
        let prior = std::mem::replace(&mut self.state, TurnState::Idle);
        if let TurnState::Active {
            lease,
            turn_id,
            run_id,
        } = prior
        {
            self.state = TurnState::Cancelling {
                lease,
                turn_id,
                run_id,
            };
            Ok(())
        } else {
            unreachable!("state was checked before replacement")
        }
    }

    /// Settles an active or cancelling turn holding `lease`.
    ///
    /// # Errors
    /// Returns a typed error unless turn state and the complete lease match.
    pub fn finish_turn(
        &mut self,
        lease: &TurnLease,
        stop_reason: StopReason,
    ) -> Result<(), TurnLifecycleError> {
        let from = self.state.kind();
        if !matches!(from, TurnStateKind::Active | TurnStateKind::Cancelling) {
            return Err(TurnTransitionError {
                from,
                to: TurnStateKind::Idle,
            }
            .into());
        }
        self.require_current_lease(lease)?;
        self.state = TurnState::Idle;
        self.last_stop_reason = Some(stop_reason);
        Ok(())
    }

    fn next_lease(&mut self) -> Result<TurnLease, TurnLeaseExhaustedError> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(TurnLeaseExhaustedError)?;
        self.generation = generation;
        Ok(TurnLease {
            owner_id: self.owner_id,
            generation,
        })
    }

    fn require_state(
        &self,
        required: TurnStateKind,
        to: TurnStateKind,
    ) -> Result<(), TurnTransitionError> {
        let from = self.state.kind();
        if from == required {
            Ok(())
        } else {
            Err(TurnTransitionError { from, to })
        }
    }

    fn require_lease(
        &self,
        lease: &TurnLease,
        required: TurnStateKind,
        to: TurnStateKind,
    ) -> Result<(), TurnLifecycleError> {
        self.require_state(required, to)?;
        self.require_current_lease(lease)
    }

    fn require_current_lease(&self, lease: &TurnLease) -> Result<(), TurnLifecycleError> {
        match self.state.lease() {
            Some(current) if current == lease => Ok(()),
            Some(_) => Err(TurnLeaseError.into()),
            None => Err(TurnTransitionError {
                from: TurnStateKind::Idle,
                to: TurnStateKind::Idle,
            }
            .into()),
        }
    }
}

/// A requested turn-state transition is not a lifecycle-diagram edge.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("illegal turn transition from {from:?} to {to:?}")]
pub struct TurnTransitionError {
    /// Current state kind.
    pub from: TurnStateKind,
    /// Requested state kind.
    pub to: TurnStateKind,
}

/// A lease did not match the current owner's complete token.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("turn lease is not current for this lifecycle owner")]
pub struct TurnLeaseError;

/// A lifecycle owner exhausted its lease generation space.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("turn lease generation exhausted")]
pub struct TurnLeaseExhaustedError;

/// A turn identifier was empty.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("turn identifier must be non-empty")]
pub struct TurnIdError;

/// A lifecycle operation failed without mutating owner state.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum TurnLifecycleError {
    /// The requested lifecycle edge was illegal.
    #[error(transparent)]
    Transition(#[from] TurnTransitionError),
    /// The supplied lease was stale or belonged to another owner.
    #[error(transparent)]
    Lease(#[from] TurnLeaseError),
    /// The owner cannot mint another generation.
    #[error(transparent)]
    LeaseExhausted(#[from] TurnLeaseExhaustedError),
    /// The requested turn identifier was empty.
    #[error(transparent)]
    InvalidTurnId(#[from] TurnIdError),
}

#[cfg(test)]
impl TurnLifecycle {
    pub(super) fn test_set_generation(&mut self, generation: u64) {
        self.generation = generation;
    }

    pub(super) const fn test_generation(&self) -> u64 {
        self.generation
    }
}

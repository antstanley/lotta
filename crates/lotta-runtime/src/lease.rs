//! Lease guards for effects resumed after asynchronous boundaries.
//!
//! Capture a guard before an await, then call one enforcement method after every await. Callers
//! must never hold a registry lock across an await.

use crate::{ListenerRuntime, RuntimeHandle};
use lotta_domain::{StopReason, TurnLease};
use std::marker::PhantomData;
use tokio_util::sync::CancellationToken;

/// Determines whether a cancelled token permits cleanup effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationPolicy {
    /// Reject every effect after cancellation.
    SuppressWhenCancelled,
    /// Permit cancellation cleanup while all lease checks remain current.
    PermitDuringCancellationCleanup,
}

/// Stable reason an awaited effect was suppressed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuppressionReason {
    /// The listener has been deactivated.
    ListenerInactive,
    /// The exact runtime handle is absent.
    RuntimeMissing,
    /// A replacement lease owns the runtime.
    StaleLease,
    /// Cancellation policy denies this effect.
    CancellationDenied,
}

/// Outcome of enforcing an awaited effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseEffect<T> {
    /// The effect passed all checks and was applied.
    Applied(T),
    /// The effect was intentionally not invoked.
    Suppressed(SuppressionReason),
}

/// Unique capability minted by the exact current Active-to-Cancelling transition.
pub struct CancellationClaim {
    _private: PhantomData<()>,
}

/// Receipt returned after the canonical cancellation transaction releases the lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CancellationReceipt {
    _private: PhantomData<()>,
}

impl CancellationReceipt {
    const fn new() -> Self {
        Self {
            _private: PhantomData,
        }
    }
}

/// Capability captured before an await and checked against live registry state afterward.
pub struct LeaseGuard {
    handle: RuntimeHandle,
    lease: TurnLease,
    cancellation: CancellationToken,
    policy: CancellationPolicy,
}

impl LeaseGuard {
    /// Captures the exact runtime, lease, cancellation token, and policy before an await.
    #[must_use]
    pub fn new(
        handle: RuntimeHandle,
        lease: TurnLease,
        cancellation: CancellationToken,
        policy: CancellationPolicy,
    ) -> Self {
        Self {
            handle,
            lease,
            cancellation,
            policy,
        }
    }

    /// Returns the exact captured runtime handle.
    #[must_use]
    pub const fn handle(&self) -> &RuntimeHandle {
        &self.handle
    }

    /// Returns the exact captured turn lease for scoped adapter calls.
    #[must_use]
    pub const fn lease(&self) -> &TurnLease {
        &self.lease
    }

    /// Applies one effect only after all four live checks pass.
    pub fn apply_after_await<T>(
        &self,
        registry: &ListenerRuntime,
        effect: impl FnOnce() -> T,
    ) -> LeaseEffect<T> {
        match self.check(registry) {
            Ok(()) => LeaseEffect::Applied(effect()),
            Err(reason) => LeaseEffect::Suppressed(reason),
        }
    }

    /// Atomically checks and releases this exact current turn after an await.
    pub fn finish_turn_after_await(
        &self,
        registry: &mut ListenerRuntime,
        stop_reason: StopReason,
    ) -> LeaseEffect<()> {
        if let Err(reason) = self.check(registry) {
            return LeaseEffect::Suppressed(reason);
        }
        let Ok(owner) = registry.lifecycle_mut(&self.handle) else {
            return LeaseEffect::Suppressed(SuppressionReason::RuntimeMissing);
        };
        match owner.finish_turn(&self.lease, stop_reason) {
            Ok(()) => LeaseEffect::Applied(()),
            Err(_) => LeaseEffect::Suppressed(SuppressionReason::StaleLease),
        }
    }

    /// Atomically claims cancellation for the exact current Active owner.
    ///
    /// Already-cancelling, idle, missing, inactive, and stale owners mint no claim.
    ///
    /// # Errors
    /// Returns a typed lifecycle ownership failure.
    pub fn claim_cancellation(
        &self,
        registry: &mut ListenerRuntime,
    ) -> Result<LeaseEffect<CancellationClaim>, crate::RuntimeError> {
        if let Err(reason) = self.check_owner(registry) {
            return Ok(LeaseEffect::Suppressed(reason));
        }
        let owner =
            registry
                .lifecycle_mut(&self.handle)
                .map_err(|_| crate::RuntimeError::NotFound {
                    context: "checked cancellation runtime".into(),
                })?;
        if owner.projection().state() != lotta_domain::TurnStateKind::Active {
            return Ok(LeaseEffect::Suppressed(
                SuppressionReason::CancellationDenied,
            ));
        }
        owner.request_cancellation(&self.lease)?;
        Ok(LeaseEffect::Applied(CancellationClaim {
            _private: PhantomData,
        }))
    }

    /// Persists and emits cancellation, then releases the exact cancelling owner.
    ///
    /// Only the holder of the unique transition claim may invoke this transaction. Effect failure
    /// retains the Cancelling owner for recovery; release follows successful persistence/emission.
    ///
    /// # Errors
    /// Returns the durable effect failure or a lifecycle ownership invariant failure.
    pub fn finish_cancelled_turn_with_effect_after_await(
        &self,
        registry: &mut ListenerRuntime,
        _claim: CancellationClaim,
        stop_reason: StopReason,
        effect: impl FnOnce() -> Result<(), crate::RuntimeError>,
    ) -> Result<LeaseEffect<CancellationReceipt>, crate::RuntimeError> {
        if let Err(reason) = self.check_owner(registry) {
            return Ok(LeaseEffect::Suppressed(reason));
        }
        let owner =
            registry
                .lifecycle_mut(&self.handle)
                .map_err(|_| crate::RuntimeError::NotFound {
                    context: "checked cancelled turn runtime".into(),
                })?;
        if owner.projection().state() != lotta_domain::TurnStateKind::Cancelling {
            return Ok(LeaseEffect::Suppressed(
                SuppressionReason::CancellationDenied,
            ));
        }
        effect()?;
        owner
            .finish_turn(&self.lease, stop_reason)
            .map_err(|_| crate::RuntimeError::Conflict {
                context: "checked cancelled turn lease".into(),
            })?;
        Ok(LeaseEffect::Applied(CancellationReceipt::new()))
    }

    /// Checks once, applies a terminal effect, then releases the exact owner synchronously.
    ///
    /// The exclusive registry borrow makes cancellation during `effect` an in-flight
    /// invalidation: it cannot install N+1, and the already-linearized N operation completes.
    /// If the effect fails ownership is retained for recovery.
    ///
    /// # Errors
    /// Returns the effect failure or a typed lifecycle invariant failure.
    pub fn finish_turn_with_effect_after_await<T>(
        &self,
        registry: &mut ListenerRuntime,
        stop_reason: StopReason,
        effect: impl FnOnce() -> Result<T, crate::RuntimeError>,
    ) -> Result<LeaseEffect<T>, crate::RuntimeError> {
        if let Err(reason) = self.check(registry) {
            return Ok(LeaseEffect::Suppressed(reason));
        }
        let value = effect()?;
        let owner =
            registry
                .lifecycle_mut(&self.handle)
                .map_err(|_| crate::RuntimeError::NotFound {
                    context: "checked turn runtime".into(),
                })?;
        owner
            .finish_turn(&self.lease, stop_reason)
            .map_err(|_| crate::RuntimeError::Conflict {
                context: "checked turn lease".into(),
            })?;
        Ok(LeaseEffect::Applied(value))
    }

    fn check(&self, registry: &ListenerRuntime) -> Result<(), SuppressionReason> {
        self.check_owner(registry)?;
        if self.cancellation.is_cancelled()
            && self.policy == CancellationPolicy::SuppressWhenCancelled
        {
            return Err(SuppressionReason::CancellationDenied);
        }
        Ok(())
    }

    fn check_owner(&self, registry: &ListenerRuntime) -> Result<(), SuppressionReason> {
        if !registry.is_active() {
            return Err(SuppressionReason::ListenerInactive);
        }
        let Some(owner) = registry.lifecycle(&self.handle) else {
            return Err(SuppressionReason::RuntimeMissing);
        };
        if !owner.is_current(&self.lease) {
            return Err(SuppressionReason::StaleLease);
        }
        Ok(())
    }
}

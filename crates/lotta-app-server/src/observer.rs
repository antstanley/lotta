//! Read-only observation of actual Runtime broadcast dispatches.

use lotta_domain::RuntimeScope;

use crate::ws::{EventDeliveryBatch, RuntimeEvent};

/// Immutable observation of one actual logical-event dispatch batch.
#[derive(Clone, Debug)]
pub struct RuntimeBroadcastObservation {
    /// Monotonic batch ordinal for this listener, starting at one.
    pub batch_ordinal: u64,
    /// Runtime subscription scope used by routing.
    pub scope: RuntimeScope,
    /// Actual logical event before per-delivery stamping.
    pub event: RuntimeEvent,
    /// Actual stamped deliveries in stable dispatch order, possibly empty.
    pub deliveries: EventDeliveryBatch,
}

/// Infallible read-only callback invoked after every successful Runtime event dispatch.
///
/// Implementations must remain bounded and retain their own failure state. Callback behavior cannot
/// change routing because it has no return value and runs only after all deliveries are queued.
pub trait RuntimeBroadcastObserver: Send + Sync {
    /// Observes one immutable actual dispatch batch.
    fn observe(&self, observation: RuntimeBroadcastObservation);
}

/// Inert observer used by all existing listener entry points.
#[derive(Default)]
pub struct InertRuntimeBroadcastObserver;

impl RuntimeBroadcastObserver for InertRuntimeBroadcastObserver {
    fn observe(&self, _: RuntimeBroadcastObservation) {}
}

pub(crate) fn observation(
    batch_ordinal: u64,
    scope: RuntimeScope,
    event: RuntimeEvent,
    deliveries: &EventDeliveryBatch,
) -> RuntimeBroadcastObservation {
    RuntimeBroadcastObservation {
        batch_ordinal,
        scope,
        event,
        deliveries: deliveries.clone(),
    }
}

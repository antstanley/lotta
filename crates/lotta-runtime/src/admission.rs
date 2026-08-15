use crate::{ListenerRuntime, QueueMutation, QueueMutationEvent, RuntimeError, RuntimeHandle};
use lotta_domain::{
    InputDisposition, QueueDropReason, QueueItem, QueueItemKind, TurnLease, TurnStateKind,
};

/// Admission routing classification established by the inbound adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdmissionRoute {
    /// Normal input follows idle-start or occupied-queue behavior.
    Ordinary,
    /// A continuation requires the captured current lease.
    Continuation(TurnLease),
    /// A control response requires the captured current lease.
    Control(TurnLease),
}

/// Fully validated input submitted to one serialized admission chain.
#[derive(Clone, Debug, PartialEq)]
pub struct AdmissionRequest {
    /// Already-complete queue item with injected ID and timestamp.
    pub item: QueueItem,
    /// Route classification established by the adapter.
    pub route: AdmissionRoute,
}

/// Typed result of one serialized input admission.
#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub enum AdmissionOutcome {
    /// Existing disposition replayed with no side effect.
    Duplicate(InputDisposition),
    /// Idle ordinary input should start a new owner operation.
    Start(QueueItem),
    /// Active-lease continuation may proceed directly.
    Continue(QueueItem),
    /// Active-lease control response may proceed directly.
    Control(QueueItem),
    /// Occupied ordinary input was retained in the queue.
    Queued(QueueMutation),
    /// Input was observably rejected or dropped.
    Rejected {
        /// Authoritative drop mutation.
        mutation: QueueMutation,
        /// Exact internal drop reason.
        reason: QueueDropReason,
    },
}

impl AdmissionOutcome {
    /// Returns the canonical input disposition.
    #[must_use]
    pub const fn disposition(&self) -> InputDisposition {
        match self {
            Self::Duplicate(disposition) => *disposition,
            Self::Start(_) | Self::Continue(_) | Self::Control(_) => InputDisposition::Started,
            Self::Queued(_) => InputDisposition::Queued,
            Self::Rejected { .. } => InputDisposition::Rejected,
        }
    }
}

impl ListenerRuntime {
    /// Atomically admits against one entry's history, lifecycle, and queue.
    ///
    /// # Errors
    /// Returns an error for stale handles, duplicate queue IDs, or revision exhaustion.
    pub fn admit(
        &mut self,
        handle: &RuntimeHandle,
        request: AdmissionRequest,
    ) -> Result<AdmissionOutcome, RuntimeError> {
        let entry = self.current_entry_mut(handle)?;
        if let Some(prior) = entry
            .admission_history
            .prior(&request.item.client_message_id)
        {
            return Ok(AdmissionOutcome::Duplicate(prior));
        }
        let client_message_id = request.item.client_message_id.clone();
        let outcome = match request.route {
            AdmissionRoute::Ordinary => {
                if entry.owner.projection().state() == TurnStateKind::Idle
                    && entry.queue.is_empty()
                    && request.item.kind == QueueItemKind::Message
                {
                    AdmissionOutcome::Start(request.item)
                } else {
                    classify_queue(entry.queue.enqueue(request.item)?)
                }
            }
            AdmissionRoute::Continuation(lease) => {
                if entry.owner.is_current(&lease) {
                    AdmissionOutcome::Continue(request.item)
                } else {
                    stale_route(&mut entry.queue, request.item)?
                }
            }
            AdmissionRoute::Control(lease) => {
                if entry.owner.is_current(&lease) {
                    AdmissionOutcome::Control(request.item)
                } else {
                    stale_route(&mut entry.queue, request.item)?
                }
            }
        };
        let _recorded = entry
            .admission_history
            .admit(&client_message_id, outcome.disposition());
        Ok(outcome)
    }
}

fn classify_queue(mutation: QueueMutation) -> AdmissionOutcome {
    if matches!(mutation.event(), QueueMutationEvent::Dropped(_, _)) {
        AdmissionOutcome::Rejected {
            mutation,
            reason: QueueDropReason::BufferLimit,
        }
    } else {
        AdmissionOutcome::Queued(mutation)
    }
}

fn stale_route(
    queue: &mut crate::ConversationQueue,
    item: QueueItem,
) -> Result<AdmissionOutcome, RuntimeError> {
    let mutation = queue.reject(item, QueueDropReason::StaleGeneration)?;
    Ok(AdmissionOutcome::Rejected {
        mutation,
        reason: QueueDropReason::StaleGeneration,
    })
}

#[cfg(test)]
mod duplicate;
#[cfg(test)]
mod flow;
#[cfg(test)]
mod sources;
#[cfg(test)]
mod test_support;

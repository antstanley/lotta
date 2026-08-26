use crate::{ListenerRuntime, QueueMutation, QueueMutationEvent, RuntimeError, RuntimeHandle};
mod control;
pub use control::admit_control_snapshot;

use lotta_domain::{
    InputDisposition, NonEmptyString, QueueDropReason, QueueItem, QueueItemKind, TurnLease,
    TurnStateKind,
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
    /// Retains an item already admitted by an active admission queue.
    ///
    /// This preserves the canonical queue's ordering and bounds without recording
    /// admission history a second time or classifying the item as a new start.
    ///
    /// # Errors
    /// Returns an error for stale handles, duplicate queue IDs, or revision exhaustion.
    pub fn enqueue_retained(
        &mut self,
        handle: &RuntimeHandle,
        item: QueueItem,
    ) -> Result<QueueMutation, RuntimeError> {
        self.queue(handle)
            .ok_or_else(stale_handle)?
            .lock()
            .map_err(|_| queue_lock_error())?
            .enqueue(item)
    }

    /// Atomically admits against one entry's history, lifecycle, and queue.
    ///
    /// # Errors
    /// Returns an error for stale handles, duplicate queue IDs, or revision exhaustion.
    pub fn admit(
        &mut self,
        handle: &RuntimeHandle,
        request: AdmissionRequest,
    ) -> Result<AdmissionOutcome, RuntimeError> {
        let observer = self.observer().clone();
        let key = handle.key().clone();
        let connection_id = static_id("runtime-local", "runtime connection id")?;
        let entry = self.current_entry_mut(handle)?;
        if let Some(prior) = entry
            .admission_history
            .prior(&request.item.client_message_id)
        {
            return Ok(AdmissionOutcome::Duplicate(prior));
        }
        let mut queue = entry.queue.lock().map_err(|_| queue_lock_error())?;
        let client_message_id = request.item.client_message_id.clone();
        let outcome = admit_route(&entry.owner, &mut queue, request)?;
        let queue_depth = queue.len();
        let active_turns = u64::from(entry.owner.projection().state() != TurnStateKind::Idle);
        drop(queue);
        let _recorded = entry
            .admission_history
            .admit(&client_message_id, outcome.disposition());
        observer.admission(
            &key,
            &connection_id,
            None,
            entry.owner.projection().active_run_ids().first(),
            queue_depth,
            active_turns,
        );
        Ok(outcome)
    }
}

fn admit_route(
    owner: &crate::LifecycleOwner,
    queue: &mut crate::ConversationQueue,
    request: AdmissionRequest,
) -> Result<AdmissionOutcome, RuntimeError> {
    match request.route {
        AdmissionRoute::Ordinary
            if owner.projection().state() == TurnStateKind::Idle
                && queue.is_empty()
                && request.item.kind == QueueItemKind::Message =>
        {
            Ok(AdmissionOutcome::Start(request.item))
        }
        AdmissionRoute::Ordinary => queue.enqueue(request.item).map(classify_queue),
        AdmissionRoute::Continuation(lease) if owner.is_current(&lease) => {
            Ok(AdmissionOutcome::Continue(request.item))
        }
        AdmissionRoute::Control(lease) if owner.is_current(&lease) => {
            Ok(AdmissionOutcome::Control(request.item))
        }
        AdmissionRoute::Continuation(_) | AdmissionRoute::Control(_) => {
            stale_route(queue, request.item)
        }
    }
}

fn stale_handle() -> RuntimeError {
    RuntimeError::InvalidData {
        context: "stale runtime handle".into(),
    }
}

fn queue_lock_error() -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "queue_lock",
        context: "conversation queue".into(),
    }
}

fn static_id(value: &'static str, context: &'static str) -> Result<NonEmptyString, RuntimeError> {
    NonEmptyString::new(value).map_err(|_| RuntimeError::InvalidData {
        context: context.to_owned(),
    })
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

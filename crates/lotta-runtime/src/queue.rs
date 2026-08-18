use crate::{QueueMutation, QueueMutationEvent, QueueSnapshot, RuntimeError};
use lotta_domain::bounds::{QUEUE_ITEMS_HARD_MAX, QUEUE_ITEMS_SOFT_MAX, QUEUE_PUMP_BATCH_MAX};
use lotta_domain::{
    NonEmptyString, QueueDropReason, QueueItem, QueueItemKind, QueueRemovalDisposition,
    TurnStateKind,
};
use std::collections::VecDeque;

/// Caller scheduling instruction after a bounded queue pump.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PumpDirective {
    /// No immediately eligible item remains.
    Complete,
    /// The caller must yield before requesting another batch.
    YieldBeforeNextBatch,
}

/// Result of selecting a bounded processing batch.
#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub struct PumpMutation {
    mutation: QueueMutation,
    directive: PumpDirective,
}

impl PumpMutation {
    /// Returns the queue mutation containing the selected FIFO batch.
    pub const fn mutation(&self) -> &QueueMutation {
        &self.mutation
    }
    /// Returns the required caller scheduling action.
    #[must_use]
    pub const fn directive(&self) -> PumpDirective {
        self.directive
    }
}

/// Bounded FIFO pending input owned by one conversation runtime.
#[derive(Debug, Default)]
pub struct ConversationQueue {
    items: VecDeque<QueueItem>,
    revision: u64,
}

impl ConversationQueue {
    /// Returns retained item count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }
    /// Returns whether no items are retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// Returns retained items in FIFO order.
    #[must_use]
    pub fn items(&self) -> impl ExactSizeIterator<Item = &QueueItem> {
        self.items.iter()
    }

    /// Returns the FIFO head without mutating the queue.
    #[must_use]
    pub fn peek(&self) -> Option<&QueueItem> {
        self.items.front()
    }

    /// Produces a drop event and snapshot without retaining an input.
    pub(crate) fn reject(
        &mut self,
        item: QueueItem,
        reason: QueueDropReason,
    ) -> Result<QueueMutation, RuntimeError> {
        let revision = self.next_revision()?;
        Ok(self.finish(revision, QueueMutationEvent::Dropped(item, reason)))
    }

    /// Retains, replaces, or rejects one input according to hard-before-soft ordering.
    ///
    /// # Errors
    /// Returns a stable error for duplicate IDs or revision exhaustion.
    pub fn enqueue(&mut self, item: QueueItem) -> Result<QueueMutation, RuntimeError> {
        let revision = self.next_revision()?;
        if self.items.len() >= QUEUE_ITEMS_HARD_MAX.value {
            return Ok(self.finish(
                revision,
                QueueMutationEvent::Dropped(item, QueueDropReason::BufferLimit),
            ));
        }
        self.validate_unique(&item)?;
        let event = if self.items.len() >= QUEUE_ITEMS_SOFT_MAX.value && coalescable(item.kind) {
            self.enqueue_soft(item)?
        } else {
            self.items.push_back(item.clone());
            QueueMutationEvent::Enqueued(item)
        };
        Ok(self.finish(revision, event))
    }

    /// Removes the head for processing with wire disposition `dequeued`.
    ///
    /// # Errors
    /// Returns a stable error for revision exhaustion or storage invariant failure.
    pub fn dequeue(&mut self) -> Result<Option<QueueMutation>, RuntimeError> {
        let Some(item) = self.items.front().cloned() else {
            return Ok(None);
        };
        self.remove_known(&item, QueueRemovalDisposition::Dequeued)
            .map(Some)
    }

    /// Removes a named item with wire disposition `dequeued`.
    ///
    /// # Errors
    /// Returns a stable error for revision exhaustion or storage invariant failure.
    pub fn remove(&mut self, id: &NonEmptyString) -> Result<Option<QueueMutation>, RuntimeError> {
        self.remove_id(id, QueueRemovalDisposition::Dequeued)
    }

    /// Cancels a named item with wire disposition `cancelled`.
    ///
    /// # Errors
    /// Returns a stable error for revision exhaustion or storage invariant failure.
    pub fn cancel(&mut self, id: &NonEmptyString) -> Result<Option<QueueMutation>, RuntimeError> {
        self.remove_id(id, QueueRemovalDisposition::Cancelled)
    }

    /// Drops a named stale-generation item, returning no mutation when absent.
    ///
    /// # Errors
    /// Returns a stable error for revision exhaustion or storage invariant failure.
    pub fn drop_stale(
        &mut self,
        id: &NonEmptyString,
    ) -> Result<Option<QueueMutation>, RuntimeError> {
        let Some(index) = self.position(id) else {
            return Ok(None);
        };
        let revision = self.next_revision()?;
        let Some(item) = self.items.remove(index) else {
            return Err(queue_invariant());
        };
        Ok(Some(self.finish(
            revision,
            QueueMutationEvent::Dropped(item, QueueDropReason::StaleGeneration),
        )))
    }

    /// Restores a previously removed item to the FIFO front.
    ///
    /// This rollback operation preserves the hard bound and rejects duplicate IDs.
    ///
    /// # Errors
    /// Returns a stable error when the queue is full, the ID is duplicated, or revision is exhausted.
    pub fn requeue_front(&mut self, item: QueueItem) -> Result<QueueMutation, RuntimeError> {
        if self.items.len() >= QUEUE_ITEMS_HARD_MAX.value {
            return Err(RuntimeError::LimitExceeded {
                context: QUEUE_ITEMS_HARD_MAX.name.into(),
            });
        }
        self.validate_unique(&item)?;
        let revision = self.next_revision()?;
        self.items.push_front(item.clone());
        Ok(self.finish(revision, QueueMutationEvent::Enqueued(item)))
    }

    /// Infallibly restores the exact item just removed by [`Self::pump_one`].
    ///
    /// Callers must invoke this before any intervening queue mutation.
    pub(crate) fn rollback_pump_one(&mut self, item: QueueItem) {
        debug_assert!(self.items.len() < QUEUE_ITEMS_HARD_MAX.value);
        debug_assert!(self.position(&item.id).is_none());
        self.items.push_front(item);
    }

    /// Selects exactly one eligible FIFO item from an idle lifecycle snapshot.
    ///
    /// # Errors
    /// Returns a stable error when snapshot revision is exhausted.
    pub fn pump_one(&mut self, state: TurnStateKind) -> Result<Option<QueueItem>, RuntimeError> {
        if state != TurnStateKind::Idle || self.items.is_empty() {
            return Ok(None);
        }
        let revision = self.next_revision()?;
        let item = self.items.pop_front().ok_or_else(queue_invariant)?;
        self.revision = revision;
        Ok(Some(item))
    }

    /// Selects one eligible FIFO batch only from an idle lifecycle snapshot.
    ///
    /// # Errors
    /// Returns a stable error when snapshot revision is exhausted.
    pub fn pump(&mut self, state: TurnStateKind) -> Result<Option<PumpMutation>, RuntimeError> {
        if state != TurnStateKind::Idle || self.items.is_empty() {
            return Ok(None);
        }
        let count = self.pump_count();
        let revision = self.next_revision()?;
        let batch: Vec<_> = self.items.drain(..count).collect();
        let more = count == QUEUE_PUMP_BATCH_MAX && !self.items.is_empty();
        let mutation = self.finish(revision, QueueMutationEvent::Pumped(batch));
        let directive = if more {
            PumpDirective::YieldBeforeNextBatch
        } else {
            PumpDirective::Complete
        };
        Ok(Some(PumpMutation {
            mutation,
            directive,
        }))
    }

    fn enqueue_soft(&mut self, item: QueueItem) -> Result<QueueMutationEvent, RuntimeError> {
        let target = self
            .items
            .iter()
            .position(|stored| coalescable(stored.kind));
        let Some(index) = target else {
            self.items.push_back(item.clone());
            return Ok(QueueMutationEvent::Enqueued(item));
        };
        let Some(dropped) = self.items.remove(index) else {
            return Err(queue_invariant());
        };
        self.items.push_back(item.clone());
        Ok(QueueMutationEvent::Replaced {
            dropped,
            enqueued: item,
            reason: QueueDropReason::BufferLimit,
        })
    }

    fn remove_id(
        &mut self,
        id: &NonEmptyString,
        disposition: QueueRemovalDisposition,
    ) -> Result<Option<QueueMutation>, RuntimeError> {
        let Some(index) = self.position(id) else {
            return Ok(None);
        };
        let Some(item) = self.items.get(index).cloned() else {
            return Err(queue_invariant());
        };
        self.remove_known(&item, disposition).map(Some)
    }

    fn remove_known(
        &mut self,
        item: &QueueItem,
        disposition: QueueRemovalDisposition,
    ) -> Result<QueueMutation, RuntimeError> {
        let revision = self.next_revision()?;
        let Some(index) = self.position(&item.id) else {
            return Err(queue_invariant());
        };
        let Some(removed) = self.items.remove(index) else {
            return Err(queue_invariant());
        };
        Ok(self.finish(revision, QueueMutationEvent::Removed(removed, disposition)))
    }

    fn validate_unique(&self, item: &QueueItem) -> Result<(), RuntimeError> {
        if self.position(&item.id).is_some() {
            Err(RuntimeError::InvalidData {
                context: "queue_item_id_duplicate".into(),
            })
        } else {
            Ok(())
        }
    }

    fn position(&self, id: &NonEmptyString) -> Option<usize> {
        self.items.iter().position(|item| item.id == *id)
    }

    fn next_revision(&self) -> Result<u64, RuntimeError> {
        self.revision
            .checked_add(1)
            .ok_or(RuntimeError::InvalidData {
                context: "queue_snapshot_revision_exhausted".into(),
            })
    }

    fn finish(&mut self, revision: u64, event: QueueMutationEvent) -> QueueMutation {
        self.revision = revision;
        let snapshot = QueueSnapshot::new(revision, self.items.iter().cloned().collect());
        QueueMutation::new(event, snapshot)
    }

    fn pump_count(&self) -> usize {
        if self
            .items
            .front()
            .is_some_and(|item| !coalescable(item.kind))
        {
            return 1;
        }
        self.items
            .iter()
            .take_while(|item| coalescable(item.kind))
            .take(QUEUE_PUMP_BATCH_MAX)
            .count()
    }
}

const fn coalescable(kind: QueueItemKind) -> bool {
    matches!(
        kind,
        QueueItemKind::Message
            | QueueItemKind::TaskNotification
            | QueueItemKind::CronPrompt
            | QueueItemKind::ModContinue
    )
}

fn queue_invariant() -> RuntimeError {
    RuntimeError::InvalidData {
        context: "queue_storage_invariant".into(),
    }
}

#[cfg(test)]
mod hard_tier;
#[cfg(test)]
mod pump;
#[cfg(test)]
mod soft_tier;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod wire_transitions;

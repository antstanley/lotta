use lotta_domain::{QueueDropReason, QueueItem, QueueRemovalDisposition};

/// Authoritative full queue contents after one observable mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueSnapshot {
    revision: u64,
    items: Vec<QueueItem>,
}

impl QueueSnapshot {
    /// Returns the monotonic snapshot revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns all retained queue items in FIFO order.
    #[must_use]
    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub(crate) const fn new(revision: u64, items: Vec<QueueItem>) -> Self {
        Self { revision, items }
    }
}

/// Typed cause represented by a queue mutation snapshot.
#[derive(Clone, Debug, PartialEq)]
pub enum QueueMutationEvent {
    /// One item was retained at the tail.
    Enqueued(QueueItem),
    /// One eligible item was replaced at the soft tier.
    Replaced {
        /// Removed oldest eligible item.
        dropped: QueueItem,
        /// Newly retained tail item.
        enqueued: QueueItem,
        /// Capacity reason for replacement.
        reason: QueueDropReason,
    },
    /// One input was not retained or one stored input was dropped.
    Dropped(QueueItem, QueueDropReason),
    /// One stored item left with a public wire disposition.
    Removed(QueueItem, QueueRemovalDisposition),
    /// A bounded FIFO batch was selected for processing.
    Pumped(Vec<QueueItem>),
}

/// Exactly one event paired with exactly one authoritative snapshot.
#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub struct QueueMutation {
    event: QueueMutationEvent,
    snapshot: QueueSnapshot,
}

impl QueueMutation {
    /// Returns the typed mutation event.
    #[must_use]
    pub const fn event(&self) -> &QueueMutationEvent {
        &self.event
    }

    /// Returns the authoritative post-mutation snapshot.
    #[must_use]
    pub const fn snapshot(&self) -> &QueueSnapshot {
        &self.snapshot
    }

    pub(crate) const fn new(event: QueueMutationEvent, snapshot: QueueSnapshot) -> Self {
        Self { event, snapshot }
    }
}

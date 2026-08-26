//! Bounded listener-owned runtime registry.

use crate::observe::RuntimeObserver;
use crate::{ConversationQueue, LifecycleOwner, PumpMutation, QueueMutation, RuntimeError};
use lotta_domain::bounds::RUNTIMES_MAX;
use lotta_domain::{
    AdmissionHistory, AgentId, ConversationId, NonEmptyString, RuntimeScope, TurnStateKind,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

/// Immutable identity of one conversation runtime.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RuntimeKey {
    agent_id: AgentId,
    conversation_id: ConversationId,
}

impl RuntimeKey {
    /// Returns the owning agent identifier.
    #[must_use]
    pub const fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
    /// Returns the conversation identifier.
    #[must_use]
    pub const fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }
}

impl From<&RuntimeScope> for RuntimeKey {
    fn from(scope: &RuntimeScope) -> Self {
        Self {
            agent_id: scope.agent_id.clone(),
            conversation_id: scope.conversation_id.clone(),
        }
    }
}

/// Opaque capability identifying one exact registry generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeHandle {
    key: RuntimeKey,
    generation: u64,
}

impl RuntimeHandle {
    /// Returns the exact runtime owner key.
    #[must_use]
    pub const fn key(&self) -> &RuntimeKey {
        &self.key
    }
}

/// Complete auxiliary snapshot controlling runtime residency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeResidency {
    pending_approval_count: usize,
    interrupted_result_present: bool,
    sandbox_subscription_count: usize,
}

impl RuntimeResidency {
    /// Creates the complete three-term auxiliary residency snapshot.
    #[must_use]
    pub const fn new(
        pending_approval_count: usize,
        interrupted_result_present: bool,
        sandbox_subscription_count: usize,
    ) -> Self {
        Self {
            pending_approval_count,
            interrupted_result_present,
            sandbox_subscription_count,
        }
    }

    /// ORs live lifecycle and queue state with the exact three auxiliary terms.
    #[must_use]
    pub fn requires_residency(self, lifecycle: TurnStateKind, queue_len: usize) -> bool {
        lifecycle != TurnStateKind::Idle
            || queue_len > 0
            || self.pending_approval_count > 0
            || self.interrupted_result_present
            || self.sandbox_subscription_count > 0
    }
}

pub(crate) struct RuntimeEntry {
    pub(crate) generation: u64,
    pub(crate) owner: LifecycleOwner,
    pub(crate) queue: Arc<Mutex<ConversationQueue>>,
    pub(crate) admission_history: AdmissionHistory,
    pub(crate) residency: RuntimeResidency,
}

/// Result of publishing a complete residency snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResidencyUpdate {
    /// The runtime remains registered.
    Retained,
    /// The runtime was synchronously removed because it became quiescent.
    Evicted,
}

/// Single owner of all bounded conversation runtimes for one listener.
pub struct ListenerRuntime {
    entries: HashMap<RuntimeKey, RuntimeEntry>,
    next_generation: u64,
    active: bool,
    observer: RuntimeObserver,
}

impl Default for ListenerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ListenerRuntime {
    /// Creates an active empty listener runtime registry.
    #[must_use]
    pub fn new() -> Self {
        Self::with_observer(RuntimeObserver::default())
    }

    /// Creates an active registry owning one explicitly injected observer instance.
    #[must_use]
    pub fn with_observer(observer: RuntimeObserver) -> Self {
        Self {
            entries: HashMap::new(),
            next_generation: 1,
            active: true,
            observer,
        }
    }

    /// Returns the shared observer owned by this listener instance.
    #[must_use]
    pub const fn observer(&self) -> &RuntimeObserver {
        &self.observer
    }

    fn refresh_queue_gauge(&self, handle: &RuntimeHandle) {
        let depth = self
            .queue(handle)
            .and_then(|queue| queue.lock().ok().map(|queue| queue.len() as u64))
            .unwrap_or(0);
        self.observer
            .set_gauge(crate::observe::metrics::MetricFamily::QueueDepth, depth);
    }

    /// Returns whether this listener accepts post-await effects.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Permanently deactivates post-await effects.
    pub fn deactivate(&mut self) {
        self.active = false;
    }

    /// Returns the number of resident runtimes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no runtimes are resident.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns whether an exact key is resident.
    #[must_use]
    pub fn contains(&self, key: &RuntimeKey) -> bool {
        self.entries.contains_key(key)
    }

    /// Returns the current handle for an exact key, if resident.
    #[must_use]
    pub fn lookup(&self, key: &RuntimeKey) -> Option<RuntimeHandle> {
        self.entries.get(key).map(|entry| RuntimeHandle {
            key: key.clone(),
            generation: entry.generation,
        })
    }

    /// Returns auxiliary residency for a current handle.
    #[must_use]
    pub fn residency(&self, handle: &RuntimeHandle) -> Option<RuntimeResidency> {
        self.current_entry(handle).map(|entry| entry.residency)
    }

    /// Returns the exact handle's live lifecycle owner.
    #[must_use]
    pub fn lifecycle(&self, handle: &RuntimeHandle) -> Option<&LifecycleOwner> {
        self.current_entry(handle).map(|entry| &entry.owner)
    }

    /// Returns the exact handle's immutable pending queue.
    #[must_use]
    pub fn queue(&self, handle: &RuntimeHandle) -> Option<Arc<Mutex<ConversationQueue>>> {
        self.current_entry(handle)
            .map(|entry| Arc::clone(&entry.queue))
    }

    /// Dequeues the pending queue head for an exact runtime generation.
    ///
    /// # Errors
    /// Returns an error for a missing/stale handle or queue mutation failure.
    pub fn dequeue_queue(
        &mut self,
        handle: &RuntimeHandle,
    ) -> Result<Option<QueueMutation>, RuntimeError> {
        let result = self.queue_lock(handle)?.dequeue()?;
        self.refresh_queue_gauge(handle);
        Ok(result)
    }

    /// Removes a queued item with the wire `dequeued` disposition.
    ///
    /// # Errors
    /// Returns an error for a missing/stale handle or queue mutation failure.
    pub fn remove_queued(
        &mut self,
        handle: &RuntimeHandle,
        id: &NonEmptyString,
    ) -> Result<Option<QueueMutation>, RuntimeError> {
        let result = self.queue_lock(handle)?.remove(id)?;
        self.refresh_queue_gauge(handle);
        Ok(result)
    }

    /// Cancels a queued item with the wire `cancelled` disposition.
    ///
    /// # Errors
    /// Returns an error for a missing/stale handle or queue mutation failure.
    pub fn cancel_queued(
        &mut self,
        handle: &RuntimeHandle,
        id: &NonEmptyString,
    ) -> Result<Option<QueueMutation>, RuntimeError> {
        let result = self.queue_lock(handle)?.cancel(id)?;
        self.refresh_queue_gauge(handle);
        Ok(result)
    }

    /// Drops a queued item with the internal stale-generation reason.
    ///
    /// # Errors
    /// Returns an error for a missing/stale handle or queue mutation failure.
    pub fn drop_stale_queued(
        &mut self,
        handle: &RuntimeHandle,
        id: &NonEmptyString,
    ) -> Result<Option<QueueMutation>, RuntimeError> {
        self.queue_lock(handle)?.drop_stale(id)
    }

    /// Restores one previously removed item to an exact runtime's queue front.
    ///
    /// # Errors
    /// Returns an error for a missing/stale handle or queue mutation failure.
    pub fn requeue_front(
        &mut self,
        handle: &RuntimeHandle,
        item: lotta_domain::QueueItem,
    ) -> Result<QueueMutation, RuntimeError> {
        self.queue_lock(handle)?.requeue_front(item)
    }

    /// Returns an exact runtime's FIFO queue head without mutation.
    #[must_use]
    pub fn peek_queue(&self, handle: &RuntimeHandle) -> Option<lotta_domain::QueueItem> {
        self.current_entry(handle).and_then(|entry| {
            entry
                .queue
                .lock()
                .ok()
                .and_then(|queue| queue.peek().cloned())
        })
    }

    /// Pumps exactly one item from an exact runtime's queue using its live lifecycle state.
    ///
    /// # Errors
    /// Returns an error for a missing/stale handle or queue mutation failure.
    pub fn pump_one_queue(
        &mut self,
        handle: &RuntimeHandle,
    ) -> Result<Option<lotta_domain::QueueItem>, RuntimeError> {
        let entry = self.current_entry(handle).ok_or_else(stale_handle)?;
        let state = entry.owner.projection().state();
        let mut queue = entry.queue.lock().map_err(|_| queue_lock_error())?;
        let result = queue.pump_one(state)?;
        let depth = queue.len() as u64;
        self.observer
            .set_gauge(crate::observe::metrics::MetricFamily::QueueDepth, depth);
        Ok(result)
    }

    /// Infallibly restores the exact item most recently removed by [`Self::pump_one_queue`].
    ///
    /// # Panics
    /// Panics if the caller does not provide the same live runtime generation used to pump.
    pub fn rollback_pump_one(&mut self, handle: &RuntimeHandle, item: lotta_domain::QueueItem) {
        if let Ok(mut queue) = self.queue_lock(handle) {
            queue.rollback_pump_one(item);
        }
    }

    /// Pumps an exact runtime's queue using its live lifecycle state.
    ///
    /// # Errors
    /// Returns an error for a missing/stale handle or queue mutation failure.
    pub fn pump_queue(
        &mut self,
        handle: &RuntimeHandle,
    ) -> Result<Option<PumpMutation>, RuntimeError> {
        let entry = self.current_entry(handle).ok_or_else(stale_handle)?;
        let state = entry.owner.projection().state();
        entry
            .queue
            .lock()
            .map_err(|_| queue_lock_error())?
            .pump(state)
    }

    fn queue_lock(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<std::sync::MutexGuard<'_, ConversationQueue>, RuntimeError> {
        self.current_entry(handle)
            .ok_or_else(stale_handle)?
            .queue
            .lock()
            .map_err(|_| queue_lock_error())
    }

    /// Returns the exact handle's canonical admission history.
    #[must_use]
    pub fn current_admission_history(&self, handle: &RuntimeHandle) -> Option<&AdmissionHistory> {
        self.current_entry(handle)
            .map(|entry| &entry.admission_history)
    }

    /// Returns the exact handle's mutable canonical admission history.
    ///
    /// # Errors
    /// Returns [`RuntimeError::NotFound`] for a missing or stale generation.
    pub fn admission_history_mut(
        &mut self,
        handle: &RuntimeHandle,
    ) -> Result<&mut AdmissionHistory, RuntimeError> {
        self.current_entry_mut(handle)
            .map(|entry| &mut entry.admission_history)
    }

    /// Returns the exact handle's mutable lifecycle owner.
    ///
    /// # Errors
    /// Returns [`RuntimeError::NotFound`] for a missing or stale generation.
    pub fn lifecycle_mut(
        &mut self,
        handle: &RuntimeHandle,
    ) -> Result<&mut LifecycleOwner, RuntimeError> {
        let Some(entry) = self.entries.get_mut(&handle.key) else {
            return Err(stale_handle());
        };
        if entry.generation != handle.generation {
            return Err(stale_handle());
        }
        Ok(&mut entry.owner)
    }

    /// Gets or creates exactly one runtime owner per scope.
    ///
    /// `owner_id` must be injected from [`crate::ports::IdGenerator`] and unique among live
    /// lifecycle owners. It is ignored when the scope already exists.
    ///
    /// # Errors
    /// Returns a limit error at capacity or invalid data on generation exhaustion.
    pub fn get_or_create(
        &mut self,
        scope: &RuntimeScope,
        owner_id: Uuid,
    ) -> Result<RuntimeHandle, RuntimeError> {
        let key = RuntimeKey::from(scope);
        if let Some(existing) = self.lookup(&key) {
            return Ok(existing);
        }
        if self.entries.len() >= RUNTIMES_MAX.value {
            return Err(RuntimeError::LimitExceeded {
                context: RUNTIMES_MAX.name.into(),
            });
        }
        let generation = self.next_generation;
        self.next_generation = generation.checked_add(1).ok_or_else(generation_exhausted)?;
        let entry = RuntimeEntry {
            generation,
            owner: LifecycleOwner::new(scope.clone(), owner_id),
            queue: Arc::new(Mutex::new(ConversationQueue::default())),
            admission_history: AdmissionHistory::default(),
            residency: RuntimeResidency::new(0, false, 0),
        };
        self.entries.insert(key.clone(), entry);
        Ok(RuntimeHandle { key, generation })
    }

    /// Removes an exact newly installed runtime generation.
    ///
    /// Used only to roll back failed initialization before the handle is published.
    #[must_use]
    pub fn rollback_create(&mut self, handle: &RuntimeHandle) -> bool {
        if self.current_entry(handle).is_none() {
            return false;
        }
        self.entries.remove(&handle.key).is_some()
    }

    /// Publishes auxiliary state and consults live lifecycle before synchronous eviction.
    ///
    /// # Errors
    /// Returns [`RuntimeError::NotFound`] for a missing or stale generation.
    pub fn set_residency(
        &mut self,
        handle: &RuntimeHandle,
        snapshot: RuntimeResidency,
    ) -> Result<ResidencyUpdate, RuntimeError> {
        let entry = self.current_entry(handle).ok_or_else(stale_handle)?;
        let lifecycle = entry.owner.projection().state();
        let queue_len = entry.queue.lock().map_err(|_| queue_lock_error())?.len();
        if snapshot.requires_residency(lifecycle, queue_len) {
            self.lifecycle_mut(handle)?;
            if let Some(entry) = self.entries.get_mut(&handle.key) {
                entry.residency = snapshot;
            }
            return Ok(ResidencyUpdate::Retained);
        }
        self.entries.remove(&handle.key);
        Ok(ResidencyUpdate::Evicted)
    }

    pub(crate) fn current_entry(&self, handle: &RuntimeHandle) -> Option<&RuntimeEntry> {
        self.entries
            .get(&handle.key)
            .filter(|entry| entry.generation == handle.generation)
    }

    pub(crate) fn current_entry_mut(
        &mut self,
        handle: &RuntimeHandle,
    ) -> Result<&mut RuntimeEntry, RuntimeError> {
        let Some(entry) = self.entries.get_mut(&handle.key) else {
            return Err(stale_handle());
        };
        if entry.generation != handle.generation {
            return Err(stale_handle());
        }
        Ok(entry)
    }
}

fn stale_handle() -> RuntimeError {
    RuntimeError::NotFound {
        context: "runtime_handle".into(),
    }
}

fn generation_exhausted() -> RuntimeError {
    RuntimeError::InvalidData {
        context: "runtime_generation_exhausted".into(),
    }
}

fn queue_lock_error() -> RuntimeError {
    RuntimeError::AdapterFailure {
        code: "queue_lock",
        context: "conversation queue".into(),
    }
}

#[cfg(test)]
pub(crate) mod ambient_is_task_local;
#[cfg(test)]
mod architecture_scan;
#[cfg(test)]
mod eviction;
#[cfg(test)]
mod keying;
#[cfg(test)]
#[path = "registry/tests/queue_ownership.rs"]
mod queue_ownership;

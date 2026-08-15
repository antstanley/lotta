//! Bounded listener-owned conversation runtime registry.

use crate::RuntimeError;
use lotta_domain::bounds::RUNTIMES_MAX;
use lotta_domain::{AgentId, ConversationId, RuntimeScope, TurnStateKind};
use std::collections::HashMap;

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

/// Complete immutable snapshot controlling runtime residency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeResidency {
    lifecycle: TurnStateKind,
    queue_item_count: usize,
    pending_approval_count: usize,
    interrupted_result_present: bool,
    sandbox_subscription_count: usize,
}

impl RuntimeResidency {
    /// Creates a complete residency snapshot with five independent terms.
    #[must_use]
    pub const fn new(
        lifecycle: TurnStateKind,
        queue_item_count: usize,
        pending_approval_count: usize,
        interrupted_result_present: bool,
        sandbox_subscription_count: usize,
    ) -> Self {
        Self {
            lifecycle,
            queue_item_count,
            pending_approval_count,
            interrupted_result_present,
            sandbox_subscription_count,
        }
    }

    /// Returns whether any of the exact five terms requires residency.
    #[must_use]
    pub fn requires_residency(self) -> bool {
        self.lifecycle != TurnStateKind::Idle
            || self.queue_item_count > 0
            || self.pending_approval_count > 0
            || self.interrupted_result_present
            || self.sandbox_subscription_count > 0
    }
}

struct RuntimeEntry {
    generation: u64,
    residency: RuntimeResidency,
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
}

impl Default for ListenerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ListenerRuntime {
    /// Creates an empty listener runtime registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            next_generation: 1,
        }
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

    /// Returns the immutable residency snapshot for a current handle.
    #[must_use]
    pub fn residency(&self, handle: &RuntimeHandle) -> Option<RuntimeResidency> {
        self.current_entry(handle).map(|entry| entry.residency)
    }

    /// Gets or creates one runtime without duplicate allocation.
    ///
    /// A new runtime begins in command lifecycle residency so creation remains valid until its
    /// caller publishes the actual complete state with [`Self::set_residency`].
    ///
    /// # Errors
    /// Returns [`RuntimeError::LimitExceeded`] before generation or allocation at the runtime
    /// bound, or [`RuntimeError::InvalidData`] if generation space is exhausted.
    pub fn get_or_create(&mut self, scope: &RuntimeScope) -> Result<RuntimeHandle, RuntimeError> {
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
        self.entries.insert(
            key.clone(),
            RuntimeEntry {
                generation,
                residency: RuntimeResidency::new(TurnStateKind::Command, 0, 0, false, 0),
            },
        );
        Ok(RuntimeHandle { key, generation })
    }

    /// Publishes state for a current handle and synchronously evicts quiescent state.
    ///
    /// # Errors
    /// Returns [`RuntimeError::NotFound`] when the key or generation is stale.
    pub fn set_residency(
        &mut self,
        handle: &RuntimeHandle,
        snapshot: RuntimeResidency,
    ) -> Result<ResidencyUpdate, RuntimeError> {
        let Some(entry) = self.entries.get_mut(&handle.key) else {
            return Err(stale_handle());
        };
        if entry.generation != handle.generation {
            return Err(stale_handle());
        }
        if snapshot.requires_residency() {
            entry.residency = snapshot;
            return Ok(ResidencyUpdate::Retained);
        }
        self.entries.remove(&handle.key);
        Ok(ResidencyUpdate::Evicted)
    }

    fn current_entry(&self, handle: &RuntimeHandle) -> Option<&RuntimeEntry> {
        self.entries
            .get(&handle.key)
            .filter(|entry| entry.generation == handle.generation)
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

#[cfg(test)]
pub(crate) mod ambient_is_task_local;
#[cfg(test)]
mod architecture_scan;
#[cfg(test)]
mod eviction;
#[cfg(test)]
mod keying;

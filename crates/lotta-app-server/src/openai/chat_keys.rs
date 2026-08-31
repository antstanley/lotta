use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use lotta_domain::ConversationId;
use tokio::sync::{Mutex, Notify};

use crate::bounds::OPENAI_CHAT_KEYS_MAX;

/// Largest accepted normalized external chat identity.
pub const OPENAI_CHAT_KEY_BYTES_MAX: usize = 1_024;

/// Structural persistent-chat scope, immune to delimiter collisions.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ChatScopeKey {
    /// Canonical agent identifier.
    pub agent_id: String,
    /// Caller-supplied persistent chat identifier.
    pub chat_id: String,
}

/// One shared conversation allocation result.
pub struct ConversationSlot {
    value: Mutex<Option<Result<ConversationId, ()>>>,
    settled: Notify,
    active: AtomicBool,
}

impl ConversationSlot {
    fn new() -> Self {
        Self {
            value: Mutex::new(None),
            settled: Notify::new(),
            active: AtomicBool::new(true),
        }
    }

    /// Waits for the allocation owner to settle.
    ///
    /// # Errors
    /// Returns an opaque error when canonical conversation allocation failed.
    pub async fn wait(&self) -> Result<ConversationId, ()> {
        loop {
            let notified = self.settled.notified();
            if let Some(value) = self.value.lock().await.clone() {
                return value;
            }
            notified.await;
        }
    }

    /// Settles the allocation exactly once.
    pub async fn settle(&self, value: Result<ConversationId, ()>) {
        let mut current = self.value.lock().await;
        if current.is_none() {
            *current = Some(value);
            self.active.store(false, Ordering::Release);
            drop(current);
            self.settled.notify_waiters();
        }
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}

/// Result of atomically consulting the chat-key cache.
pub enum ChatKeyClaim {
    /// This request owns conversation creation.
    Owner(Arc<ConversationSlot>),
    /// Another request owns or completed creation.
    Existing(Arc<ConversationSlot>),
    /// Every bounded entry is allocating, so no safe victim exists.
    Full,
}

/// Per-listener bounded cache that never evicts allocating ownership.
pub struct ChatKeyCache {
    inner: Mutex<ChatKeyEntries>,
}

struct ChatKeyEntries {
    values: HashMap<ChatScopeKey, Arc<ConversationSlot>>,
    order: VecDeque<ChatScopeKey>,
}

impl ChatKeyCache {
    /// Creates an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ChatKeyEntries {
                values: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    /// Atomically returns an existing slot or inserts one owner slot.
    pub async fn claim(&self, key: ChatScopeKey) -> ChatKeyClaim {
        let mut inner = self.inner.lock().await;
        if let Some(slot) = inner.values.get(&key) {
            return ChatKeyClaim::Existing(Arc::clone(slot));
        }
        if inner.values.len() == OPENAI_CHAT_KEYS_MAX && !evict_one_settled(&mut inner) {
            return ChatKeyClaim::Full;
        }
        let slot = Arc::new(ConversationSlot::new());
        inner.values.insert(key.clone(), Arc::clone(&slot));
        inner.order.push_back(key);
        debug_assert!(inner.values.len() <= OPENAI_CHAT_KEYS_MAX);
        ChatKeyClaim::Owner(slot)
    }

    /// Removes a failed slot only if it is still the current entry.
    pub async fn remove_failed(&self, key: &ChatScopeKey, slot: &Arc<ConversationSlot>) {
        let mut inner = self.inner.lock().await;
        if inner
            .values
            .get(key)
            .is_some_and(|found| Arc::ptr_eq(found, slot))
        {
            inner.values.remove(key);
            inner.order.retain(|candidate| candidate != key);
        }
    }

    #[cfg(test)]
    pub(crate) async fn owner_count(&self) -> usize {
        self.inner
            .lock()
            .await
            .values
            .values()
            .filter(|slot| slot.is_active())
            .count()
    }
}

impl Default for ChatKeyCache {
    fn default() -> Self {
        Self::new()
    }
}

fn evict_one_settled(inner: &mut ChatKeyEntries) -> bool {
    let Some(index) = inner
        .order
        .iter()
        .position(|key| inner.values.get(key).is_some_and(|slot| !slot.is_active()))
    else {
        return false;
    };
    if let Some(key) = inner.order.remove(index) {
        inner.values.remove(&key);
        true
    } else {
        false
    }
}

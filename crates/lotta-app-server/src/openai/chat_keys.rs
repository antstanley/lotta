//! Bounded FIFO chat-key to conversation allocation cache.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use lotta_domain::ConversationId;
use tokio::sync::{Mutex, Notify};

use crate::bounds::OPENAI_CHAT_KEYS_MAX;

/// Largest accepted normalized external chat identity.
pub const OPENAI_CHAT_KEY_BYTES_MAX: usize = 1_024;

/// One shared conversation allocation result.
pub struct ConversationSlot {
    value: Mutex<Option<Result<ConversationId, ()>>>,
    settled: Notify,
}

impl ConversationSlot {
    fn new() -> Self {
        Self {
            value: Mutex::new(None),
            settled: Notify::new(),
        }
    }

    /// Waits for the allocation owner to settle.
    ///
    /// # Errors
    /// Returns an opaque failure when the owning allocation failed.
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
            drop(current);
            self.settled.notify_waiters();
        }
    }
}

/// Result of atomically consulting the chat-key cache.
pub enum ChatKeyClaim {
    /// This request owns conversation creation.
    Owner(Arc<ConversationSlot>),
    /// Another request owns or completed creation.
    Existing(Arc<ConversationSlot>),
}

/// Per-listener bounded FIFO cache.
pub struct ChatKeyCache {
    inner: Mutex<ChatKeyEntries>,
}

struct ChatKeyEntries {
    values: HashMap<String, Arc<ConversationSlot>>,
    order: VecDeque<String>,
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
    pub async fn claim(&self, key: String) -> ChatKeyClaim {
        let mut inner = self.inner.lock().await;
        if let Some(slot) = inner.values.get(&key) {
            return ChatKeyClaim::Existing(Arc::clone(slot));
        }
        let slot = Arc::new(ConversationSlot::new());
        inner.values.insert(key.clone(), Arc::clone(&slot));
        inner.order.push_back(key);
        evict_overflow(&mut inner);
        ChatKeyClaim::Owner(slot)
    }

    /// Removes a failed slot only if it is still the current entry.
    pub async fn remove_failed(&self, key: &str, slot: &Arc<ConversationSlot>) {
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
}

impl Default for ChatKeyCache {
    fn default() -> Self {
        Self::new()
    }
}

fn evict_overflow(inner: &mut ChatKeyEntries) {
    while inner.values.len() > OPENAI_CHAT_KEYS_MAX {
        let Some(oldest) = inner.order.pop_front() else {
            break;
        };
        inner.values.remove(&oldest);
    }
    assert!(inner.values.len() <= OPENAI_CHAT_KEYS_MAX);
}

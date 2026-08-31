//! Race-safe bounded Chat Completions idempotency outcomes.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use serde::Serialize;
use tokio::sync::{Mutex, Notify, broadcast};

use crate::bounds::CHAT_IDEMPOTENCY_OUTCOMES_MAX;

/// Largest accepted normalized idempotency key.
pub const IDEMPOTENCY_KEY_BYTES_MAX: usize = 256;
/// Bounded live text fan-out retained per in-flight turn.
pub const CHAT_STREAM_EVENTS_MAX: usize = 64;

/// `OpenAI` token accounting projected from the runtime stream.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Usage {
    /// Input tokens.
    pub prompt_tokens: u64,
    /// Output tokens.
    pub completion_tokens: u64,
    /// Total tokens.
    pub total_tokens: u64,
    /// Optional reasoning-token detail.
    #[serde(skip_serializing)]
    pub reasoning_tokens: Option<u64>,
}

/// Successful or failed provider-turn outcome shared by duplicates.
#[derive(Clone, Debug)]
pub struct TurnOutcome {
    /// Complete assistant text.
    pub text: String,
    /// Token accounting.
    pub usage: Usage,
    /// Scrubbed terminal error.
    pub error: Option<String>,
}

/// One in-flight or settled outcome plus bounded live owner deltas.
pub struct OutcomeCell {
    value: Mutex<Option<Arc<TurnOutcome>>>,
    settled: Notify,
    deltas: broadcast::Sender<String>,
}

impl OutcomeCell {
    pub(crate) fn new() -> Self {
        let (deltas, _) = broadcast::channel(CHAT_STREAM_EVENTS_MAX);
        Self {
            value: Mutex::new(None),
            settled: Notify::new(),
            deltas,
        }
    }

    /// Subscribes to live assistant text without retaining an unbounded client queue.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.deltas.subscribe()
    }

    /// Publishes a live text piece; absent or lagging clients never block the turn.
    pub fn publish(&self, text: String) {
        let _ = self.deltas.send(text);
    }

    /// Waits for the owner to settle.
    pub async fn wait(&self) -> Arc<TurnOutcome> {
        loop {
            let notified = self.settled.notified();
            if let Some(value) = self.value.lock().await.clone() {
                return value;
            }
            notified.await;
        }
    }

    /// Settles exactly once and wakes every duplicate.
    pub async fn settle(&self, value: TurnOutcome) {
        let mut current = self.value.lock().await;
        if current.is_none() {
            *current = Some(Arc::new(value));
            drop(current);
            self.settled.notify_waiters();
        }
    }
}

/// Atomic idempotency-cache consultation result.
pub enum OutcomeClaim {
    /// This request owns allocation and execution.
    Owner(Arc<OutcomeCell>),
    /// An in-flight or successful prior request owns the outcome.
    Existing(Arc<OutcomeCell>),
}

/// Per-listener FIFO cache, counting successful and in-flight entries together.
pub struct OutcomeCache {
    inner: Mutex<OutcomeEntries>,
}

struct OutcomeEntries {
    values: HashMap<String, Arc<OutcomeCell>>,
    order: VecDeque<String>,
}

impl OutcomeCache {
    /// Creates an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(OutcomeEntries {
                values: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    /// Checks and inserts under one short lock, before conversation allocation.
    pub async fn claim(&self, key: String) -> OutcomeClaim {
        let mut inner = self.inner.lock().await;
        if let Some(cell) = inner.values.get(&key) {
            return OutcomeClaim::Existing(Arc::clone(cell));
        }
        let cell = Arc::new(OutcomeCell::new());
        inner.values.insert(key.clone(), Arc::clone(&cell));
        inner.order.push_back(key);
        evict_overflow(&mut inner);
        OutcomeClaim::Owner(cell)
    }

    /// Evicts a failed or cancelled owner only if its entry was not replaced.
    pub async fn evict_failed(&self, key: &str, cell: &Arc<OutcomeCell>) {
        let mut inner = self.inner.lock().await;
        if inner
            .values
            .get(key)
            .is_some_and(|found| Arc::ptr_eq(found, cell))
        {
            inner.values.remove(key);
            inner.order.retain(|candidate| candidate != key);
        }
    }
}

impl Default for OutcomeCache {
    fn default() -> Self {
        Self::new()
    }
}

fn evict_overflow(inner: &mut OutcomeEntries) {
    while inner.values.len() > CHAT_IDEMPOTENCY_OUTCOMES_MAX {
        let Some(oldest) = inner.order.pop_front() else {
            break;
        };
        inner.values.remove(&oldest);
    }
    assert!(inner.values.len() <= CHAT_IDEMPOTENCY_OUTCOMES_MAX);
}

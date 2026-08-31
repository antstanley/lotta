use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use serde::Serialize;
use tokio::sync::{Mutex, Notify, broadcast};

use crate::bounds::CHAT_IDEMPOTENCY_OUTCOMES_MAX;

/// Largest accepted normalized idempotency key.
pub const IDEMPOTENCY_KEY_BYTES_MAX: usize = 256;
/// Bounded live text fan-out retained per in-flight turn.
pub const CHAT_STREAM_EVENTS_MAX: usize = 64;

/// Structural identity for the chat side of an idempotency scope.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChatIdentity {
    /// A caller supplied an explicit persistent chat identity.
    Persistent(String),
    /// A headerless request is isolated by its bounded canonical request digest.
    Ephemeral([u8; 32]),
}

/// Collision-free idempotency scope. No delimiter encoding is used.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OutcomeKey {
    /// Canonical agent identifier.
    pub agent_id: String,
    /// Explicit chat identity or canonical headerless request fingerprint.
    pub chat: ChatIdentity,
    /// Caller-supplied idempotency key.
    pub idempotency_key: String,
}

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
    response_identity: Mutex<Option<(String, i64)>>,
    settled: Notify,
    active: AtomicBool,
    next_delta: AtomicU64,
    replay: StdMutex<Vec<(u64, String)>>,
    deltas: broadcast::Sender<(u64, String)>,
}

impl OutcomeCell {
    pub(crate) fn new() -> Self {
        let (deltas, _) = broadcast::channel(CHAT_STREAM_EVENTS_MAX);
        Self {
            value: Mutex::new(None),
            response_identity: Mutex::new(None),
            settled: Notify::new(),
            active: AtomicBool::new(true),
            next_delta: AtomicU64::new(1),
            replay: StdMutex::new(Vec::new()),
            deltas,
        }
    }

    /// Subscribes before taking a replay snapshot so no publication can be missed.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<(u64, String)> {
        self.deltas.subscribe()
    }

    /// Returns the ordered chunks already published for deterministic late joins.
    #[must_use]
    pub fn replay(&self) -> Vec<(u64, String)> {
        self.replay
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default()
    }

    /// Publishes a live text piece; absent or lagging clients never block the turn.
    pub fn publish(&self, text: String) {
        let sequence = self.next_delta.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut replay) = self.replay.lock() {
            if replay.len() < CHAT_STREAM_EVENTS_MAX {
                replay.push((sequence, text.clone()));
            } else if let Some((last_sequence, last_text)) = replay.last_mut() {
                *last_sequence = sequence;
                last_text.push_str(&text);
            }
            drop(self.deltas.send((sequence, text)));
        }
    }

    /// Returns one stable completion identity shared by every replay mode.
    pub async fn response_identity(&self, create: impl FnOnce() -> (String, i64)) -> (String, i64) {
        let mut identity = self.response_identity.lock().await;
        identity.get_or_insert_with(create).clone()
    }

    /// True while allocation or execution ownership is active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
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
            self.active.store(false, Ordering::Release);
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
    /// Every bounded entry is active, so no safe victim exists.
    Full,
}

/// Per-listener cache with active ownership independent from settled FIFO eviction.
pub struct OutcomeCache {
    inner: Mutex<OutcomeEntries>,
}

struct OutcomeEntries {
    values: HashMap<OutcomeKey, Arc<OutcomeCell>>,
    order: VecDeque<OutcomeKey>,
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
    pub async fn claim(&self, key: OutcomeKey) -> OutcomeClaim {
        let mut inner = self.inner.lock().await;
        if let Some(cell) = inner.values.get(&key) {
            return OutcomeClaim::Existing(Arc::clone(cell));
        }
        if inner.values.len() == CHAT_IDEMPOTENCY_OUTCOMES_MAX && !evict_one_settled(&mut inner) {
            return OutcomeClaim::Full;
        }
        let cell = Arc::new(OutcomeCell::new());
        inner.values.insert(key.clone(), Arc::clone(&cell));
        inner.order.push_back(key);
        debug_assert!(inner.values.len() <= CHAT_IDEMPOTENCY_OUTCOMES_MAX);
        OutcomeClaim::Owner(cell)
    }

    /// Evicts a failed or cancelled owner only if its entry was not replaced.
    pub async fn evict_failed(&self, key: &OutcomeKey, cell: &Arc<OutcomeCell>) {
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

    #[cfg(test)]
    pub(crate) async fn owner_count(&self) -> usize {
        self.inner
            .lock()
            .await
            .values
            .values()
            .filter(|cell| cell.is_active())
            .count()
    }
}

impl Default for OutcomeCache {
    fn default() -> Self {
        Self::new()
    }
}

fn evict_one_settled(inner: &mut OutcomeEntries) -> bool {
    let Some(index) = inner
        .order
        .iter()
        .position(|key| inner.values.get(key).is_some_and(|cell| !cell.is_active()))
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

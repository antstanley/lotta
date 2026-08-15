use super::*;
use crate::{ListenerRuntime, RuntimeResidency};
use lotta_domain::{AgentId, ConversationId, DomainError, RuntimeScope};

struct FakeClock(Mutex<Timestamp>);

impl FakeClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(timestamp("2026-01-01T00:00:00Z"))))
    }

    fn advance_ms(&self, milliseconds: i64) {
        let mut now = lock_state(&self.0);
        *now = now
            .checked_add(chrono::Duration::milliseconds(milliseconds))
            .unwrap_or_else(|error| panic!("advance: {error}"));
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        *lock_state(&self.0)
    }

    fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
        Timestamp::parse_persisted_rfc3339(value)
    }
}

fn timestamp(value: &str) -> Timestamp {
    Timestamp::parse_persisted_rfc3339(value).unwrap_or_else(|error| panic!("timestamp: {error}"))
}

async fn advance(clock: &FakeClock, milliseconds: i64) {
    tokio::task::yield_now().await;
    clock.advance_ms(milliseconds);
    tokio::time::advance(duration(milliseconds.abs())).await;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
}

#[tokio::test(start_paused = true)]
async fn automatic_below_and_at_threshold() {
    let clock = FakeClock::new();
    let watcher = WorktreeWatcher::new(clock.clone());
    advance(&clock, WORKTREE_WATCHER_IDLE_STOP_MS - 1).await;
    assert!(watcher.is_active());
    advance(&clock, 1).await;
    assert!(!watcher.is_active());
}

#[tokio::test(start_paused = true)]
async fn refresh_replaces_deadline() {
    let clock = FakeClock::new();
    let mut watcher = WorktreeWatcher::new(clock.clone());
    advance(&clock, WORKTREE_WATCHER_IDLE_STOP_MS - 1).await;
    watcher.record_activity();
    advance(&clock, WORKTREE_WATCHER_IDLE_STOP_MS - 1).await;
    assert!(watcher.is_active());
    advance(&clock, 1).await;
    assert!(!watcher.is_active());
}

#[tokio::test(start_paused = true)]
async fn activity_restarts_stopped_watcher() {
    let clock = FakeClock::new();
    let mut watcher = WorktreeWatcher::new(clock.clone());
    advance(&clock, WORKTREE_WATCHER_IDLE_STOP_MS).await;
    assert!(!watcher.is_active());
    watcher.record_activity();
    assert!(watcher.is_active());
    advance(&clock, WORKTREE_WATCHER_IDLE_STOP_MS).await;
    assert!(!watcher.is_active());
}

#[tokio::test(start_paused = true)]
async fn clock_regression_reschedules_remaining() {
    let clock = FakeClock::new();
    let watcher = WorktreeWatcher::new(clock.clone());
    clock.advance_ms(-1);
    tokio::time::advance(duration(WORKTREE_WATCHER_IDLE_STOP_MS)).await;
    tokio::task::yield_now().await;
    assert!(watcher.is_active());
    advance(&clock, WORKTREE_WATCHER_IDLE_STOP_MS + 1).await;
    assert!(!watcher.is_active());
}

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_and_joins() {
    let clock = FakeClock::new();
    let mut watcher = WorktreeWatcher::new(clock);
    watcher.shutdown().await;
    assert!(watcher.task.is_none());
    assert!(!watcher.is_active());
}

#[tokio::test(start_paused = true)]
async fn independent_from_runtime_registry() {
    let clock = FakeClock::new();
    let watcher = WorktreeWatcher::new(clock.clone());
    let mut registry = ListenerRuntime::new();
    let scope = RuntimeScope::new(
        AgentId::accept("agent").unwrap_or_else(|error| panic!("agent: {error}")),
        ConversationId::default_for_agent(),
        None,
    );
    let handle = registry
        .get_or_create(&scope, uuid::Uuid::from_u128(1))
        .unwrap_or_else(|error| panic!("runtime: {error}"));
    let residency = RuntimeResidency::new(1, 0, false, 0);
    let _update = registry.set_residency(&handle, residency);
    advance(&clock, WORKTREE_WATCHER_IDLE_STOP_MS).await;
    assert!(!watcher.is_active());
    assert_eq!(registry.residency(&handle), Some(residency));
}

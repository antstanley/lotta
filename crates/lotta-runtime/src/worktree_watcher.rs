use lotta_domain::{Clock, Timestamp};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Watcher inactivity threshold in milliseconds.
pub const WORKTREE_WATCHER_IDLE_STOP_MS: i64 = 1_800_000;

struct WatcherState {
    last_activity: Timestamp,
    active: bool,
}

/// Owned scheduled worktree watcher lifetime controller.
pub struct WorktreeWatcher<C: Clock + Send + Sync + 'static> {
    clock: Arc<C>,
    state: Arc<Mutex<WatcherState>>,
    cancellation: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl<C: Clock + Send + Sync + 'static> WorktreeWatcher<C> {
    /// Starts an active watcher and schedules its exact idle deadline.
    #[must_use]
    pub fn new(clock: Arc<C>) -> Self {
        let state = Arc::new(Mutex::new(WatcherState {
            last_activity: clock.now(),
            active: true,
        }));
        let cancellation = CancellationToken::new();
        let task = spawn_idle_stop(clock.clone(), state.clone(), cancellation.clone());
        Self {
            clock,
            state,
            cancellation,
            task: Some(task),
        }
    }

    /// Returns whether the watcher is currently active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        lock_state(&self.state).active
    }

    /// Refreshes activity, restarting the stopped watcher with a replacement timer task.
    pub fn record_activity(&mut self) {
        self.cancel_task();
        {
            let mut state = lock_state(&self.state);
            state.last_activity = self.clock.now();
            state.active = true;
        }
        self.cancellation = CancellationToken::new();
        self.task = Some(spawn_idle_stop(
            self.clock.clone(),
            self.state.clone(),
            self.cancellation.clone(),
        ));
    }

    /// Cancels and joins the owned timer task.
    pub async fn shutdown(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            let _result = task.await;
        }
        lock_state(&self.state).active = false;
    }

    fn cancel_task(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl<C: Clock + Send + Sync + 'static> Drop for WorktreeWatcher<C> {
    fn drop(&mut self) {
        self.cancel_task();
    }
}

/// Audited scope-free timer spawn: captures only clock, watcher state, and cancellation.
fn spawn_idle_stop<C>(
    clock: Arc<C>,
    state: Arc<Mutex<WatcherState>>,
    cancellation: CancellationToken,
) -> JoinHandle<()>
where
    C: Clock + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut wait_ms = WORKTREE_WATCHER_IDLE_STOP_MS;
        loop {
            tokio::select! {
                () = cancellation.cancelled() => return,
                () = tokio::time::sleep(duration(wait_ms)) => {}
            }
            let elapsed_ms = elapsed_since_last_activity(clock.as_ref(), &state);
            if elapsed_ms >= WORKTREE_WATCHER_IDLE_STOP_MS {
                lock_state(&state).active = false;
                return;
            }
            wait_ms = WORKTREE_WATCHER_IDLE_STOP_MS.saturating_sub(elapsed_ms.max(0));
        }
    })
}

fn elapsed_since_last_activity<C: Clock>(clock: &C, state: &Mutex<WatcherState>) -> i64 {
    let last_activity = lock_state(state).last_activity;
    clock
        .now()
        .as_utc()
        .signed_duration_since(*last_activity.as_utc())
        .num_milliseconds()
}

fn duration(milliseconds: i64) -> Duration {
    Duration::from_millis(u64::try_from(milliseconds.max(0)).unwrap_or(0))
}

fn lock_state<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod idle_stop;

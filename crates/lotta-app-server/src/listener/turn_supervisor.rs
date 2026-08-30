//! Listener-runtime ownership for admitted turn tasks.

use std::{future::Future, pin::Pin, sync::Arc};

use lotta_domain::bounds::RUNTIMES_MAX;
use tokio::{
    sync::{Mutex, Semaphore, mpsc},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

use crate::error::AppServerError;

type TurnFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// One admitted task per bounded conversation runtime.
const RUNTIME_TURN_TASKS_MAX: usize = RUNTIMES_MAX.value;

/// Process-runtime owner for admitted turns, independent of WebSocket lifetime.
pub(super) struct RuntimeTurnSupervisor {
    permits: Arc<Semaphore>,
    runtime_cancellation: CancellationToken,
    stop: CancellationToken,
    state: Mutex<SupervisorState>,
}

struct SupervisorState {
    sender: Option<mpsc::Sender<TurnFuture>>,
    task: Option<JoinHandle<Result<(), AppServerError>>>,
    stopped: bool,
}

impl RuntimeTurnSupervisor {
    pub(super) fn new(runtime_cancellation: CancellationToken) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(RUNTIME_TURN_TASKS_MAX)),
            runtime_cancellation,
            stop: CancellationToken::new(),
            state: Mutex::new(SupervisorState {
                sender: None,
                task: None,
                stopped: false,
            }),
        }
    }

    pub(super) fn cancellation(&self) -> CancellationToken {
        self.runtime_cancellation.child_token()
    }

    pub(super) async fn spawn(&self, turn: TurnFuture) -> Result<(), AppServerError> {
        let permits = Arc::clone(&self.permits);
        let permit = tokio::select! {
            result = permits.acquire_owned() => result.map_err(|_| AppServerError::Unavailable),
            () = self.runtime_cancellation.cancelled() => {
                turn.await;
                return Err(AppServerError::Unavailable);
            }
        }?;
        let supervised = Box::pin(async move {
            let _permit = permit;
            turn.await;
        });
        let mut state = self.state.lock().await;
        if state.stopped || self.runtime_cancellation.is_cancelled() {
            drop(state);
            supervised.await;
            return Err(AppServerError::Unavailable);
        }
        if state.sender.is_none() {
            let (sender, receiver) = mpsc::channel(RUNTIME_TURN_TASKS_MAX);
            state.task = Some(tokio::spawn(supervise_turns(receiver, self.stop.clone())));
            state.sender = Some(sender);
        }
        let Some(sender) = state.sender.clone() else {
            drop(state);
            supervised.await;
            return Err(AppServerError::Internal);
        };
        let result = sender.send(supervised).await;
        drop(state);
        if let Err(error) = result {
            error.0.await;
            return Err(AppServerError::Unavailable);
        }
        Ok(())
    }

    pub(super) async fn shutdown(&self) -> Result<(), AppServerError> {
        self.runtime_cancellation.cancel();
        let task = {
            let mut state = self.state.lock().await;
            if state.stopped {
                return Ok(());
            }
            state.stopped = true;
            self.stop.cancel();
            state.sender.take();
            state.task.take()
        };
        match task {
            Some(task) => task.await.map_err(|_| AppServerError::Task)?,
            None => Ok(()),
        }
    }
}

impl Drop for RuntimeTurnSupervisor {
    fn drop(&mut self) {
        self.runtime_cancellation.cancel();
        self.stop.cancel();
    }
}

async fn supervise_turns(
    mut receiver: mpsc::Receiver<TurnFuture>,
    stop: CancellationToken,
) -> Result<(), AppServerError> {
    let mut turns = JoinSet::new();
    let mut task_failed = false;
    loop {
        tokio::select! {
            biased;
            joined = turns.join_next(), if !turns.is_empty() => {
                task_failed |= joined.is_some_and(|result| result.is_err());
            }
            () = stop.cancelled() => {
                receiver.close();
                break;
            }
            turn = receiver.recv() => match turn {
                Some(turn) => {
                    turns.spawn(turn);
                }
                None => break,
            }
        }
    }
    while let Ok(turn) = receiver.try_recv() {
        turns.spawn(turn);
    }
    while let Some(result) = turns.join_next().await {
        task_failed |= result.is_err();
    }
    if task_failed {
        Err(AppServerError::Task)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use tokio::sync::oneshot;

    use super::RuntimeTurnSupervisor;

    #[tokio::test]
    async fn unrelated_transport_cancellation_does_not_cancel_an_admitted_turn() {
        let runtime = tokio_util::sync::CancellationToken::new();
        let supervisor = RuntimeTurnSupervisor::new(runtime);
        let transport = tokio_util::sync::CancellationToken::new();
        let release = tokio_util::sync::CancellationToken::new();
        let finished = Arc::new(AtomicUsize::new(0));
        let task_finished = Arc::clone(&finished);
        let task_release = release.clone();
        supervisor
            .spawn(Box::pin(async move {
                task_release.cancelled().await;
                task_finished.fetch_add(1, Ordering::SeqCst);
            }))
            .await
            .expect("admit turn");

        transport.cancel();
        tokio::task::yield_now().await;
        assert_eq!(finished.load(Ordering::SeqCst), 0);
        release.cancel();
        supervisor.shutdown().await.expect("shutdown");
        assert_eq!(finished.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn shutdown_cancels_joins_and_cleans_once() {
        let runtime = tokio_util::sync::CancellationToken::new();
        let supervisor = RuntimeTurnSupervisor::new(runtime);
        let cancellation = supervisor.cancellation();
        let completed = Arc::new(AtomicUsize::new(0));
        let task_completed = Arc::clone(&completed);
        let (started, observed_start) = oneshot::channel();
        supervisor
            .spawn(Box::pin(async move {
                let _ignored = started.send(());
                cancellation.cancelled().await;
                task_completed.fetch_add(1, Ordering::SeqCst);
            }))
            .await
            .expect("admit turn");
        observed_start.await.expect("turn started");

        supervisor.shutdown().await.expect("first shutdown");
        supervisor.shutdown().await.expect("second shutdown");
        assert_eq!(completed.load(Ordering::SeqCst), 1);
    }
}

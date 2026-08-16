//! Restart-bounded transactional sidecar process supervision.

use lotta_domain::{Clock, Timestamp};
use std::{collections::VecDeque, future::Future, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

/// Canonical restarts accepted during one rolling hour, excluding initial launch.
pub const SIDECAR_RESTARTS_PER_HOUR_MAX: usize = 10;
const SIDECAR_RESTART_WINDOW_SECONDS: i64 = 3_600;
/// Maximum time the supervisor waits for an exact child to join after stop.
pub const SIDECAR_CHILD_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Terminal supervised-run failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SidecarRunFailure {
    /// Restart admission was refused before spawn.
    #[error("sidecar restart rate exceeded")]
    RestartLimit,
    /// Launching the child failed.
    #[error("sidecar child launch failed")]
    Launch,
    /// Child exited before its commit checkpoint.
    #[error("sidecar child crashed")]
    Crash,
    /// Session framing or validation failed.
    #[error("sidecar protocol failed")]
    Protocol,
    /// Host cancellation stopped and joined the child.
    #[error("sidecar run cancelled")]
    Cancelled,
    /// Stopping the exact child failed.
    #[error("sidecar child stop failed")]
    Stop,
    /// Joining the exact child failed.
    #[error("sidecar child join failed")]
    Join,
    /// Joining the exact child exceeded the host bound.
    #[error("sidecar child join timed out")]
    JoinTimeout,
}

/// Restart admission failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("sidecar restart rate exceeded")]
pub struct RestartLimitError;

/// Candidate produced privately by a child before the commit checkpoint.
#[derive(Clone, Debug)]
pub struct SidecarCandidate<T> {
    state: Arc<T>,
}
impl<T> SidecarCandidate<T> {
    /// Creates uncommitted candidate state.
    #[must_use]
    pub fn new(state: T) -> Self {
        Self {
            state: Arc::new(state),
        }
    }
}

/// Host state committed only after successful child completion.
#[derive(Clone, Debug)]
pub struct SidecarHostState<T> {
    current: Arc<T>,
}
impl<T> SidecarHostState<T> {
    /// Creates the initial stable host state.
    #[must_use]
    pub fn new(initial: T) -> Self {
        Self {
            current: Arc::new(initial),
        }
    }
    /// Borrows the exact committed allocation.
    #[must_use]
    pub fn current(&self) -> &Arc<T> {
        &self.current
    }
}

/// Bounded lifecycle port for one exact spawned sidecar child.
pub trait SidecarChild<T> {
    /// Child run future.
    type Run<'a>: Future<Output = Result<SidecarCandidate<T>, SidecarRunFailure>> + 'a
    where
        Self: 'a,
        T: 'a;
    /// Child stop future.
    type Stop<'a>: Future<Output = Result<(), SidecarRunFailure>> + 'a
    where
        Self: 'a;
    /// Child join future.
    type Join<'a>: Future<Output = Result<(), SidecarRunFailure>> + 'a
    where
        Self: 'a;

    /// Runs this exact child/session to its transactional checkpoint.
    fn run(&mut self) -> Self::Run<'_>;
    /// Requests stop for this exact child.
    fn stop(&mut self) -> Self::Stop<'_>;
    /// Joins this exact child.
    fn join(&mut self) -> Self::Join<'_>;
}

/// Factory that launches one owned sidecar child.
pub trait SidecarLauncher<T> {
    /// Exact child type returned by this launcher.
    type Child: SidecarChild<T>;
    /// Launch future.
    type Launch<'a>: Future<Output = Result<Self::Child, SidecarRunFailure>> + 'a
    where
        Self: 'a,
        T: 'a;

    /// Spawns one exact child only after supervisor admission.
    fn launch(&mut self) -> Self::Launch<'_>;
}

/// Restart-bounded supervisor using an injected monotonic-compatible clock.
pub struct SidecarSupervisor<C> {
    clock: Arc<C>,
    initial_admitted: bool,
    restarts: VecDeque<Timestamp>,
    last_seen: Option<Timestamp>,
    join_timeout: Duration,
}
impl<C: Clock> SidecarSupervisor<C> {
    /// Creates an empty supervisor with a free initial launch and rolling restart history.
    #[must_use]
    pub fn new(clock: Arc<C>) -> Self {
        Self {
            clock,
            initial_admitted: false,
            restarts: VecDeque::with_capacity(SIDECAR_RESTARTS_PER_HOUR_MAX),
            last_seen: None,
            join_timeout: SIDECAR_CHILD_JOIN_TIMEOUT,
        }
    }

    /// Overrides the bounded join wait, primarily for deterministic host tests.
    #[must_use]
    pub const fn with_join_timeout(mut self, join_timeout: Duration) -> Self {
        self.join_timeout = join_timeout;
        self
    }

    /// Admits the unique initial launch without consuming restart quota.
    pub fn admit_initial(&mut self) -> Result<(), RestartLimitError> {
        if self.initial_admitted {
            return Err(RestartLimitError);
        }
        self.initial_admitted = true;
        self.observe_now();
        Ok(())
    }

    /// Admits one rolling restart after initial launch.
    pub fn admit_restart(&mut self) -> Result<(), RestartLimitError> {
        if !self.initial_admitted {
            return Err(RestartLimitError);
        }
        let now = self.observe_now();
        while self
            .restarts
            .front()
            .is_some_and(|restart| elapsed_seconds(*restart, now) >= SIDECAR_RESTART_WINDOW_SECONDS)
        {
            self.restarts.pop_front();
        }
        if self.restarts.len() >= SIDECAR_RESTARTS_PER_HOUR_MAX {
            return Err(RestartLimitError);
        }
        self.restarts.push_back(now);
        Ok(())
    }

    /// Admits, launches, runs, stops, and bounded-joins one exact child.
    ///
    /// Restart admission occurs before launch. Host state commits only after successful run and
    /// successful cleanup of the same child identity.
    pub async fn supervise<T, L>(
        &mut self,
        host: &mut SidecarHostState<T>,
        cancellation: &CancellationToken,
        launcher: &mut L,
        initial: bool,
    ) -> Result<(), SidecarRunFailure>
    where
        L: SidecarLauncher<T>,
    {
        if initial {
            self.admit_initial()
        } else {
            self.admit_restart()
        }
        .map_err(|_| SidecarRunFailure::RestartLimit)?;

        let mut child = launcher
            .launch()
            .await
            .map_err(|_| SidecarRunFailure::Launch)?;
        let run_result = tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(SidecarRunFailure::Cancelled),
            result = child.run() => result,
        };
        let stop_result = child.stop().await;
        let join_result = match tokio::time::timeout(self.join_timeout, child.join()).await {
            Ok(result) => result,
            Err(_) => Err(SidecarRunFailure::JoinTimeout),
        };

        if let Err(error) = stop_result {
            return Err(match error {
                SidecarRunFailure::Stop => error,
                _ => SidecarRunFailure::Stop,
            });
        }
        if let Err(error) = join_result {
            return Err(match error {
                SidecarRunFailure::JoinTimeout => error,
                _ => SidecarRunFailure::Join,
            });
        }
        match run_result {
            Ok(candidate) => {
                host.current = candidate.state;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Returns retained restart timestamps for bounded-history diagnostics.
    #[must_use]
    pub fn retained_restarts(&self) -> usize {
        self.restarts.len()
    }

    fn observe_now(&mut self) -> Timestamp {
        let observed = self.clock.now();
        let now = self.last_seen.map_or(
            observed,
            |last| {
                if observed < last { last } else { observed }
            },
        );
        self.last_seen = Some(now);
        now
    }
}

fn elapsed_seconds(start: Timestamp, end: Timestamp) -> i64 {
    end.as_utc()
        .signed_duration_since(start.as_utc())
        .num_seconds()
}

use super::supervisor::*;
use chrono::Duration;
use lotta_domain::Timestamp;
use lotta_testkit::clock::FakeClock;
use std::{
    future::{Pending, Ready},
    sync::{Arc, Mutex},
    time::Duration as StdDuration,
};
use tokio_util::sync::CancellationToken;

fn clock() -> Arc<FakeClock> {
    Arc::new(FakeClock::new(
        Timestamp::parse_persisted_rfc3339("2025-01-01T00:00:00Z").unwrap(),
    ))
}

#[test]
fn bounds_restarts_per_hour() {
    let clock = clock();
    let mut supervisor = SidecarSupervisor::new(Arc::clone(&clock));
    supervisor.admit_initial().unwrap();
    assert_eq!(supervisor.retained_restarts(), 0);
    for _ in 0..SIDECAR_RESTARTS_PER_HOUR_MAX {
        supervisor.admit_restart().unwrap();
    }
    assert!(supervisor.admit_restart().is_err());
    assert_eq!(
        supervisor.retained_restarts(),
        SIDECAR_RESTARTS_PER_HOUR_MAX
    );

    clock.advance(Duration::seconds(3_599)).unwrap();
    assert!(supervisor.admit_restart().is_err());
    clock.advance(Duration::seconds(1)).unwrap();
    supervisor.admit_restart().unwrap();
    assert_eq!(supervisor.retained_restarts(), 1);

    clock.advance(Duration::seconds(-1)).unwrap();
    for _ in 1..SIDECAR_RESTARTS_PER_HOUR_MAX {
        supervisor.admit_restart().unwrap();
    }
    assert!(supervisor.admit_restart().is_err());
}

#[derive(Clone, Copy)]
enum RunOutcome {
    Success,
    Crash,
    Protocol,
    Pending,
}

#[derive(Default, Debug)]
struct LifecycleCounts {
    spawned: usize,
    run: usize,
    stop: usize,
    join: usize,
    ids: Vec<usize>,
}

struct FakeLauncher {
    counts: Arc<Mutex<LifecycleCounts>>,
    outcome: RunOutcome,
}
struct FakeChild {
    id: usize,
    counts: Arc<Mutex<LifecycleCounts>>,
    outcome: RunOutcome,
}
impl SidecarLauncher<Vec<u8>> for FakeLauncher {
    type Child = FakeChild;
    type Launch<'a> = Ready<Result<Self::Child, SidecarRunFailure>>;

    fn launch(&mut self) -> Self::Launch<'_> {
        let id = {
            let mut counts = self.counts.lock().unwrap();
            counts.spawned += 1;
            counts.spawned
        };
        std::future::ready(Ok(FakeChild {
            id,
            counts: Arc::clone(&self.counts),
            outcome: self.outcome,
        }))
    }
}
impl SidecarChild<Vec<u8>> for FakeChild {
    type Run<'a> = PinRunFuture;
    type Stop<'a> = Ready<Result<(), SidecarRunFailure>>;
    type Join<'a> = Ready<Result<(), SidecarRunFailure>>;

    fn run(&mut self) -> Self::Run<'_> {
        {
            let mut counts = self.counts.lock().unwrap();
            counts.run += 1;
            counts.ids.push(self.id);
        }
        match self.outcome {
            RunOutcome::Success => {
                PinRunFuture::Ready(std::future::ready(Ok(SidecarCandidate::new(vec![2]))))
            }
            RunOutcome::Crash => {
                PinRunFuture::Ready(std::future::ready(Err(SidecarRunFailure::Crash)))
            }
            RunOutcome::Protocol => {
                PinRunFuture::Ready(std::future::ready(Err(SidecarRunFailure::Protocol)))
            }
            RunOutcome::Pending => PinRunFuture::Pending(std::future::pending()),
        }
    }

    fn stop(&mut self) -> Self::Stop<'_> {
        let mut counts = self.counts.lock().unwrap();
        counts.stop += 1;
        counts.ids.push(self.id);
        std::future::ready(Ok(()))
    }

    fn join(&mut self) -> Self::Join<'_> {
        let mut counts = self.counts.lock().unwrap();
        counts.join += 1;
        counts.ids.push(self.id);
        std::future::ready(Ok(()))
    }
}

enum PinRunFuture {
    Ready(Ready<Result<SidecarCandidate<Vec<u8>>, SidecarRunFailure>>),
    Pending(Pending<Result<SidecarCandidate<Vec<u8>>, SidecarRunFailure>>),
}
impl std::future::Future for PinRunFuture {
    type Output = Result<SidecarCandidate<Vec<u8>>, SidecarRunFailure>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match &mut *self {
            Self::Ready(future) => std::pin::Pin::new(future).poll(context),
            Self::Pending(future) => std::pin::Pin::new(future).poll(context),
        }
    }
}

#[tokio::test]
async fn crash_leaves_host_consistent() {
    for (outcome, expected, cancel) in [
        (RunOutcome::Success, Ok(()), false),
        (RunOutcome::Crash, Err(SidecarRunFailure::Crash), false),
        (
            RunOutcome::Protocol,
            Err(SidecarRunFailure::Protocol),
            false,
        ),
        (RunOutcome::Pending, Err(SidecarRunFailure::Cancelled), true),
    ] {
        let counts = Arc::new(Mutex::new(LifecycleCounts::default()));
        let mut launcher = FakeLauncher {
            counts: Arc::clone(&counts),
            outcome,
        };
        let mut supervisor =
            SidecarSupervisor::new(clock()).with_join_timeout(StdDuration::from_millis(50));
        let mut host = SidecarHostState::new(vec![1]);
        let prior = Arc::clone(host.current());
        let cancellation = CancellationToken::new();
        if cancel {
            cancellation.cancel();
        }
        let result = supervisor
            .supervise(&mut host, &cancellation, &mut launcher, true)
            .await;
        assert_eq!(result, expected);
        let counts = counts.lock().unwrap();
        assert_eq!(
            (counts.spawned, counts.run, counts.stop, counts.join),
            (1, 1, 1, 1)
        );
        assert_eq!(counts.ids, [1, 1, 1]);
        if expected.is_ok() {
            assert_eq!(host.current().as_slice(), &[2]);
        } else {
            assert!(Arc::ptr_eq(&prior, host.current()));
        }
    }
}

#[tokio::test]
async fn refused_restart_does_not_spawn() {
    let counts = Arc::new(Mutex::new(LifecycleCounts::default()));
    let mut launcher = FakeLauncher {
        counts: Arc::clone(&counts),
        outcome: RunOutcome::Success,
    };
    let mut supervisor = SidecarSupervisor::new(clock());
    supervisor.admit_initial().unwrap();
    for _ in 0..SIDECAR_RESTARTS_PER_HOUR_MAX {
        supervisor.admit_restart().unwrap();
    }
    let mut host = SidecarHostState::new(vec![1]);
    let result = supervisor
        .supervise(&mut host, &CancellationToken::new(), &mut launcher, false)
        .await;
    assert_eq!(result, Err(SidecarRunFailure::RestartLimit));
    assert_eq!(counts.lock().unwrap().spawned, 0);
    assert_eq!(host.current().as_slice(), &[1]);
}

use super::cancel::*;
use super::test_support::{RecordingEffects, id, runtime};
use crate::ports::PortFuture;
use crate::{CancellationPolicy, LeaseGuard, RuntimeError};
use lotta_domain::{NonEmptyString, RunId, TurnStateKind};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn record() -> super::TurnStopRecord {
    super::TurnStopRecord {
        turn_id: NonEmptyString::new("turn").unwrap(),
        run_id: RunId::generate_sequence(1).unwrap(),
        input_id: NonEmptyString::new("input").unwrap(),
        reason: super::TurnStopReason::UserCancellation,
        provider_failure: None,
    }
}

fn context<'a>(
    runtime: &'a mut crate::ListenerRuntime,
    handle: crate::RuntimeHandle,
    lease: lotta_domain::TurnLease,
    cancellation: CancellationToken,
    effects: &'a RecordingEffects,
) -> CancelContext<'a> {
    CancelContext {
        runtime,
        guard: LeaseGuard::new(
            handle,
            lease,
            cancellation.clone(),
            CancellationPolicy::PermitDuringCancellationCleanup,
        ),
        provider_cancellation: cancellation,
        unfinished: UnfinishedCallTracker::default(),
        effects,
        children: None,
        post_turn: None,
        record: record(),
        stages: None,
    }
}

mod cancel {
    use super::*;

    mod step_order {
        use super::*;

        #[tokio::test]
        async fn six_exact_steps() {
            let (mut runtime, handle, lease) = runtime();
            let effects = RecordingEffects::default();
            let stages = Arc::new(Mutex::new(Vec::new()));
            let mut context = context(
                &mut runtime,
                handle,
                lease,
                CancellationToken::new(),
                &effects,
            );
            let recorded = Arc::clone(&stages);
            context.stages = Some(Arc::new(move |step| recorded.lock().unwrap().push(step)));
            cancel_turn(&mut context).await.unwrap();
            assert_eq!(
                *stages.lock().unwrap(),
                [
                    CancelStep::ClaimAndCancel,
                    CancelStep::NormalizeUnfinished,
                    CancelStep::SuppressEffects,
                    CancelStep::KillChildren,
                    CancelStep::PersistTerminal,
                    CancelStep::EmitAndRelease,
                ]
            );
        }
    }

    mod normalizes_unfinished_tools {
        use super::*;

        #[tokio::test]
        async fn mixed_completed_pending_local_controller_preserves_provider_order_once() {
            let (mut runtime, handle, lease) = runtime();
            let effects = RecordingEffects::default();
            let local = CancellationToken::new();
            let controller = CancellationToken::new();
            let mut context = context(
                &mut runtime,
                handle,
                lease,
                CancellationToken::new(),
                &effects,
            );
            assert!(register(&mut context, "local-done", &local));
            assert!(context.unfinished.mark_completed(&id("local-done")));
            assert!(register(&mut context, "controller-pending", &controller));
            assert!(!register(&mut context, "controller-pending", &controller));
            assert!(register(&mut context, "local-pending", &local));
            cancel_turn(&mut context).await.unwrap();
            assert!(local.is_cancelled());
            assert!(controller.is_cancelled());
            let ids: Vec<_> = effects
                .results
                .lock()
                .unwrap()
                .iter()
                .map(|result| result.call_id.clone())
                .collect();
            assert_eq!(ids, [id("controller-pending"), id("local-pending")]);
            assert!(
                effects
                    .results
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|result| matches!(
                        result.outcome,
                        crate::ports::ToolOutcome::Interruption { .. }
                    ))
            );
            assert!(matches!(
                cancel_turn(&mut context).await.unwrap(),
                crate::LeaseEffect::Suppressed(_)
            ));
            assert_eq!(effects.results.lock().unwrap().len(), 2);
        }
    }

    fn register(
        context: &mut CancelContext<'_>,
        call_id: &str,
        cancellation: &CancellationToken,
    ) -> bool {
        context.unfinished.register(UnfinishedToolCall {
            call_id: id(call_id),
            cancellation: cancellation.clone(),
        })
    }

    struct Children(Arc<Mutex<Vec<Duration>>>);
    impl TurnChildOwner for Children {
        fn has_operations(&self) -> bool {
            true
        }

        fn terminate_and_reap(&self, grace: Duration) -> PortFuture<'_, ()> {
            self.0.lock().unwrap().push(grace);
            Box::pin(async { Ok(()) })
        }
    }

    mod child_kill {
        use super::*;

        #[tokio::test(start_paused = true)]
        async fn fake_clock_bounds_and_process_group_empty() {
            let (mut runtime, handle, lease) = runtime();
            let effects = RecordingEffects::default();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let children = Children(Arc::clone(&calls));
            let mut context = context(
                &mut runtime,
                handle,
                lease,
                CancellationToken::new(),
                &effects,
            );
            context.children = Some(&children);
            let task = cancel_turn(&mut context);
            tokio::pin!(task);
            assert!(
                tokio::time::timeout(Duration::ZERO, &mut task)
                    .await
                    .is_err()
            );
            tokio::time::advance(Duration::from_millis(TURN_CANCEL_GRACE_MS - 1)).await;
            assert!(calls.lock().unwrap().is_empty());
            tokio::time::advance(Duration::from_millis(1)).await;
            task.await.unwrap();
            assert_eq!(
                *calls.lock().unwrap(),
                [Duration::from_millis(CHILD_KILL_GRACE_MS)]
            );
        }
    }

    mod exactly_once {
        use super::*;
        use crate::{CancellationClaim, LeaseEffect, TurnEffectPort, TurnEvent};
        use std::sync::Barrier;

        type ClaimResult = (LeaseGuard, LeaseEffect<CancellationClaim>);
        type SharedClaims = Arc<Mutex<Vec<ClaimResult>>>;

        #[test]
        fn three_barrier_threads_claim_normalize_persist_emit_and_release_once() {
            let (runtime, handle, lease) = runtime();
            let shared = Arc::new(Mutex::new(runtime));
            let barrier = Arc::new(Barrier::new(3));
            let claims = Arc::new(Mutex::new(Vec::new()));
            let mut threads = Vec::new();
            for _ in 0..3 {
                let shared = Arc::clone(&shared);
                let barrier = Arc::clone(&barrier);
                let claims = Arc::clone(&claims);
                let guard = LeaseGuard::new(
                    handle.clone(),
                    lease.clone(),
                    CancellationToken::new(),
                    CancellationPolicy::PermitDuringCancellationCleanup,
                );
                threads.push(spawn_claimant(shared, barrier, claims, guard));
            }
            for thread in threads {
                thread.join().unwrap();
            }
            let mut claims = claims.lock().unwrap();
            let winner = claims.iter_mut().find_map(|(guard, outcome)| {
                let outcome = std::mem::replace(
                    outcome,
                    LeaseEffect::Suppressed(crate::SuppressionReason::StaleLease),
                );
                match outcome {
                    LeaseEffect::Applied(claim) => Some((guard, claim)),
                    LeaseEffect::Suppressed(_) => None,
                }
            });
            let (guard, claim) = winner.expect("one cancellation claimant");
            let effects = RecordingEffects::default();
            let applied = guard
                .finish_cancelled_turn_with_effect_after_await(
                    &mut shared.lock().unwrap(),
                    claim,
                    lotta_domain::StopReason::new("user_cancellation").unwrap(),
                    || {
                        effects.persist_stop_reason(record())?;
                        effects.emit(TurnEvent::Cancelled)
                    },
                )
                .unwrap();
            assert!(matches!(applied, LeaseEffect::Applied(_)));
            assert_eq!(effects.stops.lock().unwrap().len(), 1);
            assert_eq!(effects.events.lock().unwrap().len(), 1);
            assert_eq!(&*effects.order.lock().unwrap(), &["stop", "finished"]);
            assert_eq!(
                shared
                    .lock()
                    .unwrap()
                    .lifecycle(&handle)
                    .unwrap()
                    .projection()
                    .state(),
                TurnStateKind::Idle
            );
        }

        #[cfg(test)]
        fn spawn_claimant(
            shared: Arc<Mutex<crate::ListenerRuntime>>,
            barrier: Arc<Barrier>,
            claims: SharedClaims,
            guard: LeaseGuard,
        ) -> std::thread::JoinHandle<()> {
            std::thread::spawn(move || {
                barrier.wait();
                let outcome = guard.claim_cancellation(&mut shared.lock().unwrap());
                claims.lock().unwrap().push((guard, outcome.unwrap()));
            })
        }
    }

    struct FailingPost(Arc<Mutex<u8>>);
    impl PostTurnPort for FailingPost {
        fn run(&self) -> PortFuture<'_, ()> {
            *self.0.lock().unwrap() += 1;
            Box::pin(async {
                Err(RuntimeError::AdapterFailure {
                    code: "reflection",
                    context: "injected reflection failure".into(),
                })
            })
        }
    }

    mod reflection_after_terminal {
        use super::*;

        #[tokio::test]
        async fn failure_outcome_unchanged() {
            let (mut runtime, handle, lease) = runtime();
            let effects = RecordingEffects::default();
            let calls = Arc::new(Mutex::new(0));
            let post = FailingPost(Arc::clone(&calls));
            let mut context = context(
                &mut runtime,
                handle,
                lease,
                CancellationToken::new(),
                &effects,
            );
            context.post_turn = Some(&post);
            cancel_turn(&mut context).await.unwrap();
            assert_eq!(*calls.lock().unwrap(), 1);
            assert_eq!(effects.stops.lock().unwrap().len(), 1);
            assert_eq!(effects.events.lock().unwrap().len(), 1);
        }
    }
}

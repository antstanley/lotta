//! Certificate selectors for the lease-aware cron scheduler.
//!
//! Tests drive the real admission chain over a shared [`ListenerRuntime`] with
//! in-memory adapters for the ports the runtime does not own (clock and
//! schedule persistence), per the repo testing guideline.

mod scheduler {
    use lotta_domain::{
        AgentId, BoundedJsonValue, Clock, ConversationId, DomainError, EntityExtras,
        InputDisposition, MessageId, NonEmptyString, QueueItem, QueueItemKind, QueueItemSource,
        RunId, RuntimeScope, Schedule, ScheduleRunOutcome, ScheduleStatus, Timestamp,
        TurnStateKind,
    };
    use lotta_runtime::ListenerRuntime;
    use lotta_runtime::RuntimeError;
    use lotta_runtime::admission::{AdmissionOutcome, AdmissionRequest, AdmissionRoute};
    use lotta_runtime::ports::{IdGenerator, PortFuture, SchedulePersistence};
    use lotta_runtime::registry::{RuntimeHandle, RuntimeKey, RuntimeResidency};
    use lotta_runtime::schedule::{
        RunLogEntry, RunLogStatus, RunUpdate, ScheduleFile, ScheduleScheduler, apply_run_update,
    };
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    /// Minute-aligned canonical evaluation instant: 2026-01-03T00:00:00Z.
    fn t0() -> Timestamp {
        Timestamp::parse_persisted_rfc3339("2026-01-03T00:00:00Z").expect("t0")
    }

    fn seconds_from(base: Timestamp, seconds: i64) -> Timestamp {
        base.checked_add(chrono::Duration::seconds(seconds))
            .expect("offset")
    }

    fn non_empty(value: String) -> NonEmptyString {
        NonEmptyString::new(value).expect("non-empty")
    }

    fn schedule(recurring: bool, jitter_ms: i64, scheduled_for: Option<Timestamp>) -> Schedule {
        let scheduled_for = scheduled_for.map(|instant| instant.to_string());
        serde_json::from_value(serde_json::json!({
            "id": "schedule-1", "agent_id": "agent", "conversation_id": "conversation",
            "name": "name", "description": "description", "cron": "* * * * *",
            "timezone": "UTC", "recurring": recurring, "prompt": "prompt",
            "status": "active", "created_at": "2025-12-01T00:00:00Z", "expires_at": null,
            "last_fired_at": null, "fire_count": 0, "cancel_reason": null,
            "jitter_offset_ms": jitter_ms, "last_run_at": null, "last_run_outcome": null,
            "last_run_reason": null, "last_run_error": null, "last_missed_at": null,
            "missed_count": 0, "failed_count": 0, "scheduled_for": scheduled_for,
            "fired_at": null, "missed_at": null
        }))
        .expect("schedule")
    }

    fn file_with(task: Schedule) -> ScheduleFile {
        ScheduleFile {
            version: 1,
            scheduler_owner: None,
            tasks: vec![task],
            extras: serde_json::Map::new(),
        }
    }

    struct MemState {
        file: ScheduleFile,
        updates: Vec<RunUpdate>,
        logs: Vec<RunLogEntry>,
    }

    struct MemPersistence(Mutex<MemState>);

    impl MemPersistence {
        fn new(file: ScheduleFile) -> Self {
            Self(Mutex::new(MemState {
                file,
                updates: Vec::new(),
                logs: Vec::new(),
            }))
        }

        fn lock(&self) -> MutexGuard<'_, MemState> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl SchedulePersistence for MemPersistence {
        fn load(&self) -> PortFuture<'_, ScheduleFile> {
            Box::pin(async { Ok(self.lock().file.clone()) })
        }

        fn apply_update(
            &self,
            schedule_id: &str,
            update: RunUpdate,
            now: Timestamp,
        ) -> PortFuture<'_, ()> {
            let schedule_id = schedule_id.to_owned();
            Box::pin(async move {
                let mut state = self.lock();
                let target = state
                    .file
                    .tasks
                    .iter_mut()
                    .find(|task| task.id.as_str() == schedule_id)
                    .ok_or_else(|| RuntimeError::NotFound {
                        context: "schedule".into(),
                    })?;
                apply_run_update(target, update.clone(), now)?;
                state.updates.push(update);
                Ok(())
            })
        }

        fn append_run_log(&self, entry: &RunLogEntry) -> PortFuture<'_, ()> {
            let entry = entry.clone();
            Box::pin(async move {
                self.lock().logs.push(entry);
                Ok(())
            })
        }
    }

    struct FakeClock(Mutex<Timestamp>);

    impl Clock for FakeClock {
        fn now(&self) -> Timestamp {
            *self.0.lock().unwrap_or_else(PoisonError::into_inner)
        }

        fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
            Timestamp::parse_persisted_rfc3339(value)
        }
    }

    #[derive(Default)]
    struct FixedIds(Mutex<u128>);

    fn unsupported<T>() -> PortFuture<'static, T> {
        Box::pin(async {
            Err(RuntimeError::Unsupported {
                context: "test".into(),
            })
        })
    }

    impl FixedIds {
        fn next_uuid(&self) -> uuid::Uuid {
            let mut counter = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            *counter += 1;
            uuid::Uuid::from_u128(*counter)
        }
    }

    impl IdGenerator for FixedIds {
        fn agent_id(&self) -> PortFuture<'_, AgentId> {
            unsupported()
        }

        fn conversation_id(&self) -> PortFuture<'_, ConversationId> {
            unsupported()
        }

        fn message_id(&self) -> PortFuture<'_, MessageId> {
            unsupported()
        }

        fn run_id(&self) -> PortFuture<'_, RunId> {
            unsupported()
        }

        fn incident_id(&self) -> PortFuture<'_, uuid::Uuid> {
            unsupported()
        }

        fn turn_lifecycle_owner_id(&self) -> PortFuture<'_, uuid::Uuid> {
            let id = self.next_uuid();
            Box::pin(async move { Ok(id) })
        }
    }

    /// Shared fixture state plus a separately owned scheduler handle.
    struct World {
        listener: Arc<Mutex<ListenerRuntime>>,
        persistence: Arc<MemPersistence>,
    }

    fn world(task: Schedule) -> (World, ScheduleScheduler) {
        let clock = Arc::new(FakeClock(Mutex::new(t0())));
        let listener = Arc::new(Mutex::new(ListenerRuntime::new()));
        let persistence = Arc::new(MemPersistence::new(file_with(task)));
        let scheduler = ScheduleScheduler::new(
            clock,
            persistence.clone(),
            listener.clone(),
            Arc::new(FixedIds::default()),
        );
        (
            World {
                listener,
                persistence,
            },
            scheduler,
        )
    }

    fn scope_of(task: &Schedule) -> RuntimeScope {
        RuntimeScope::new(task.agent_id.clone(), task.conversation_id.clone(), None)
    }

    fn lock_listener(listener: &Mutex<ListenerRuntime>) -> MutexGuard<'_, ListenerRuntime> {
        listener.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn handle_for(world: &World, task: &Schedule) -> RuntimeHandle {
        let listener = lock_listener(&world.listener);
        listener
            .lookup(&RuntimeKey::from(&scope_of(task)))
            .expect("target runtime resident")
    }

    fn stored_kinds(world: &World, task: &Schedule) -> Vec<(QueueItemKind, QueueItemSource)> {
        let listener = lock_listener(&world.listener);
        let handle = listener
            .lookup(&RuntimeKey::from(&scope_of(task)))
            .expect("target runtime resident");
        listener
            .queue(&handle)
            .expect("queue")
            .items()
            .map(|item| (item.kind, item.source))
            .collect()
    }

    fn ensure_runtime(world: &World, task: &Schedule) -> RuntimeHandle {
        let mut listener = lock_listener(&world.listener);
        let scope = scope_of(task);
        let key = RuntimeKey::from(&scope);
        if let Some(handle) = listener.lookup(&key) {
            return handle;
        }
        listener
            .get_or_create(&scope, uuid::Uuid::from_u128(5))
            .expect("handle")
    }

    fn fill_queue(world: &World, handle: &RuntimeHandle, count: usize) {
        let mut listener = lock_listener(&world.listener);
        for index in 0..count {
            let item = QueueItem {
                id: non_empty(format!("fill-{index}")),
                client_message_id: non_empty(format!("fill-client-{index}")),
                // Approval results never coalesce, so every fill is retained.
                kind: QueueItemKind::ApprovalResult,
                source: QueueItemSource::System,
                content: BoundedJsonValue::new(serde_json::json!({})).expect("content"),
                enqueued_at: t0(),
                extras: EntityExtras::default(),
            };
            let _mutation = listener.enqueue_retained(handle, item).expect("fill");
        }
    }

    #[tokio::test]
    async fn enqueues_through_queue() {
        let task = schedule(false, 0, Some(t0()));
        let (world, scheduler) = world(task.clone());
        scheduler.tick(t0()).await.expect("tick");
        assert_eq!(
            stored_kinds(&world, &task),
            [(QueueItemKind::CronPrompt, QueueItemSource::Cron)]
        );
        let handle = handle_for(&world, &task);
        {
            let listener = lock_listener(&world.listener);
            assert_eq!(
                listener
                    .lifecycle(&handle)
                    .expect("owner")
                    .projection()
                    .state(),
                TurnStateKind::Idle,
                "lifecycle owner must not be invoked directly"
            );
        }
        // The admission chain recorded the fire: replaying the identical
        // client_message_id resolves to the prior queued disposition.
        let replay = AdmissionRequest {
            item: QueueItem {
                id: non_empty("replay".to_owned()),
                client_message_id: non_empty(format!(
                    "cron-schedule-1-{}",
                    t0().as_utc().timestamp_millis()
                )),
                kind: QueueItemKind::CronPrompt,
                source: QueueItemSource::Cron,
                content: BoundedJsonValue::new(serde_json::json!({})).expect("content"),
                enqueued_at: t0(),
                extras: EntityExtras::default(),
            },
            route: AdmissionRoute::Ordinary,
        };
        let outcome = lock_listener(&world.listener)
            .admit(&handle, replay)
            .expect("replay");
        assert!(matches!(
            outcome,
            AdmissionOutcome::Duplicate(InputDisposition::Queued)
        ));
    }

    #[tokio::test]
    async fn missed() {
        let task = schedule(false, 0, Some(seconds_from(t0(), -360)));
        let (world, scheduler) = world(task.clone());
        scheduler.tick(t0()).await.expect("tick");
        {
            let persisted = world.persistence.lock();
            let stored = &persisted.file.tasks[0];
            assert_eq!(stored.status, ScheduleStatus::Missed);
            assert_eq!(stored.missed_count, Some(1));
            assert_eq!(persisted.updates, [RunUpdate::Missed]);
            assert_eq!(persisted.logs.len(), 1);
            let entry = &persisted.logs[0];
            assert_eq!(entry.outcome, Some(ScheduleRunOutcome::Missed));
            assert_eq!(entry.reason.as_deref(), Some("started_too_late"));
        }
        scheduler.tick(t0()).await.expect("second tick");
        let persisted = world.persistence.lock();
        assert_eq!(
            persisted.file.tasks[0].missed_count,
            Some(1),
            "exactly once"
        );
        assert_eq!(persisted.updates, [RunUpdate::Missed]);
    }

    #[tokio::test]
    async fn cancels_pending_timers() {
        let jittered_now = seconds_from(t0(), 30);

        // Case one: stopping cancels the pending jitter-delayed timer.
        let stopped_task = schedule(true, 30_000, None);
        let (stopped_world, mut stopped_scheduler) = world(stopped_task.clone());
        stopped_scheduler.tick(t0()).await.expect("schedule tick");
        assert!(
            stored_kinds(&stopped_world, &stopped_task).is_empty(),
            "no admission before jitter elapses"
        );
        stopped_scheduler.stop().await;
        stopped_scheduler
            .tick(jittered_now)
            .await
            .expect("post-stop tick");
        assert!(
            stored_kinds(&stopped_world, &stopped_task).is_empty(),
            "stop must leave no late fire"
        );
        assert!(stopped_world.persistence.lock().updates.is_empty());

        // Control: without stop the delayed fire executes on a later tick.
        let control_task = schedule(true, 30_000, None);
        let (control_world, control_scheduler) = world(control_task.clone());
        control_scheduler
            .tick(t0())
            .await
            .expect("control schedule tick");
        control_scheduler
            .tick(jittered_now)
            .await
            .expect("control fire tick");
        assert_eq!(stored_kinds(&control_world, &control_task).len(), 1);

        // Case two: losing the runtime generation cancels the pending timer.
        let lease_task = schedule(true, 30_000, None);
        let (lease_world, lease_scheduler) = world(lease_task.clone());
        lease_scheduler
            .tick(t0())
            .await
            .expect("lease schedule tick");
        let handle = handle_for(&lease_world, &lease_task);
        {
            let mut listener = lock_listener(&lease_world.listener);
            listener
                .set_residency(&handle, RuntimeResidency::new(0, false, 0))
                .expect("evict");
        }
        lease_scheduler
            .tick(jittered_now)
            .await
            .expect("lease-loss tick");
        // The evicted runtime stays gone; recreating it must expose no late
        // fire, and no lifecycle transition may have been recorded.
        let recreated = ensure_runtime(&lease_world, &lease_task);
        let listener = lock_listener(&lease_world.listener);
        assert_eq!(
            listener.queue(&recreated).expect("recreated queue").len(),
            0,
            "lease loss leaves no fire"
        );
        drop(listener);
        assert!(lease_world.persistence.lock().updates.is_empty());
    }

    #[tokio::test]
    async fn outcomes() {
        // A one-shot schedule retires after firing with a queued run-log entry.
        let one_shot = schedule(false, 0, Some(t0()));
        let (one_shot_world, one_shot_scheduler) = world(one_shot.clone());
        one_shot_scheduler.tick(t0()).await.expect("one-shot tick");
        {
            let persisted = one_shot_world.persistence.lock();
            let stored = &persisted.file.tasks[0];
            assert_eq!(stored.status, ScheduleStatus::Fired);
            assert_eq!(stored.fire_count, 1);
            assert!(stored.fired_at.is_some());
            assert_eq!(persisted.logs.len(), 1);
            let entry = &persisted.logs[0];
            assert_eq!(entry.status, Some(RunLogStatus::Ok));
            assert_eq!(entry.outcome, Some(ScheduleRunOutcome::Queued));
            assert_eq!(entry.reason.as_deref(), Some("one_off_due"));
            assert!(entry.queue_item_id.is_some());
        }

        // A recurring schedule stays active and fires each matched minute.
        let recurring = schedule(true, 0, None);
        let (recurring_world, recurring_scheduler) = world(recurring.clone());
        recurring_scheduler
            .tick(t0())
            .await
            .expect("recurring tick");
        recurring_scheduler
            .tick(seconds_from(t0(), 60))
            .await
            .expect("next minute tick");
        {
            let persisted = recurring_world.persistence.lock();
            let stored = &persisted.file.tasks[0];
            assert_eq!(stored.status, ScheduleStatus::Active);
            assert_eq!(stored.fire_count, 2);
            let reasons: Vec<_> = persisted
                .logs
                .iter()
                .filter_map(|entry| entry.reason.clone())
                .collect();
            assert_eq!(
                reasons,
                ["scheduled_time_matched", "scheduled_time_matched"]
            );
        }

        // A rejected fire records a failed run with its error run-log entry.
        let failing = schedule(false, 0, Some(t0()));
        let (failing_world, failing_scheduler) = world(failing.clone());
        let handle = ensure_runtime(&failing_world, &failing);
        fill_queue(
            &failing_world,
            &handle,
            lotta_domain::bounds::QUEUE_ITEMS_HARD_MAX.value,
        );
        failing_scheduler.tick(t0()).await.expect("failing tick");
        let persisted = failing_world.persistence.lock();
        assert_eq!(persisted.file.tasks[0].failed_count, Some(1));
        assert_eq!(persisted.logs.len(), 1);
        let entry = &persisted.logs[0];
        assert_eq!(entry.status, Some(RunLogStatus::Error));
        assert_eq!(entry.outcome, Some(ScheduleRunOutcome::Failed));
        assert_eq!(entry.reason.as_deref(), Some("scheduler_error"));
        assert!(entry.error.is_some());
    }

    fn plain_request(
        number: usize,
        kind: QueueItemKind,
        source: QueueItemSource,
    ) -> AdmissionRequest {
        AdmissionRequest {
            item: QueueItem {
                id: non_empty(format!("source-{number}")),
                client_message_id: non_empty(format!("source-client-{number}")),
                kind,
                source,
                content: BoundedJsonValue::new(serde_json::json!({})).expect("content"),
                enqueued_at: t0(),
                extras: EntityExtras::default(),
            },
            route: AdmissionRoute::Ordinary,
        }
    }

    #[tokio::test]
    async fn sources_regression() {
        let task = schedule(false, 0, Some(t0()));
        let (world, scheduler) = world(task.clone());
        let handle = ensure_runtime(&world, &task);
        let cases = [
            (
                QueueItemKind::TaskNotification,
                QueueItemSource::TaskNotification,
            ),
            (QueueItemKind::CronPrompt, QueueItemSource::Cron),
            (QueueItemKind::ApprovalResult, QueueItemSource::System),
            (QueueItemKind::OverlayAction, QueueItemSource::System),
            (QueueItemKind::ModContinue, QueueItemSource::System),
        ];
        {
            let mut listener = lock_listener(&world.listener);
            for (number, (kind, source)) in cases.iter().enumerate() {
                let outcome = listener
                    .admit(&handle, plain_request(number, *kind, *source))
                    .expect("admit");
                assert!(matches!(outcome, AdmissionOutcome::Queued(_)));
            }
        }
        scheduler.tick(t0()).await.expect("tick");
        let mut expected = cases.to_vec();
        expected.push((QueueItemKind::CronPrompt, QueueItemSource::Cron));
        assert_eq!(
            stored_kinds(&world, &task),
            expected,
            "FIFO preserved alongside a live scheduler"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn tick_loop_runs() {
        let task = schedule(false, 0, Some(t0()));
        let (world, mut scheduler) = world(task.clone());
        scheduler.start();
        assert!(scheduler.is_running());
        // Paused tokio time auto-advances while every task awaits a timer, so
        // the spawned loop's immediate tick runs without wall-clock waits.
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if !stored_kinds(&world, &task).is_empty() {
                break;
            }
        }
        assert_eq!(
            stored_kinds(&world, &task).len(),
            1,
            "spawned loop must fire due schedules"
        );
        scheduler.stop().await;
        assert!(!scheduler.is_running());
    }
}

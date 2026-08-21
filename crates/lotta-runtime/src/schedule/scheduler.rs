//! Lease-aware cron scheduler that fires due schedules through admission.
//!
//! Each bounded tick evaluates the loaded canonical schedule file and enqueues
//! matching `cron_prompt` items through [`ListenerRuntime::admit`], never by
//! starting a turn directly. One-shot schedules past their grace window are
//! marked missed exactly once. Jitter-delayed recurring fires are pending
//! timers cancelled on stop or when the target runtime generation changes.

use crate::RuntimeError;
use crate::admission::{AdmissionOutcome, AdmissionRequest, AdmissionRoute};
use crate::ports::{IdGenerator, SchedulePersistence};
use crate::registry::{ListenerRuntime, RuntimeHandle, RuntimeKey};
use crate::schedule::{RunLogAction, RunLogEntry, RunLogStatus, RunUpdate, ScheduleExpression};
use chrono::{DateTime, Utc};
use lotta_domain::{
    BoundedJsonValue, Clock, EntityExtras, NonEmptyString, QueueItem, QueueItemKind,
    QueueItemSource, RuntimeScope, Schedule, ScheduleRunOutcome, ScheduleStatus, Timestamp,
};
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Baseline scheduler cadence in milliseconds.
pub const TICK_INTERVAL_MS: i64 = 60_000;
/// One-shot grace window past the scheduled instant before a miss is recorded.
pub const MISS_WINDOW_MS: i64 = 5 * MINUTE_MS;
/// Failure summaries are truncated to this many characters before persisting.
pub const FAILURE_SUMMARY_CHARS_MAX: usize = 200;

const MINUTE_MS: i64 = 60_000;
const MINUTE_SECONDS: i64 = 60;
const SECOND_MS: i64 = 1_000;

/// One jitter-delayed fire captured by a tick for a later one.
#[derive(Clone)]
struct PendingFire {
    fire_at_ms: i64,
    occurrence_ms: i64,
    schedule_id: String,
    handle: RuntimeHandle,
}

/// Per-minute deduplication and pending timer state shared with the tick loop.
#[derive(Default)]
struct SchedulerState {
    pending: Vec<PendingFire>,
    fired_minute_key: i64,
    fired_this_minute: HashSet<String>,
}

/// Shared scheduler dependencies owned by both the handle and the tick loop.
struct SchedulerCore {
    persistence: Arc<dyn SchedulePersistence>,
    listener: Arc<Mutex<ListenerRuntime>>,
    ids: Arc<dyn IdGenerator>,
    state: Mutex<SchedulerState>,
    cancellation: CancellationToken,
}

/// Lease-aware scheduler over one canonical schedule file.
///
/// Construct with injected ports, then either [`ScheduleScheduler::start`] the
/// periodic loop or drive [`ScheduleScheduler::tick`] manually under an
/// injected clock. Stopping cancels every pending jitter-delayed timer.
pub struct ScheduleScheduler {
    clock: Arc<dyn Clock + Send + Sync>,
    core: Arc<SchedulerCore>,
    cancellation: CancellationToken,
    task: Option<JoinHandle<()>>,
}

/// Outcome of evaluating one active schedule against the current instant.
enum Evaluation {
    /// Not due inside this tick.
    Idle,
    /// The due window passed without firing.
    Missed,
    /// Enqueue immediately at the intended occurrence.
    Fire(i64),
    /// Enqueue after the persisted jitter delay elapses.
    Delay(i64, i64),
}

impl ScheduleScheduler {
    /// Creates an idle scheduler over the injected persistence and registry.
    #[must_use]
    pub fn new(
        clock: Arc<dyn Clock + Send + Sync>,
        persistence: Arc<dyn SchedulePersistence>,
        listener: Arc<Mutex<ListenerRuntime>>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        let cancellation = CancellationToken::new();
        Self {
            clock,
            core: Arc::new(SchedulerCore {
                persistence,
                listener,
                ids,
                state: Mutex::new(SchedulerState::default()),
                cancellation: cancellation.clone(),
            }),
            cancellation,
            task: None,
        }
    }

    /// Returns whether the periodic tick loop is currently running.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.task.is_some()
    }

    /// Starts the periodic tick loop unless it is already running.
    ///
    /// The loop performs one immediate evaluation and then ticks every
    /// [`TICK_INTERVAL_MS`], reading each instant from the injected clock.
    pub fn start(&mut self) {
        if self.task.is_some() {
            return;
        }
        self.task = Some(spawn_tick_loop(
            self.clock.clone(),
            self.core.clone(),
            self.cancellation.clone(),
        ));
    }

    /// Cancels every pending jitter-delayed timer and joins the tick loop.
    pub async fn stop(&mut self) {
        self.cancellation.cancel();
        lock_state(&self.core.state).pending.clear();
        if let Some(task) = self.task.take() {
            let _joined = task.await;
        }
    }

    /// Evaluates due schedules exactly once at `now`.
    ///
    /// # Errors
    /// Returns a typed failure only when the canonical file cannot be loaded;
    /// per-schedule failures are recorded as failed runs instead.
    pub async fn tick(&self, now: Timestamp) -> Result<(), RuntimeError> {
        self.core.tick(now).await
    }
}

impl Drop for ScheduleScheduler {
    fn drop(&mut self) {
        self.cancellation.cancel();
        lock_state(&self.core.state).pending.clear();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Audited scope-free timer spawn: captures only clock, core, and cancellation.
fn spawn_tick_loop(
    clock: Arc<dyn Clock + Send + Sync>,
    core: Arc<SchedulerCore>,
    cancellation: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(error) = core.tick(clock.now()).await {
                tracing::warn!(error = %error, "scheduler tick failed");
            }
            tokio::select! {
                () = cancellation.cancelled() => return,
                () = tokio::time::sleep(tick_duration()) => {}
            }
        }
    })
}

fn tick_duration() -> std::time::Duration {
    let milliseconds = u64::try_from(TICK_INTERVAL_MS.max(0)).unwrap_or(0);
    std::time::Duration::from_millis(milliseconds)
}

impl SchedulerCore {
    async fn tick(&self, now: Timestamp) -> Result<(), RuntimeError> {
        if self.cancellation.is_cancelled() {
            return Ok(());
        }
        let minute = minute_key(&now);
        self.execute_due(now).await;
        let file = self.persistence.load().await?;
        for task in &file.tasks {
            if task.status != ScheduleStatus::Active {
                continue;
            }
            if let Err(error) = self.process_task(task, now, minute).await {
                self.record_failure(task, &error, now).await;
            }
        }
        Ok(())
    }

    async fn process_task(
        &self,
        task: &Schedule,
        now: Timestamp,
        minute: i64,
    ) -> Result<(), RuntimeError> {
        match evaluate_task(task, &now)? {
            Evaluation::Missed => self.mark_missed(task, now).await,
            Evaluation::Fire(occurrence_ms) => {
                if !self.claim_minute(task, minute) {
                    return Ok(());
                }
                let handle = self.handle_for(task).await?;
                self.fire(task, &handle, now, occurrence_ms).await
            }
            Evaluation::Delay(fire_at_ms, occurrence_ms) => {
                if !self.claim_minute(task, minute) {
                    return Ok(());
                }
                let handle = self.handle_for(task).await?;
                lock_state(&self.state).pending.push(PendingFire {
                    fire_at_ms,
                    occurrence_ms,
                    schedule_id: task.id.as_str().to_owned(),
                    handle,
                });
                Ok(())
            }
            Evaluation::Idle => Ok(()),
        }
    }

    /// Executes every pending timer whose delayed instant has arrived.
    async fn execute_due(&self, now: Timestamp) {
        for pending in drain_due(&self.state, now.as_utc().timestamp_millis()) {
            if self.cancellation.is_cancelled() {
                return;
            }
            self.execute_pending(pending, now).await;
        }
    }

    /// Revalidates lease, liveness, and status before one delayed fire.
    async fn execute_pending(&self, pending: PendingFire, now: Timestamp) {
        if !self.lease_current(&pending.handle) {
            return;
        }
        let Ok(file) = self.persistence.load().await else {
            tracing::warn!(schedule_id = %pending.schedule_id, "delayed fire load failed");
            return;
        };
        let Some(task) = file
            .tasks
            .iter()
            .find(|task| task.id.as_str() == pending.schedule_id)
        else {
            return;
        };
        if task.status != ScheduleStatus::Active {
            return;
        }
        if let Err(error) = self
            .fire(task, &pending.handle, now, pending.occurrence_ms)
            .await
        {
            self.record_failure(task, &error, now).await;
        }
    }

    /// Returns whether the captured runtime generation is still current.
    fn lease_current(&self, captured: &RuntimeHandle) -> bool {
        let listener = lock_listener(&self.listener);
        listener
            .lookup(captured.key())
            .is_some_and(|current| current == *captured)
    }

    /// Resolves or creates the exact runtime scope targeted by a schedule.
    async fn handle_for(&self, task: &Schedule) -> Result<RuntimeHandle, RuntimeError> {
        let scope = RuntimeScope::new(task.agent_id.clone(), task.conversation_id.clone(), None);
        let key = RuntimeKey::from(&scope);
        {
            let listener = lock_listener(&self.listener);
            if let Some(handle) = listener.lookup(&key) {
                return Ok(handle);
            }
        }
        let owner_id = self.ids.turn_lifecycle_owner_id().await?;
        lock_listener(&self.listener).get_or_create(&scope, owner_id)
    }

    /// Enqueues one cron prompt through admission and records the fired run.
    async fn fire(
        &self,
        task: &Schedule,
        handle: &RuntimeHandle,
        now: Timestamp,
        occurrence_ms: i64,
    ) -> Result<(), RuntimeError> {
        let item = cron_queue_item(task, occurrence_ms, now)?;
        let queue_item_id = item.id.as_str().to_owned();
        let outcome = {
            let mut listener = lock_listener(&self.listener);
            listener.admit(
                handle,
                AdmissionRequest {
                    item,
                    route: AdmissionRoute::Ordinary,
                },
            )
        }?;
        if matches!(outcome, AdmissionOutcome::Duplicate(_)) {
            return Ok(());
        }
        if matches!(outcome, AdmissionOutcome::Rejected { .. }) {
            return Err(RuntimeError::LimitExceeded {
                context: "cron_prompt_queue".into(),
            });
        }
        self.apply_and_log(task, RunUpdate::Fired, now, || {
            fired_entry(task, &queue_item_id, now)
        })
        .await
    }

    /// Applies one lifecycle update and appends its canonical run-log entry.
    async fn apply_and_log(
        &self,
        task: &Schedule,
        update: RunUpdate,
        now: Timestamp,
        entry: impl FnOnce() -> RunLogEntry,
    ) -> Result<(), RuntimeError> {
        self.persistence
            .apply_update(task.id.as_str(), update, now)
            .await?;
        self.persistence.append_run_log(&entry()).await
    }

    /// Marks one passed window missed exactly once through CAS persistence.
    async fn mark_missed(&self, task: &Schedule, now: Timestamp) -> Result<(), RuntimeError> {
        self.apply_and_log(task, RunUpdate::Missed, now, || missed_entry(task, now))
            .await
    }

    /// Records one failed attempt; secondary persistence failures warn only.
    async fn record_failure(&self, task: &Schedule, error: &RuntimeError, now: Timestamp) {
        let summary = summarize(error);
        if let Err(update_error) = self
            .persistence
            .apply_update(task.id.as_str(), RunUpdate::Failed(summary.clone()), now)
            .await
        {
            tracing::warn!(
                schedule_id = task.id.as_str(),
                error = %update_error,
                "failed-run transition rejected",
            );
        }
        if let Err(log_error) = self
            .persistence
            .append_run_log(&failed_entry(task, summary, now))
            .await
        {
            tracing::warn!(
                schedule_id = task.id.as_str(),
                error = %log_error,
                "failure run-log append rejected",
            );
        }
    }

    /// Claims the per-minute dedup slot, resetting state on minute rollover.
    fn claim_minute(&self, task: &Schedule, minute: i64) -> bool {
        let mut state = lock_state(&self.state);
        if state.fired_minute_key != minute {
            state.fired_minute_key = minute;
            state.fired_this_minute.clear();
        }
        state.fired_this_minute.insert(task.id.as_str().to_owned())
    }
}

/// Classifies one active schedule against the current instant.
///
/// Recurring schedules match when the cron expression hits the current wall
/// minute in the schedule timezone; their persisted jitter delays enqueue.
/// One-shots compare against their scheduled instant plus jitter, and miss
/// once more than [`MISS_WINDOW_MS`] behind without firing (baseline parity).
///
/// # Errors
/// Returns a typed failure when a recurring expression fails to parse or the
/// cron search horizon is exhausted.
fn evaluate_task(task: &Schedule, now: &Timestamp) -> Result<Evaluation, RuntimeError> {
    let now_ms = now.as_utc().timestamp_millis();
    if !task.recurring {
        return Ok(evaluate_one_shot(task, now_ms));
    }
    let expression = ScheduleExpression::parse(task.cron.as_str())?;
    let Some(minute_start) = matched_minute_start(&expression, now, task.timezone.as_tz())? else {
        return Ok(Evaluation::Idle);
    };
    let occurrence_ms = minute_start * MINUTE_SECONDS * SECOND_MS;
    Ok(if task.jitter_offset_ms > 0 {
        Evaluation::Delay(now_ms.saturating_add(task.jitter_offset_ms), occurrence_ms)
    } else {
        Evaluation::Fire(occurrence_ms)
    })
}

fn evaluate_one_shot(task: &Schedule, now_ms: i64) -> Evaluation {
    let Some(scheduled) = task.scheduled_for else {
        return Evaluation::Idle;
    };
    let scheduled_ms = scheduled.as_utc().timestamp_millis();
    if now_ms > scheduled_ms.saturating_add(MISS_WINDOW_MS) {
        return Evaluation::Missed;
    }
    if scheduled_ms.saturating_add(task.jitter_offset_ms) <= now_ms {
        return Evaluation::Fire(scheduled_ms);
    }
    Evaluation::Idle
}

/// Returns the epoch-second start of the matched minute, if any.
///
/// # Errors
/// Propagates the bounded cron search failure.
fn matched_minute_start(
    expression: &ScheduleExpression,
    now: &Timestamp,
    timezone: chrono_tz::Tz,
) -> Result<Option<i64>, RuntimeError> {
    let minute_start = now.as_utc().timestamp().div_euclid(MINUTE_SECONDS) * MINUTE_SECONDS;
    let search_from = DateTime::<Utc>::from_timestamp(minute_start - MINUTE_SECONDS, 0)
        .ok_or_else(|| RuntimeError::InvalidData {
            context: "schedule tick instant".into(),
        })?;
    let next = expression.next_after(search_from, timezone)?;
    let matched = next.timestamp() <= now.as_utc().timestamp();
    Ok(matched.then_some(minute_start))
}

/// Builds the deterministic canonical queue item for one intended occurrence.
///
/// # Errors
/// Returns a typed failure when derived identifiers or content violate bounds.
fn cron_queue_item(
    task: &Schedule,
    occurrence_ms: i64,
    now: Timestamp,
) -> Result<QueueItem, RuntimeError> {
    let identity = format!("cron-{}-{occurrence_ms}", task.id.as_str());
    let content = BoundedJsonValue::new(json!({
        "prompt": task.prompt.as_str(),
        "schedule_id": task.id.as_str(),
        "name": task.name,
        "occurrence": occurrence_ms,
    }))
    .map_err(|_| RuntimeError::LimitExceeded {
        context: "cron prompt content".into(),
    })?;
    Ok(QueueItem {
        id: non_empty(format!("queue-{identity}"))?,
        client_message_id: non_empty(identity)?,
        kind: QueueItemKind::CronPrompt,
        source: QueueItemSource::Cron,
        content,
        enqueued_at: now,
        extras: EntityExtras::default(),
    })
}

/// Drains and returns every pending timer whose delayed instant has arrived.
fn drain_due(state: &Mutex<SchedulerState>, now_ms: i64) -> Vec<PendingFire> {
    let mut due = Vec::new();
    lock_state(state).pending.retain(|pending| {
        if pending.fire_at_ms <= now_ms {
            due.push(pending.clone());
            return false;
        }
        true
    });
    due
}

/// Returns the epoch-minute index containing `now`.
fn minute_key(now: &Timestamp) -> i64 {
    now.as_utc().timestamp().div_euclid(MINUTE_SECONDS)
}

fn non_empty(value: String) -> Result<NonEmptyString, RuntimeError> {
    NonEmptyString::new(value).map_err(|_| RuntimeError::InvalidData {
        context: "cron prompt identity".into(),
    })
}

fn summarize(error: &RuntimeError) -> String {
    let summary = error.to_string();
    match summary.char_indices().nth(FAILURE_SUMMARY_CHARS_MAX) {
        Some((index, _)) => summary[..index].to_owned(),
        None => summary,
    }
}

fn fired_entry(task: &Schedule, queue_item_id: &str, now: Timestamp) -> RunLogEntry {
    let reason = if task.recurring {
        "scheduled_time_matched"
    } else {
        "one_off_due"
    };
    base_entry(task, now, |entry| {
        entry.status = Some(RunLogStatus::Ok);
        entry.outcome = Some(ScheduleRunOutcome::Queued);
        entry.reason = Some(reason.to_owned());
        entry.queue_item_id = Some(queue_item_id.to_owned());
        entry.fired_at = Some(now.to_string());
        entry.scheduled_for = Some(task.scheduled_for.map(|instant| instant.to_string()));
    })
}

fn missed_entry(task: &Schedule, now: Timestamp) -> RunLogEntry {
    base_entry(task, now, |entry| {
        entry.status = Some(RunLogStatus::Skipped);
        entry.outcome = Some(ScheduleRunOutcome::Missed);
        entry.reason = Some("started_too_late".to_owned());
        entry.summary = Some("missed".to_owned());
        entry.missed_count = Some(1);
        entry.scheduled_for = Some(task.scheduled_for.map(|instant| instant.to_string()));
    })
}

fn failed_entry(task: &Schedule, summary: String, now: Timestamp) -> RunLogEntry {
    base_entry(task, now, |entry| {
        entry.status = Some(RunLogStatus::Error);
        entry.outcome = Some(ScheduleRunOutcome::Failed);
        entry.reason = Some("scheduler_error".to_owned());
        entry.error = Some(summary);
        entry.scheduled_for = Some(task.scheduled_for.map(|instant| instant.to_string()));
    })
}

fn base_entry(
    task: &Schedule,
    now: Timestamp,
    customize: impl FnOnce(&mut RunLogEntry),
) -> RunLogEntry {
    let mut entry = RunLogEntry {
        ts: now.as_utc().timestamp_millis(),
        job_id: task.id.as_str().to_owned(),
        action: RunLogAction::Finished,
        status: None,
        outcome: None,
        reason: None,
        error: None,
        summary: None,
        agent_id: Some(task.agent_id.as_str().to_owned()),
        conversation_id: Some(task.conversation_id.as_str().to_owned()),
        run_id: None,
        run_at_ms: Some(now.as_utc().timestamp_millis()),
        queue_item_id: None,
        scheduled_for: None,
        fired_at: None,
        missed_count: None,
        window_start: None,
        window_end: None,
    };
    customize(&mut entry);
    entry
}

fn lock_state(state: &Mutex<SchedulerState>) -> MutexGuard<'_, SchedulerState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn lock_listener(listener: &Mutex<ListenerRuntime>) -> std::sync::MutexGuard<'_, ListenerRuntime> {
    listener.lock().unwrap_or_else(PoisonError::into_inner)
}

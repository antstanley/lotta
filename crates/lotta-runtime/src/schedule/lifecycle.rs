use lotta_domain::{Schedule, ScheduleCancelReason, ScheduleRunOutcome, ScheduleStatus, Timestamp};

use crate::RuntimeError;

/// One authoritative schedule run transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunUpdate {
    /// Prompt was queued successfully.
    Fired,
    /// Due window was missed.
    Missed,
    /// Attempt failed with a bounded summary.
    Failed(String),
    /// Schedule was cancelled for an exact canonical reason.
    Cancelled(ScheduleCancelReason),
}

/// Applies one lifecycle transition and updates counters and timestamps atomically in memory.
///
/// # Errors
/// Rejects transitions from terminal schedules and counter overflow.
pub fn apply_run_update(
    schedule: &mut Schedule,
    update: RunUpdate,
    now: Timestamp,
) -> Result<(), RuntimeError> {
    if schedule.status != ScheduleStatus::Active {
        return Err(RuntimeError::Conflict {
            context: "terminal schedule transition".into(),
        });
    }
    let mut updated = schedule.clone();
    updated.last_run_at = Some(Some(now));
    match update {
        RunUpdate::Fired => fired(&mut updated, now)?,
        RunUpdate::Missed => missed(&mut updated, now)?,
        RunUpdate::Failed(error) => failed(&mut updated, error)?,
        RunUpdate::Cancelled(reason) => cancelled(&mut updated, reason),
    }
    *schedule = updated;
    Ok(())
}

fn fired(schedule: &mut Schedule, now: Timestamp) -> Result<(), RuntimeError> {
    schedule.fire_count = increment(schedule.fire_count)?;
    schedule.last_fired_at = Some(now);
    schedule.last_run_outcome = Some(Some(ScheduleRunOutcome::Queued));
    schedule.last_run_reason = Some(Some(if schedule.recurring {
        "scheduled_time_matched".into()
    } else {
        "one_off_due".into()
    }));
    schedule.last_run_error = Some(None);
    if !schedule.recurring {
        schedule.status = ScheduleStatus::Fired;
        schedule.fired_at = Some(now);
    }
    Ok(())
}

fn missed(schedule: &mut Schedule, now: Timestamp) -> Result<(), RuntimeError> {
    schedule.missed_count = Some(increment(schedule.missed_count.unwrap_or(0))?);
    schedule.last_missed_at = Some(Some(now));
    schedule.last_run_outcome = Some(Some(ScheduleRunOutcome::Missed));
    schedule.last_run_reason = Some(Some("started_too_late".into()));
    schedule.last_run_error = Some(None);
    if !schedule.recurring {
        schedule.status = ScheduleStatus::Missed;
        schedule.missed_at = Some(now);
    }
    Ok(())
}

fn failed(schedule: &mut Schedule, error: String) -> Result<(), RuntimeError> {
    schedule.failed_count = Some(increment(schedule.failed_count.unwrap_or(0))?);
    schedule.last_run_outcome = Some(Some(ScheduleRunOutcome::Failed));
    schedule.last_run_reason = Some(Some("scheduler_error".into()));
    schedule.last_run_error = Some(Some(error));
    Ok(())
}

fn cancelled(schedule: &mut Schedule, reason: ScheduleCancelReason) {
    schedule.status = ScheduleStatus::Cancelled;
    schedule.cancel_reason = Some(reason);
    schedule.last_run_outcome = Some(Some(ScheduleRunOutcome::Skipped));
    schedule.last_run_reason = Some(Some("task_cancelled".into()));
    schedule.last_run_error = Some(None);
}

fn increment(value: u64) -> Result<u64, RuntimeError> {
    value
        .checked_add(1)
        .ok_or_else(|| RuntimeError::LimitExceeded {
            context: "schedule counter".into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::support::{instant, schedule};
    use lotta_domain::{ScheduleCancelReason, ScheduleStatus, Timestamp};

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_utc(instant(seconds))
    }

    #[test]
    fn active_to_fired() {
        let now = at(100);
        let mut item = schedule(false, "0 0 * * *", "UTC", instant(0));
        apply_run_update(&mut item, RunUpdate::Fired, now).expect("fire");
        assert_eq!(item.status, ScheduleStatus::Fired);
        assert_eq!(item.fire_count, 1);
        assert_eq!(item.fired_at, Some(now));
        assert_eq!(item.last_fired_at, Some(now));
    }

    #[test]
    fn missed() {
        let now = at(100);
        let mut item = schedule(false, "0 0 * * *", "UTC", instant(0));
        apply_run_update(&mut item, RunUpdate::Missed, now).expect("miss");
        assert_eq!(item.status, ScheduleStatus::Missed);
        assert_eq!(item.missed_count, Some(1));
        assert_eq!(item.last_missed_at, Some(Some(now)));
        assert_eq!(item.missed_at, Some(now));
    }

    #[test]
    fn failed_counter() {
        let now = at(100);
        let mut item = schedule(true, "0 0 * * *", "UTC", instant(0));
        apply_run_update(&mut item, RunUpdate::Failed("failure".into()), now).expect("fail");
        assert_eq!(item.status, ScheduleStatus::Active);
        assert_eq!(item.failed_count, Some(1));
    }

    #[test]
    fn cancel_reason() {
        let now = at(100);
        let mut item = schedule(true, "0 0 * * *", "UTC", instant(0));
        apply_run_update(
            &mut item,
            RunUpdate::Cancelled(ScheduleCancelReason::Expired),
            now,
        )
        .expect("cancel");
        assert_eq!(item.status, ScheduleStatus::Cancelled);
        assert_eq!(item.cancel_reason, Some(ScheduleCancelReason::Expired));
    }

    #[test]
    fn overflow_does_not_mutate() {
        let now = at(100);
        let mut item = schedule(true, "0 0 * * *", "UTC", instant(0));
        item.fire_count = u64::MAX;
        let original = item.clone();
        assert!(apply_run_update(&mut item, RunUpdate::Fired, now).is_err());
        assert_eq!(item, original);
    }
}

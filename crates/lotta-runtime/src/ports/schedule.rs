use super::PortFuture;
use crate::schedule::{RunLogEntry, RunUpdate, ScheduleFile};
use lotta_domain::Timestamp;

/// Persists canonical schedules and their append-only run history.
///
/// # Preconditions
/// Schedule identifiers reference records inside the canonical file, updates are
/// lifecycle transitions, and entries carry a `job_id` matching their schedule.
/// Implementations must apply updates under optimistic concurrency so concurrent
/// writers cannot silently overwrite each other.
///
/// # Errors
/// Futures report absence, lifecycle conflicts, stale revisions, limits, and
/// translated storage failures as [`crate::RuntimeError`].
///
/// # Cancellation
/// Scalar operations are atomic under future cancellation; a dropped future
/// leaves the durable file either unchanged or fully updated.
///
/// # Ownership
/// Loads return owned snapshots. Update and append inputs are borrowed only for
/// the returned future's lifetime.
pub trait SchedulePersistence: Send + Sync {
    /// Loads the bounded canonical schedule file.
    fn load(&self) -> PortFuture<'_, ScheduleFile>;

    /// Applies one lifecycle transition to the exact schedule with CAS persistence.
    fn apply_update(
        &self,
        schedule_id: &str,
        update: RunUpdate,
        now: Timestamp,
    ) -> PortFuture<'_, ()>;

    /// Appends one canonical run-log line for the entry's schedule.
    fn append_run_log(&self, entry: &RunLogEntry) -> PortFuture<'_, ()>;
}

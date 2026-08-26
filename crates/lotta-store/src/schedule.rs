//! Secure canonical schedule and append-only run-log persistence.

use crate::adapter::{LOCAL_STORE_BLOCKING_MAX, run_blocking};
use crate::atomic::{FileRevision, WriteMode};
use crate::confinement::{backend_root, create_confined_parent, validate_existing};
use crate::side::{SidePaths, SideRevision};
use crate::{LottaStorageLock, StoreError, StoreErrorKind};
use lotta_domain::Timestamp;
use lotta_runtime::ports::{PortFuture, SchedulePersistence};
pub use lotta_runtime::schedule::{RunLogAction, RunLogEntry, RunLogStatus};
use lotta_runtime::schedule::{RunUpdate, ScheduleFile, apply_run_update, run_log_append_fits};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Maximum bytes in one canonical run-log line.
pub const SCHEDULE_RUN_LOG_LINE_BYTES_MAX: usize = 64 * 1_024;
/// Maximum records accepted while repairing or rotating one run log.
pub const SCHEDULE_RUN_LOG_RECORDS_MAX: usize = 20_000;
const RUN_LOG_READ_MULTIPLIER: usize = 4;

struct ScheduleAtomicObserver;
impl crate::atomic::AtomicObserver for ScheduleAtomicObserver {}

/// Concrete optimistic revision for `crons.json`.
#[derive(Debug)]
pub struct ScheduleRevision(SideRevision);

/// Loaded canonical schedule file and its optimistic revision.
#[derive(Debug)]
pub struct LoadedSchedules {
    /// Fully validated file.
    pub file: ScheduleFile,
    /// Revision required for replacement.
    pub revision: ScheduleRevision,
}

/// Secure concrete schedule store.
pub struct ScheduleStore<'a> {
    paths: &'a SidePaths,
}

impl<'a> ScheduleStore<'a> {
    /// Constructs an adapter using Task 27's exact path authority.
    #[must_use]
    pub const fn new(paths: &'a SidePaths) -> Self {
        Self { paths }
    }

    /// Loads the bounded canonical file with an optimistic revision.
    ///
    /// # Errors
    /// Returns typed path, security, revision, JSON, schema, or bounds failures.
    pub fn load(&self) -> Result<LoadedSchedules, StoreError> {
        let loaded = crate::side::crons::read(self.paths)?;
        let file = ScheduleFile::decode(loaded.bytes()).map_err(|error| runtime_error(&error))?;
        Ok(LoadedSchedules {
            file,
            revision: ScheduleRevision(loaded.revision().clone()),
        })
    }

    /// Atomically saves only if the loaded revision remains current.
    ///
    /// # Errors
    /// Returns typed path, security, revision, JSON, schema, or bounds failures.
    pub fn save(&self, file: &ScheduleFile, revision: &ScheduleRevision) -> Result<(), StoreError> {
        let bytes = file.encode().map_err(|error| runtime_error(&error))?;
        let path = self.paths.crons()?;
        crate::side::write_opaque_expected(&path, &bytes, &revision.0, WriteMode::ProviderAuth)
    }

    /// Applies one lifecycle update to the exact schedule and persists it with CAS.
    ///
    /// # Errors
    /// Returns not-found, lifecycle, overflow, security, I/O, or stale-revision failures.
    pub fn apply_update(
        &self,
        schedule_id: &str,
        expected_revision: &ScheduleRevision,
        update: RunUpdate,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let mut loaded = self.load()?;
        let schedule = loaded
            .file
            .tasks
            .iter_mut()
            .find(|schedule| schedule.id.as_str() == schedule_id)
            .ok_or_else(|| StoreError::new(StoreErrorKind::NotFound, schedule_id))?;
        apply_run_update(schedule, update, now).map_err(|error| runtime_error(&error))?;
        self.save(&loaded.file, expected_revision)
    }
}

/// Owning adapter exposing the canonical schedule file, CAS lifecycle
/// transitions, and the append-only run log through the runtime's
/// [`SchedulePersistence`] port.
pub struct ScheduleService {
    paths: Arc<SidePaths>,
    pool: Arc<Semaphore>,
}

impl ScheduleService {
    /// Constructs the production-bound adapter over shared side paths.
    #[must_use]
    pub fn new(paths: Arc<SidePaths>) -> Self {
        Self {
            paths,
            pool: Arc::new(Semaphore::new(LOCAL_STORE_BLOCKING_MAX)),
        }
    }
}

impl SchedulePersistence for ScheduleService {
    fn load(&self) -> PortFuture<'_, ScheduleFile> {
        let paths = self.paths.clone();
        let pool = self.pool.clone();
        Box::pin(async move {
            let loaded = run_blocking(pool, move || {
                ScheduleStore::new(&paths).load().map(|loaded| loaded.file)
            })
            .await?;
            Ok(loaded)
        })
    }

    fn apply_update(
        &self,
        schedule_id: &str,
        update: RunUpdate,
        now: Timestamp,
    ) -> PortFuture<'_, ()> {
        let paths = self.paths.clone();
        let pool = self.pool.clone();
        let schedule_id = schedule_id.to_owned();
        Box::pin(async move {
            run_blocking(pool, move || {
                let revision = ScheduleStore::new(&paths).load()?.revision;
                ScheduleStore::new(&paths).apply_update(&schedule_id, &revision, update, now)
            })
            .await?;
            Ok(())
        })
    }

    fn append_run_log(&self, entry: &RunLogEntry) -> PortFuture<'_, ()> {
        let paths = self.paths.clone();
        let pool = self.pool.clone();
        let entry = entry.clone();
        Box::pin(async move {
            run_blocking(pool, move || {
                RunLogStore::new(&paths).append(entry.job_id.as_str(), &entry)
            })
            .await?;
            Ok(())
        })
    }
}

/// Secure append-and-rotate run-log adapter.
pub struct RunLogStore<'a> {
    paths: &'a SidePaths,
    keep_lines: usize,
    bytes_max: usize,
}

impl<'a> RunLogStore<'a> {
    /// Constructs the production-bound adapter.
    #[must_use]
    pub const fn new(paths: &'a SidePaths) -> Self {
        Self {
            paths,
            keep_lines: lotta_domain::bounds::SCHEDULE_RUN_LOG_KEEP_LINES.value,
            bytes_max: lotta_domain::bounds::SCHEDULE_RUN_LOG_BYTES_MAX.value,
        }
    }

    /// Constructs explicit bounds for tests and constrained deployments.
    #[must_use]
    pub const fn with_bounds(paths: &'a SidePaths, keep_lines: usize, bytes_max: usize) -> Self {
        Self {
            paths,
            keep_lines,
            bytes_max,
        }
    }

    /// Appends one complete canonical line or atomically rotates when either bound would exceed.
    ///
    /// # Errors
    /// Rejects invalid IDs, mismatched records, symlinks/special files, malformed
    /// existing lines, line/record/byte bounds, lock conflicts, and I/O failures.
    pub fn append(&self, schedule_id: &str, entry: &RunLogEntry) -> Result<(), StoreError> {
        let line = encode_entry(schedule_id, entry, self.bytes_max)?;
        let path = self.paths.run_log(schedule_id)?;
        let root = backend_root(&path)?;
        let lock = LottaStorageLock::try_acquire_confined(root)?;
        let parent = path.parent().ok_or_else(|| invalid(&path))?;
        create_confined_parent(root, parent)?;
        apply_secure_directory(parent)?;
        let existing = read_existing(root, &path, read_limit(self.bytes_max)?)?;
        let parsed = parse_lines(&existing, schedule_id, TailPolicy::RepairPartial)?;
        if existing.ends_with(b"\n")
            && run_log_append_fits(
                existing.len(),
                parsed.len(),
                line.len(),
                self.keep_lines,
                self.bytes_max,
            )
        {
            return append_sync(root, &path, &line, &lock);
        }
        let rotated = rotate(parsed, &line, self.keep_lines, self.bytes_max)?;
        let expected = FileRevision::sample_bounded(
            root,
            &path,
            read_limit(self.bytes_max)? as u64,
            &ScheduleAtomicObserver,
        )?;
        crate::atomic::atomic_write_expected_locked(
            &path,
            &rotated,
            WriteMode::ProviderAuth,
            &expected,
            &lock,
        )
    }

    /// Reads strictly validated complete canonical records.
    ///
    /// # Errors
    /// Rejects symlinks/special files, malformed or partial lines, and bounds.
    pub fn read(&self, schedule_id: &str) -> Result<Vec<RunLogEntry>, StoreError> {
        let path = self.paths.run_log(schedule_id)?;
        let root = backend_root(&path)?;
        let bytes = read_existing(root, &path, self.bytes_max)?;
        parse_lines(&bytes, schedule_id, TailPolicy::RejectPartial)
            .map(|lines| lines.into_iter().map(|line| line.entry).collect())
    }
}

#[derive(Clone, Copy)]
enum TailPolicy {
    RepairPartial,
    RejectPartial,
}

struct ParsedLine {
    bytes: Vec<u8>,
    entry: RunLogEntry,
}

fn encode_entry(
    schedule_id: &str,
    entry: &RunLogEntry,
    bytes_max: usize,
) -> Result<Vec<u8>, StoreError> {
    if entry.job_id != schedule_id {
        return Err(invalid("run-log job id"));
    }
    let mut line = serde_json::to_vec(entry).map_err(|_| parse("run-log entry"))?;
    line.push(b'\n');
    if line.len() > SCHEDULE_RUN_LOG_LINE_BYTES_MAX || line.len() > bytes_max {
        return Err(limit("run-log line"));
    }
    Ok(line)
}

fn read_existing(root: &Path, path: &Path, maximum: usize) -> Result<Vec<u8>, StoreError> {
    validate_existing(root, path)?;
    let metadata = match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(StoreError::from_io(path, &error)),
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid(path));
    }
    if metadata.len() > maximum as u64 {
        return Err(limit(path));
    }
    let mut file = File::open(path).map_err(|error| StoreError::from_io(path, &error))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| StoreError::from_io(path, &error))?;
    Ok(bytes)
}

fn parse_lines(
    bytes: &[u8],
    schedule_id: &str,
    tail: TailPolicy,
) -> Result<Vec<ParsedLine>, StoreError> {
    let complete_end = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    if complete_end != bytes.len() && matches!(tail, TailPolicy::RejectPartial) {
        return Err(parse("partial run-log line"));
    }
    let mut parsed = Vec::new();
    for raw in bytes[..complete_end]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if parsed.len() >= SCHEDULE_RUN_LOG_RECORDS_MAX
            || raw.len() + 1 > SCHEDULE_RUN_LOG_LINE_BYTES_MAX
        {
            return Err(limit("run-log records"));
        }
        let entry: RunLogEntry = serde_json::from_slice(raw).map_err(|_| parse("run-log line"))?;
        if entry.job_id != schedule_id {
            return Err(parse("run-log job id"));
        }
        let mut line = raw.to_vec();
        line.push(b'\n');
        parsed.push(ParsedLine { bytes: line, entry });
    }
    Ok(parsed)
}

fn rotate(
    mut lines: Vec<ParsedLine>,
    new_line: &[u8],
    keep_lines: usize,
    bytes_max: usize,
) -> Result<Vec<u8>, StoreError> {
    lines.push(ParsedLine {
        entry: serde_json::from_slice(&new_line[..new_line.len() - 1])
            .map_err(|_| parse("run-log entry"))?,
        bytes: new_line.to_vec(),
    });
    let mut start = lines.len().saturating_sub(keep_lines);
    let mut total = lines[start..]
        .iter()
        .map(|line| line.bytes.len())
        .sum::<usize>();
    while total > bytes_max && start < lines.len() {
        total = total.saturating_sub(lines[start].bytes.len());
        start += 1;
    }
    if start == lines.len() {
        return Err(limit("run-log rotation"));
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(total)
        .map_err(|_| limit("run-log rotation"))?;
    for line in &lines[start..] {
        output.extend_from_slice(&line.bytes);
    }
    Ok(output)
}

fn append_sync(
    root: &Path,
    path: &Path,
    line: &[u8],
    lock: &LottaStorageLock,
) -> Result<(), StoreError> {
    if !lock.guards_root(root) {
        return Err(StoreError::new(StoreErrorKind::LottaLock, path));
    }
    validate_existing(root, path)?;
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    configure_secure_create(&mut options);
    let mut file = options
        .open(path)
        .map_err(|error| StoreError::from_io(path, &error))?;
    let metadata = file
        .metadata()
        .map_err(|error| StoreError::from_io(path, &error))?;
    if !metadata.is_file() {
        return Err(invalid(path));
    }
    apply_secure_file(&file, path)?;
    file.write_all(line)
        .map_err(|error| StoreError::from_io(path, &error))?;
    file.flush()
        .map_err(|error| StoreError::from_io(path, &error))?;
    file.sync_all()
        .map_err(|error| StoreError::from_io(path, &error))
}

fn read_limit(bytes_max: usize) -> Result<usize, StoreError> {
    bytes_max
        .checked_mul(RUN_LOG_READ_MULTIPLIER)
        .ok_or_else(|| limit("run-log read"))
}

fn runtime_error(error: &lotta_runtime::RuntimeError) -> StoreError {
    let kind = match error {
        lotta_runtime::RuntimeError::LimitExceeded { .. } => StoreErrorKind::Limit,
        _ => StoreErrorKind::Parse,
    };
    StoreError::new(kind, "schedule")
}

fn invalid(path: impl Into<std::path::PathBuf>) -> StoreError {
    StoreError::new(StoreErrorKind::InvalidPath, path)
}

fn parse(path: impl Into<std::path::PathBuf>) -> StoreError {
    StoreError::new(StoreErrorKind::Parse, path)
}

fn limit(path: impl Into<std::path::PathBuf>) -> StoreError {
    StoreError::new(StoreErrorKind::Limit, path)
}

#[cfg(unix)]
fn configure_secure_create(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt as _;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn configure_secure_create(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn apply_secure_directory(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| StoreError::from_io(path, &error))
}

#[cfg(not(unix))]
fn apply_secure_directory(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

#[cfg(unix)]
fn apply_secure_file(file: &File, path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| StoreError::from_io(path, &error))
}

#[cfg(not(unix))]
fn apply_secure_file(_file: &File, _path: &Path) -> Result<(), StoreError> {
    Ok(())
}

#[cfg(test)]
mod test_support {
    use super::{RunLogAction, RunLogEntry, RunLogStatus, ScheduleFile};
    use crate::side::SidePaths;
    use lotta_domain::Timestamp;
    use lotta_testkit::fixtures::FixtureLoader;
    use lotta_testkit::roots::TemporaryRoot;

    pub(crate) const CRON_ID: &str = "store-schedule";
    pub(crate) const LOG_ID: &str = "log-schedule";

    pub(crate) struct Harness {
        pub(crate) root: TemporaryRoot,
        pub(crate) paths: SidePaths,
    }

    impl Harness {
        pub(crate) fn new(label: &str) -> Self {
            let root = TemporaryRoot::new(label).expect("temporary root");
            let home = root.path().join("home");
            let paths = SidePaths::new(&home, None, []).expect("paths");
            Self { root, paths }
        }
    }

    pub(crate) fn fixture(relative: &str) -> Vec<u8> {
        FixtureLoader::new()
            .load_bytes(format!("persistence/side_stores/{relative}"))
            .expect("fixture bytes")
    }

    pub(crate) fn seed_crons(paths: &SidePaths, bytes: &[u8]) {
        let path = paths.crons().expect("crons path");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        std::fs::write(path, bytes).expect("seed crons");
    }

    pub(crate) fn seed_schedule(paths: &SidePaths, id: &str, recurring: bool) {
        let schedule = test_schedule(id, recurring);
        let file = ScheduleFile {
            version: 1,
            scheduler_owner: None,
            tasks: vec![schedule],
            extras: serde_json::Map::new(),
        };
        seed_crons(paths, &file.encode().expect("encode seed"));
    }

    fn test_schedule(id: &str, recurring: bool) -> lotta_domain::Schedule {
        serde_json::from_value(serde_json::json!({
            "id":id, "agent_id":"agent-local-fixture", "conversation_id":"conversation",
            "name":"name", "description":"description", "cron":"0 0 * * *",
            "timezone":"UTC", "recurring":recurring, "prompt":"prompt", "status":"active",
            "created_at":"2026-01-01T00:00:00Z", "expires_at":null, "last_fired_at":null,
            "fire_count":0, "cancel_reason":null, "jitter_offset_ms":0,
            "last_run_at":null, "last_run_outcome":null, "last_run_reason":null,
            "last_run_error":null, "last_missed_at":null, "missed_count":0, "failed_count":0,
            "scheduled_for":null, "fired_at":null, "missed_at":null
        }))
        .expect("schedule")
    }

    pub(crate) fn at(seconds: i64) -> Timestamp {
        use chrono::TimeZone as _;
        Timestamp::from_utc(
            chrono::Utc
                .timestamp_opt(seconds, 0)
                .single()
                .expect("test time"),
        )
    }

    pub(crate) fn log_entry(ts: i64, marker: &str) -> RunLogEntry {
        entry_for(LOG_ID, ts, marker)
    }

    pub(crate) fn entry_for(job_id: &str, ts: i64, marker: &str) -> RunLogEntry {
        RunLogEntry {
            ts,
            job_id: job_id.into(),
            action: RunLogAction::Finished,
            status: Some(RunLogStatus::Ok),
            outcome: None,
            reason: None,
            error: None,
            summary: Some(marker.into()),
            agent_id: None,
            conversation_id: None,
            run_id: None,
            run_at_ms: None,
            queue_item_id: None,
            scheduled_for: None,
            fired_at: None,
            missed_count: None,
            window_start: None,
            window_end: None,
        }
    }
}

#[cfg(test)]
mod store_round_trip {
    use super::ScheduleStore;
    use super::test_support::{Harness, fixture, seed_crons};
    use crate::StoreErrorKind;

    #[test]
    fn fixture_file_round_trips_through_store_with_open_extras() {
        let harness = Harness::new("schedule-store-roundtrip");
        seed_crons(&harness.paths, &fixture("letta_home/crons.json"));
        let store = ScheduleStore::new(&harness.paths);
        let mut loaded = store.load().expect("load");
        assert_eq!(loaded.file.tasks.len(), 1);
        assert_eq!(loaded.file.tasks[0].id.as_str(), "schedule-fixture");
        loaded
            .file
            .extras
            .insert("root_extension".into(), serde_json::json!({"future": true}));
        loaded.file.tasks[0]
            .extras
            .insert("schedule_extension".into(), serde_json::json!([1, 2, 3]));
        let expected = loaded.file.clone();
        store.save(&loaded.file, &loaded.revision).expect("save");
        let reloaded = ScheduleStore::new(&harness.paths).load().expect("reload");
        assert_eq!(reloaded.file, expected);
        assert!(reloaded.file.extras.contains_key("root_extension"));
        assert!(
            reloaded.file.tasks[0]
                .extras
                .contains_key("schedule_extension")
        );
    }

    #[test]
    fn stale_revision_save_conflicts_and_preserves_winner_bytes() {
        let harness = Harness::new("schedule-store-stale-save");
        seed_crons(&harness.paths, &fixture("letta_home/crons.json"));
        let store = ScheduleStore::new(&harness.paths);
        let first = store.load().expect("first load");
        let mut second = store.load().expect("second load");
        second.file.tasks[0].fire_count = 7;
        store
            .save(&second.file, &second.revision)
            .expect("winner save");
        let winner = std::fs::read(harness.paths.crons().expect("path")).expect("snapshot");
        let error = store
            .save(&first.file, &first.revision)
            .expect_err("stale save");
        assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
        assert_eq!(
            std::fs::read(harness.paths.crons().expect("path")).expect("after"),
            winner
        );
        assert_eq!(store.load().expect("final").file.tasks[0].fire_count, 7);
    }

    #[test]
    fn duplicate_schedule_ids_are_rejected_on_load() {
        let harness = Harness::new("schedule-store-duplicate-id");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fixture("letta_home/crons.json")).expect("fixture");
        let task = value["tasks"][0].clone();
        value["tasks"].as_array_mut().expect("tasks").push(task);
        let bytes = serde_json::to_vec_pretty(&value).expect("encode");
        seed_crons(&harness.paths, &bytes);
        let error = ScheduleStore::new(&harness.paths)
            .load()
            .expect_err("duplicate ids");
        assert_eq!(error.kind(), StoreErrorKind::Parse);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_crons_file_is_rejected() {
        let harness = Harness::new("schedule-store-symlink");
        let outside = harness.root.path().join("outside-crons.json");
        std::fs::write(&outside, b"secret").expect("outside bytes");
        let target = harness.paths.crons().expect("crons path");
        std::fs::create_dir_all(target.parent().expect("parent")).expect("parent");
        std::os::unix::fs::symlink(&outside, &target).expect("symlink");
        let error = ScheduleStore::new(&harness.paths)
            .load()
            .expect_err("symlink read");
        assert_eq!(error.kind(), StoreErrorKind::InvalidPath);
        assert_eq!(std::fs::read(&outside).expect("outside intact"), b"secret");
    }
}

#[cfg(test)]
mod lifecycle {
    use super::RunUpdate;
    use super::ScheduleStore;
    use super::test_support::{CRON_ID, Harness, at, seed_schedule};
    use crate::StoreErrorKind;
    use lotta_domain::{ScheduleCancelReason, ScheduleRunOutcome, ScheduleStatus};

    #[test]
    fn fired_persists_counters_timestamps_and_restart() {
        let harness = Harness::new("schedule-lifecycle-fired");
        seed_schedule(&harness.paths, CRON_ID, false);
        let store = ScheduleStore::new(&harness.paths);
        let now = at(1_767_225_600);
        let loaded = store.load().expect("load");
        store
            .apply_update(CRON_ID, &loaded.revision, RunUpdate::Fired, now)
            .expect("apply fired");
        let after = store.load().expect("reload");
        let fired = &after.file.tasks[0];
        assert_eq!(fired.status, ScheduleStatus::Fired);
        assert_eq!(fired.fire_count, 1);
        assert_eq!(fired.fired_at, Some(now));
        assert_eq!(fired.last_fired_at, Some(now));
        assert_eq!(
            fired.last_run_outcome,
            Some(Some(ScheduleRunOutcome::Queued))
        );
        let restarted = ScheduleStore::new(&harness.paths);
        assert_eq!(restarted.load().expect("restart").file, after.file);
    }

    #[test]
    fn missed_then_failed_counters_persist_and_restart() {
        let harness = Harness::new("schedule-lifecycle-missed-failed");
        seed_schedule(&harness.paths, CRON_ID, true);
        let store = ScheduleStore::new(&harness.paths);
        let missed_at = at(1_767_225_600);
        let loaded = store.load().expect("load");
        store
            .apply_update(CRON_ID, &loaded.revision, RunUpdate::Missed, missed_at)
            .expect("apply missed");
        let failed_at = at(1_767_226_200);
        let reloaded = store.load().expect("reload");
        store
            .apply_update(
                CRON_ID,
                &reloaded.revision,
                RunUpdate::Failed("boom".into()),
                failed_at,
            )
            .expect("apply failed");
        let after = store.load().expect("final load");
        let task = &after.file.tasks[0];
        assert_eq!(task.status, ScheduleStatus::Active);
        assert_eq!(task.missed_count, Some(1));
        assert_eq!(task.last_missed_at, Some(Some(missed_at)));
        assert_eq!(task.failed_count, Some(1));
        assert_eq!(task.last_run_error, Some(Some("boom".into())));
        let restarted = ScheduleStore::new(&harness.paths);
        assert_eq!(restarted.load().expect("restart").file, after.file);
    }

    #[test]
    fn cancelled_records_reason_and_survives_restart() {
        let harness = Harness::new("schedule-lifecycle-cancelled");
        seed_schedule(&harness.paths, CRON_ID, true);
        let store = ScheduleStore::new(&harness.paths);
        let now = at(1_767_225_600);
        let loaded = store.load().expect("load");
        store
            .apply_update(
                CRON_ID,
                &loaded.revision,
                RunUpdate::Cancelled(ScheduleCancelReason::Expired),
                now,
            )
            .expect("apply cancelled");
        let after = store.load().expect("reload");
        let cancelled = &after.file.tasks[0];
        assert_eq!(cancelled.status, ScheduleStatus::Cancelled);
        assert_eq!(cancelled.cancel_reason, Some(ScheduleCancelReason::Expired));
        assert_eq!(
            cancelled.last_run_outcome,
            Some(Some(ScheduleRunOutcome::Skipped))
        );
        let restarted = ScheduleStore::new(&harness.paths);
        assert_eq!(restarted.load().expect("restart").file, after.file);
    }

    #[test]
    fn two_writer_stale_revision_conflict_leaves_file_unchanged() {
        let harness = Harness::new("schedule-lifecycle-two-writer");
        seed_schedule(&harness.paths, CRON_ID, true);
        let store = ScheduleStore::new(&harness.paths);
        let stale = store.load().expect("stale loader");
        let current = store.load().expect("current loader");
        store
            .apply_update(
                CRON_ID,
                &current.revision,
                RunUpdate::Fired,
                at(1_767_225_600),
            )
            .expect("winner apply");
        let winner = std::fs::read(harness.paths.crons().expect("path")).expect("snapshot");
        let error = store
            .apply_update(
                CRON_ID,
                &stale.revision,
                RunUpdate::Missed,
                at(1_767_226_200),
            )
            .expect_err("stale writer");
        assert_eq!(error.kind(), StoreErrorKind::StorageConflict);
        assert_eq!(
            std::fs::read(harness.paths.crons().expect("path")).expect("after"),
            winner
        );
        let final_state = store.load().expect("final");
        assert_eq!(final_state.file.tasks[0].fire_count, 1);
        assert_eq!(final_state.file.tasks[0].missed_count, Some(0));
    }
}

#[cfg(test)]
mod run_log {
    use super::RunLogStore;
    use super::SCHEDULE_RUN_LOG_LINE_BYTES_MAX;
    use super::test_support::{Harness, LOG_ID, entry_for, fixture, log_entry};
    use crate::StoreErrorKind;
    use lotta_domain::ScheduleRunOutcome;

    const TEST_KEEP_LINES: usize = 3;
    const ROOMY_BYTES_MAX: usize = 100_000;

    fn bounded(harness: &Harness) -> RunLogStore<'_> {
        RunLogStore::with_bounds(&harness.paths, TEST_KEEP_LINES, ROOMY_BYTES_MAX)
    }

    fn summaries(entries: &[super::RunLogEntry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| entry.summary.as_deref().expect("summary"))
            .collect()
    }

    #[test]
    fn line_bound_rotates_from_below_to_above() {
        let harness = Harness::new("run-log-line-bound");
        let store = bounded(&harness);
        store
            .append(LOG_ID, &log_entry(1, "one"))
            .expect("below one");
        store
            .append(LOG_ID, &log_entry(2, "two"))
            .expect("below two");
        assert_eq!(
            summaries(&store.read(LOG_ID).expect("read")),
            ["one", "two"]
        );
        store
            .append(LOG_ID, &log_entry(3, "three"))
            .expect("at bound");
        assert_eq!(
            summaries(&store.read(LOG_ID).expect("read at bound")),
            ["one", "two", "three"]
        );
        store
            .append(LOG_ID, &log_entry(4, "four"))
            .expect("above rotates");
        assert_eq!(
            summaries(&store.read(LOG_ID).expect("rotated")),
            ["two", "three", "four"]
        );
    }

    #[test]
    fn byte_bound_rotates_to_newest_complete_record() {
        let harness = Harness::new("run-log-byte-bound");
        let first = log_entry(1, "xxxxxxxxxxxxxxxxxxxxxxxx");
        let line_bytes_max = serde_json::to_vec(&first).expect("line").len() + 1;
        let store = RunLogStore::with_bounds(&harness.paths, 100, line_bytes_max);
        store.append(LOG_ID, &first).expect("append at byte bound");
        assert_eq!(store.read(LOG_ID).expect("read"), vec![first]);
        let second = log_entry(2, "yyyyyyyyyyyyyyyyyyyyyyyy");
        store
            .append(LOG_ID, &second)
            .expect("rotate above byte bound");
        assert_eq!(store.read(LOG_ID).expect("rotated"), vec![second]);
    }

    #[test]
    fn true_append_preserves_prefix_within_bounds() {
        let harness = Harness::new("run-log-prefix");
        let store = RunLogStore::with_bounds(&harness.paths, 10, ROOMY_BYTES_MAX);
        for ts in 1..=3_i64 {
            store
                .append(LOG_ID, &log_entry(ts, &format!("m{ts}")))
                .expect("append");
        }
        let entries = store.read(LOG_ID).expect("read");
        assert_eq!(summaries(&entries), ["m1", "m2", "m3"]);
        let raw = std::fs::read(harness.paths.run_log(LOG_ID).expect("path")).expect("raw");
        let first_line = serde_json::to_vec(&log_entry(1, "m1")).expect("line");
        assert!(raw.starts_with(&first_line));
        assert_eq!(*raw.last().expect("newline"), b'\n');
    }

    #[test]
    fn real_default_line_bound_keeps_newest_two_thousand() {
        let harness = Harness::new("run-log-default-lines");
        let store = RunLogStore::with_bounds(
            &harness.paths,
            lotta_domain::bounds::SCHEDULE_RUN_LOG_KEEP_LINES.value,
            200_000_000,
        );
        for ts in 1..=2_001_i64 {
            store
                .append(LOG_ID, &log_entry(ts, &format!("m{ts}")))
                .expect("append");
        }
        let entries = store.read(LOG_ID).expect("read");
        assert_eq!(entries.len(), 2_000);
        assert_eq!(entries.first().map(|entry| entry.ts), Some(2));
        assert_eq!(entries.last().map(|entry| entry.ts), Some(2_001));
    }

    #[test]
    fn concurrent_writers_each_record_exactly_once() {
        let harness = Harness::new("run-log-concurrent");
        std::thread::scope(|scope| {
            for index in 0..40_u64 {
                let harness = &harness;
                scope.spawn(move || {
                    let store = RunLogStore::with_bounds(&harness.paths, 1_000, 1_000_000);
                    append_until_accepted(&store, index);
                });
            }
        });
        let store = RunLogStore::with_bounds(&harness.paths, 1_000, 1_000_000);
        let entries = store.read(LOG_ID).expect("read");
        let mut stamps: Vec<i64> = entries.iter().map(|entry| entry.ts).collect();
        stamps.sort_unstable();
        let expected: Vec<i64> = (0..40_i64).collect();
        assert_eq!(stamps, expected);
        assert_eq!(entries.len(), 40);
    }

    fn append_until_accepted(store: &RunLogStore<'_>, index: u64) {
        let entry = log_entry(
            i64::try_from(index).expect("index"),
            &format!("writer-{index}"),
        );
        for attempt in 0..20_000_u32 {
            match store.append(LOG_ID, &entry) {
                Ok(()) => return,
                Err(error)
                    if matches!(
                        error.kind(),
                        StoreErrorKind::LottaLock | StoreErrorKind::StorageConflict
                    ) =>
                {
                    if attempt % 100 == 0 {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    } else {
                        std::thread::yield_now();
                    }
                }
                Err(error) => panic!("append rejected: {error:?}"),
            }
        }
        panic!("writer {index} never accepted");
    }

    #[test]
    fn strict_read_rejects_malformed_complete_lines() {
        let harness = Harness::new("run-log-malformed");
        let store = bounded(&harness);
        store.append(LOG_ID, &log_entry(1, "good")).expect("append");
        append_raw(&harness, b"{broken}\n");
        assert_eq!(
            store.read(LOG_ID).expect_err("broken line").kind(),
            StoreErrorKind::Parse
        );
        let other = Harness::new("run-log-wrong-job");
        let other_store = bounded(&other);
        other_store
            .append(LOG_ID, &log_entry(1, "good"))
            .expect("append");
        append_raw(
            &other,
            b"{\"ts\":9,\"jobId\":\"intruder\",\"action\":\"finished\"}\n",
        );
        assert_eq!(
            other_store.read(LOG_ID).expect_err("wrong job id").kind(),
            StoreErrorKind::Parse
        );
    }

    #[test]
    fn partial_tail_rejected_then_repaired_by_append() {
        let harness = Harness::new("run-log-partial-tail");
        let store = bounded(&harness);
        store
            .append(LOG_ID, &log_entry(1, "first"))
            .expect("append");
        append_raw(&harness, b"{\"ts\":9,\"jobId\":\"log-schedu");
        assert_eq!(
            store.read(LOG_ID).expect_err("partial tail").kind(),
            StoreErrorKind::Parse
        );
        store
            .append(LOG_ID, &log_entry(2, "second"))
            .expect("repairing append");
        assert_eq!(
            summaries(&store.read(LOG_ID).expect("repaired read")),
            ["first", "second"]
        );
    }

    fn append_raw(harness: &Harness, bytes: &[u8]) {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(harness.paths.run_log(LOG_ID).expect("path"))
            .expect("open run log");
        file.write_all(bytes).expect("raw write");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_and_directory_run_logs_are_rejected() {
        let harness = Harness::new("run-log-not-regular");
        let path = harness.paths.run_log(LOG_ID).expect("path");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("runs dir");
        let outside = harness.root.path().join("outside-run.jsonl");
        std::fs::write(&outside, b"outside").expect("outside bytes");
        std::os::unix::fs::symlink(&outside, &path).expect("symlink");
        let store = bounded(&harness);
        assert_eq!(
            store.read(LOG_ID).expect_err("symlink read").kind(),
            StoreErrorKind::InvalidPath
        );
        assert_eq!(
            store
                .append(LOG_ID, &log_entry(1, "x"))
                .expect_err("symlink append")
                .kind(),
            StoreErrorKind::InvalidPath
        );
        std::fs::remove_file(&path).expect("remove link");
        std::fs::create_dir(&path).expect("directory target");
        assert_eq!(
            store.read(LOG_ID).expect_err("directory read").kind(),
            StoreErrorKind::InvalidPath
        );
        assert_eq!(
            store
                .append(LOG_ID, &log_entry(2, "y"))
                .expect_err("directory append")
                .kind(),
            StoreErrorKind::InvalidPath
        );
        assert_eq!(std::fs::read(&outside).expect("outside intact"), b"outside");
    }

    #[cfg(unix)]
    #[test]
    fn secure_modes_cover_file_and_parent() {
        use std::os::unix::fs::PermissionsExt as _;

        let harness = Harness::new("run-log-modes");
        bounded(&harness)
            .append(LOG_ID, &log_entry(1, "only"))
            .expect("append");
        let path = harness.paths.run_log(LOG_ID).expect("path");
        let file_mode = std::fs::metadata(&path).expect("meta").permissions().mode();
        let parent_mode = std::fs::metadata(path.parent().expect("parent"))
            .expect("dir meta")
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o777, 0o600);
        assert_eq!(parent_mode & 0o777, 0o700);
    }

    #[test]
    fn oversized_single_records_rejected_without_mutation() {
        let harness = Harness::new("run-log-oversized-bytes");
        let store = RunLogStore::with_bounds(&harness.paths, 10, 96);
        store.append(LOG_ID, &log_entry(1, "small")).expect("seed");
        let oversized = log_entry(2, &"o".repeat(64));
        assert_eq!(
            store
                .append(LOG_ID, &oversized)
                .expect_err("byte bound")
                .kind(),
            StoreErrorKind::Limit
        );
        assert_eq!(
            summaries(&store.read(LOG_ID).expect("unchanged")),
            ["small"]
        );
        let roomy = Harness::new("run-log-oversized-line");
        let roomy_store = RunLogStore::with_bounds(&roomy.paths, 10, 8_000_000);
        let huge = log_entry(1, "h".repeat(SCHEDULE_RUN_LOG_LINE_BYTES_MAX).as_str());
        assert_eq!(
            roomy_store
                .append(LOG_ID, &huge)
                .expect_err("line bound")
                .kind(),
            StoreErrorKind::Limit
        );
    }

    #[test]
    fn fixture_log_appends_and_survives_restart() {
        let harness = Harness::new("run-log-fixture-restart");
        let path = harness.paths.run_log("schedule-fixture").expect("path");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("runs dir");
        std::fs::write(&path, fixture("letta_home/runs/schedule-fixture.jsonl"))
            .expect("seed fixture run log");
        let store = RunLogStore::new(&harness.paths);
        let follow_up = entry_for("schedule-fixture", 946_684_800_001, "post-restart");
        store
            .append("schedule-fixture", &follow_up)
            .expect("append");
        let first_pass = store.read("schedule-fixture").expect("read");
        let restarted = RunLogStore::new(&harness.paths);
        assert_eq!(
            restarted.read("schedule-fixture").expect("reread"),
            first_pass
        );
        assert_eq!(first_pass.len(), 2);
        assert_eq!(
            first_pass[0].summary.as_deref(),
            Some("SANITIZED_FIXTURE_RUN")
        );
        assert_eq!(first_pass[0].outcome, Some(ScheduleRunOutcome::Queued));
        assert_eq!(first_pass[1], follow_up);
    }
}

#[cfg(test)]
mod scheduler_service {
    use super::test_support::{Harness, at, seed_crons};
    use super::{RunLogStore, ScheduleService};
    use lotta_domain::{
        AgentId, Clock, ConversationId, DomainError, MessageId, QueueItemKind, RunId, RuntimeScope,
        ScheduleRunOutcome, Timestamp,
    };
    use lotta_runtime::ListenerRuntime;
    use lotta_runtime::ports::{IdGenerator, PortFuture, SchedulePersistence};
    use lotta_runtime::schedule::{ScheduleFile, ScheduleScheduler};
    use std::sync::{Arc, Mutex};

    struct FixedClock(Timestamp);

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            self.0
        }

        fn parse_timestamp(&self, value: &str) -> Result<Timestamp, DomainError> {
            Timestamp::parse_persisted_rfc3339(value)
        }
    }

    struct FixedIds;

    fn unsupported<T>() -> PortFuture<'static, T> {
        Box::pin(async {
            Err(lotta_runtime::RuntimeError::Unsupported {
                context: "scheduler_service test".into(),
            })
        })
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
            Box::pin(async { Ok(uuid::Uuid::from_u128(11)) })
        }
    }

    fn recurring_seed() -> Vec<u8> {
        let file = serde_json::from_value::<ScheduleFile>(serde_json::json!({
            "version": 1, "scheduler_owner": null,
            "tasks": [{
                "id":"service-schedule", "agent_id":"agent-local-fixture",
                "conversation_id":"conversation", "name":"name", "description":"description",
                "cron":"* * * * *", "timezone":"UTC", "recurring":true, "prompt":"prompt",
                "status":"active", "created_at":"2026-01-01T00:00:00Z", "expires_at":null,
                "last_fired_at":null, "fire_count":0, "cancel_reason":null,
                "jitter_offset_ms":0, "last_run_at":null, "last_run_outcome":null,
                "last_run_reason":null, "last_run_error":null, "last_missed_at":null,
                "missed_count":0, "failed_count":0, "scheduled_for":null,
                "fired_at":null, "missed_at":null
            }]
        }))
        .expect("seed file");
        file.encode().expect("seed encode")
    }

    #[tokio::test]
    async fn fires_due_recurring_through_real_stores() {
        let harness = Harness::new("schedule-scheduler-service");
        seed_crons(&harness.paths, &recurring_seed());
        let service = Arc::new(ScheduleService::new(Arc::new(harness.paths.clone())));
        let listener = Arc::new(Mutex::new(ListenerRuntime::new()));
        let now = at(1_767_225_600);
        let scheduler = ScheduleScheduler::new(
            Arc::new(FixedClock(now)),
            service.clone(),
            listener.clone(),
            Arc::new(FixedIds),
        );
        scheduler.tick(now).await.expect("tick");
        let persisted = service.load().await.expect("reload");
        assert_eq!(persisted.tasks[0].fire_count, 1);
        assert_eq!(
            persisted.tasks[0].last_run_reason,
            Some(Some("scheduled_time_matched".into()))
        );
        let entries = RunLogStore::new(&harness.paths)
            .read("service-schedule")
            .expect("run log");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].outcome, Some(ScheduleRunOutcome::Queued));
        let target = &persisted.tasks[0];
        let scope = RuntimeScope::new(
            target.agent_id.clone(),
            target.conversation_id.clone(),
            None,
        );
        let listener = listener.lock().expect("listener");
        let handle = listener
            .lookup(&lotta_runtime::registry::RuntimeKey::from(&scope))
            .expect("scheduler-created runtime");
        let stored: Vec<_> = listener
            .queue(&handle)
            .expect("queue")
            .lock()
            .expect("queue lock")
            .items()
            .map(|item| item.kind)
            .collect();
        assert_eq!(stored, [QueueItemKind::CronPrompt]);
    }
}

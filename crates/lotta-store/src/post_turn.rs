//! Durable bounded post-turn job queue.

use crate::atomic::{FileRevision, atomic_write_expected_locked};
use crate::confinement::{backend_root, validate_existing, validate_regular_file};
use crate::{LocalStore, LottaStorageLock, StoreError, StoreErrorKind, WriteMode};
use lotta_domain::RuntimeScope;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Maximum jobs retained in the post-turn queue.
pub const POST_TURN_JOBS_MAX: usize = 2_048;
/// Maximum serialized post-turn queue size, in bytes.
pub const POST_TURN_JOBS_BYTES_MAX: usize = 1_048_576;
/// Maximum execution attempts before stale or failed work becomes terminal.
pub const POST_TURN_JOB_ATTEMPTS_MAX: u32 = 3;
/// Maximum completed jobs retained for diagnostics and idempotence.
pub const POST_TURN_COMPLETED_JOBS_MAX: usize = 512;
const POST_TURN_QUEUE_SCHEMA_VERSION: u8 = 1;
const POST_TURN_FILE_NAME: &str = "post-turn-jobs.json";

/// Kind of durable work performed after a terminal turn outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostTurnJobKind {
    /// Generate or apply a reflection through an injected capability.
    Reflection,
    /// Push committed memory through an injected capability.
    MemoryPush,
}

/// Durable lifecycle state of a post-turn job.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostTurnJobState {
    /// Available for execution.
    Pending,
    /// Exclusively claimed by one runner.
    Running,
    /// Successfully completed and immutable.
    Done,
    /// Exhausted attempts and immutable.
    Failed,
}

/// Canonical durable identity for one post-turn action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PostTurnJobKey {
    /// Runtime scope owning the terminal turn.
    pub scope: RuntimeScope,
    /// Durable turn generation.
    pub turn_generation: u64,
    /// Action to perform.
    pub kind: PostTurnJobKind,
}

/// One durable post-turn queue entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PostTurnJob {
    /// Canonical scope, turn, and kind identity.
    pub key: PostTurnJobKey,
    /// Current durable state.
    pub state: PostTurnJobState,
    /// Per-job compare-and-swap revision.
    pub revision: u64,
    /// Number of execution claims made.
    pub attempts: u32,
}

/// Compare token returned with an exclusive claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostTurnClaim {
    /// Claimed job snapshot.
    pub job: PostTurnJob,
}

/// Result of executing a claimed job through a capability port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostTurnExecution {
    /// Capability completed successfully.
    Complete,
    /// Capability is not currently registered; leave the job pending without consuming an attempt.
    Unavailable,
}

/// Reflection capability used by [`PostTurnJobRunner`].
pub trait ReflectionJob: Send + Sync {
    /// Executes reflection for one durable job.
    ///
    /// # Errors
    /// Returns a scrubbed execution failure. The runner records a retry or terminal failure.
    fn reflect(&self, job: &PostTurnJob) -> Result<PostTurnExecution, StoreError>;
}

/// Memory-push capability used by [`PostTurnJobRunner`].
pub trait MemoryPushJob: Send + Sync {
    /// Executes a memory push for one durable job.
    ///
    /// # Errors
    /// Returns a scrubbed execution failure. The runner records a retry or terminal failure.
    fn push_memory(&self, job: &PostTurnJob) -> Result<PostTurnExecution, StoreError>;
}

/// Capability-injected executor for durable post-turn jobs.
pub struct PostTurnJobRunner<'a> {
    queue: &'a PostTurnQueue,
    reflection: &'a dyn ReflectionJob,
    memory_push: &'a dyn MemoryPushJob,
}

impl<'a> PostTurnJobRunner<'a> {
    /// Creates a runner over one durable queue and its two capability ports.
    #[must_use]
    pub const fn new(
        queue: &'a PostTurnQueue,
        reflection: &'a dyn ReflectionJob,
        memory_push: &'a dyn MemoryPushJob,
    ) -> Self {
        Self {
            queue,
            reflection,
            memory_push,
        }
    }

    /// Recovers stale claims and drains every currently executable job in stable order.
    ///
    /// Unavailable capabilities return their claims to pending without consuming an attempt.
    /// Execution failures become pending while retries remain, then terminal `Failed`.
    ///
    /// # Errors
    /// Returns durable queue failures. Capability failures are durably recorded and draining
    /// continues.
    pub fn drain(&self) -> Result<(), StoreError> {
        self.queue.recover_stale()?;
        let available = self
            .queue
            .jobs()?
            .iter()
            .filter(|job| {
                matches!(
                    job.state,
                    PostTurnJobState::Pending | PostTurnJobState::Running
                )
            })
            .count();
        for _ in 0..available {
            let Some(claim) = self.queue.claim_next()? else {
                break;
            };
            let result = match claim.job.key.kind {
                PostTurnJobKind::Reflection => self.reflection.reflect(&claim.job),
                PostTurnJobKind::MemoryPush => self.memory_push.push_memory(&claim.job),
            };
            match result {
                Ok(PostTurnExecution::Complete) => self.queue.complete(&claim)?,
                Ok(PostTurnExecution::Unavailable) => self.queue.retry_unavailable(&claim)?,
                Err(_) => self.queue.fail(&claim)?,
            }
        }
        Ok(())
    }
}

/// Durable queue stored at `runtime/post-turn-jobs.json`.
#[derive(Clone, Debug)]
pub struct PostTurnQueue {
    root: PathBuf,
    path: PathBuf,
}

impl PostTurnQueue {
    /// Creates the canonical queue for a local store.
    #[must_use]
    pub fn new(store: &LocalStore) -> Self {
        Self {
            root: store.paths().root().to_path_buf(),
            path: store.paths().runtime().join(POST_TURN_FILE_NAME),
        }
    }

    /// Returns the canonical queue path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Idempotently enqueues reflection and memory-push jobs for one terminal turn.
    ///
    /// # Errors
    /// Returns confinement, lock, parse, limit, conflict, or durable-write failures.
    pub fn enqueue_turn(
        &self,
        scope: &RuntimeScope,
        turn_generation: u64,
    ) -> Result<(), StoreError> {
        self.transaction(|queue| {
            let before = queue.jobs.len();
            for kind in [PostTurnJobKind::Reflection, PostTurnJobKind::MemoryPush] {
                let key = PostTurnJobKey {
                    scope: scope.clone(),
                    turn_generation,
                    kind,
                };
                if !queue.jobs.iter().any(|job| job.key == key) {
                    queue.jobs.push(PostTurnJob {
                        key,
                        state: PostTurnJobState::Pending,
                        revision: 1,
                        attempts: 0,
                    });
                }
            }
            queue
                .jobs
                .sort_by(|left, right| compare_keys(&left.key, &right.key));
            Ok(((), queue.jobs.len() != before))
        })
    }

    /// Claims the next pending job in canonical key order.
    ///
    /// # Errors
    /// Returns confinement, lock, parse, conflict, or durable-write failures.
    pub fn claim_next(&self) -> Result<Option<PostTurnClaim>, StoreError> {
        self.transaction(|queue| {
            let Some(job) = queue
                .jobs
                .iter_mut()
                .find(|job| job.state == PostTurnJobState::Pending)
            else {
                return Ok((None, false));
            };
            job.state = PostTurnJobState::Running;
            job.attempts = job.attempts.saturating_add(1);
            job.revision = job.revision.saturating_add(1);
            Ok((Some(PostTurnClaim { job: job.clone() }), true))
        })
    }

    /// Marks an exact running claim successful.
    ///
    /// # Errors
    /// Returns a conflict for stale claims or a durable queue failure.
    pub fn complete(&self, claim: &PostTurnClaim) -> Result<(), StoreError> {
        self.finish_claim(claim, Finish::Complete)
    }

    /// Records an exact running claim's execution failure, retrying while attempts remain.
    ///
    /// # Errors
    /// Returns a conflict for stale claims or a durable queue failure.
    pub fn fail(&self, claim: &PostTurnClaim) -> Result<(), StoreError> {
        self.finish_claim(claim, Finish::Failure)
    }

    /// Returns an unavailable exact claim to pending without consuming an attempt.
    ///
    /// # Errors
    /// Returns a conflict for stale claims or a durable queue failure.
    pub fn retry_unavailable(&self, claim: &PostTurnClaim) -> Result<(), StoreError> {
        self.finish_claim(claim, Finish::Unavailable)
    }

    /// Deterministically recovers stale `Running` jobs after startup or runner restart.
    ///
    /// Claims with attempts remaining return to pending; exhausted claims become terminal failed.
    ///
    /// # Errors
    /// Returns confinement, lock, parse, conflict, or durable-write failures.
    pub fn recover_stale(&self) -> Result<(), StoreError> {
        self.transaction(|queue| {
            let mut changed = false;
            for job in &mut queue.jobs {
                if job.state == PostTurnJobState::Running {
                    job.state = if job.attempts >= POST_TURN_JOB_ATTEMPTS_MAX {
                        PostTurnJobState::Failed
                    } else {
                        PostTurnJobState::Pending
                    };
                    job.revision = job.revision.saturating_add(1);
                    changed = true;
                }
            }
            Ok(((), changed))
        })
    }

    /// Returns a bounded snapshot in canonical order.
    ///
    /// # Errors
    /// Returns confinement, lock, parse, or size failures.
    pub fn jobs(&self) -> Result<Vec<PostTurnJob>, StoreError> {
        let _lock = LottaStorageLock::try_acquire(&self.root)?;
        Ok(read_queue(&self.root, &self.path)?.0.jobs)
    }

    fn finish_claim(&self, claim: &PostTurnClaim, finish: Finish) -> Result<(), StoreError> {
        self.transaction(|queue| {
            let job = queue
                .jobs
                .iter_mut()
                .find(|job| job.key == claim.job.key)
                .ok_or_else(|| conflict(&self.path))?;
            if job.state != PostTurnJobState::Running
                || job.revision != claim.job.revision
                || job.attempts != claim.job.attempts
            {
                return Err(conflict(&self.path));
            }
            match finish {
                Finish::Complete => job.state = PostTurnJobState::Done,
                Finish::Failure => {
                    job.state = if job.attempts >= POST_TURN_JOB_ATTEMPTS_MAX {
                        PostTurnJobState::Failed
                    } else {
                        PostTurnJobState::Pending
                    };
                }
                Finish::Unavailable => {
                    job.attempts = job.attempts.saturating_sub(1);
                    job.state = PostTurnJobState::Pending;
                }
            }
            job.revision = job.revision.saturating_add(1);
            Ok(((), true))
        })
    }

    fn transaction<T>(
        &self,
        mutate: impl FnOnce(&mut QueueFile) -> Result<(T, bool), StoreError>,
    ) -> Result<T, StoreError> {
        let lock = LottaStorageLock::try_acquire(&self.root)?;
        let (mut queue, source) = read_queue(&self.root, &self.path)?;
        let (output, changed) = mutate(&mut queue)?;
        if changed {
            queue.revision = queue.revision.saturating_add(1);
            compact_completed(&mut queue.jobs);
            write_queue(&self.path, &queue, &source, &lock)?;
        }
        Ok(output)
    }
}

#[derive(Clone, Copy)]
enum Finish {
    Complete,
    Failure,
    Unavailable,
}

#[derive(Default, Deserialize, Serialize)]
struct QueueFile {
    #[serde(default = "schema_version")]
    schema_version: u8,
    revision: u64,
    jobs: Vec<PostTurnJob>,
}

const fn schema_version() -> u8 {
    POST_TURN_QUEUE_SCHEMA_VERSION
}

fn read_queue(root: &Path, path: &Path) -> Result<(QueueFile, FileRevision), StoreError> {
    if backend_root(path)? != root {
        return Err(StoreError::new(StoreErrorKind::InvalidPath, path));
    }
    validate_existing(root, path)?;
    let revision = FileRevision::sample_path_bounded(path, POST_TURN_JOBS_BYTES_MAX as u64)?;
    if !revision.exists() {
        return Ok((
            QueueFile {
                schema_version: POST_TURN_QUEUE_SCHEMA_VERSION,
                ..QueueFile::default()
            },
            revision,
        ));
    }
    validate_regular_file(root, path)?;
    let file = std::fs::File::open(path).map_err(|error| StoreError::from_io(path, &error))?;
    let mut bytes = Vec::new();
    file.take((POST_TURN_JOBS_BYTES_MAX + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| StoreError::from_io(path, &error))?;
    if bytes.len() > POST_TURN_JOBS_BYTES_MAX {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    let queue: QueueFile =
        serde_json::from_slice(&bytes).map_err(|_| StoreError::new(StoreErrorKind::Parse, path))?;
    validate_queue(&queue, path)?;
    Ok((queue, revision))
}

fn validate_queue(queue: &QueueFile, path: &Path) -> Result<(), StoreError> {
    if queue.schema_version != POST_TURN_QUEUE_SCHEMA_VERSION
        || queue.jobs.len() > POST_TURN_JOBS_MAX
    {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    if queue
        .jobs
        .windows(2)
        .any(|pair| compare_keys(&pair[0].key, &pair[1].key).is_ge())
        || queue.jobs.iter().any(|job| job.revision == 0)
    {
        return Err(StoreError::new(StoreErrorKind::Parse, path));
    }
    Ok(())
}

fn write_queue(
    path: &Path,
    queue: &QueueFile,
    revision: &FileRevision,
    lock: &LottaStorageLock,
) -> Result<(), StoreError> {
    if queue.jobs.len() > POST_TURN_JOBS_MAX {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    let mut bytes = Vec::new();
    serde_json::to_writer(&mut BoundedWriter::new(&mut bytes), queue)
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    bytes
        .write_all(b"\n")
        .map_err(|_| StoreError::new(StoreErrorKind::Limit, path))?;
    if bytes.len() > POST_TURN_JOBS_BYTES_MAX {
        return Err(StoreError::new(StoreErrorKind::Limit, path));
    }
    atomic_write_expected_locked(path, &bytes, WriteMode::ProviderAuth, revision, lock)
}

fn compact_completed(jobs: &mut Vec<PostTurnJob>) {
    let completed = jobs
        .iter()
        .filter(|job| matches!(job.state, PostTurnJobState::Done | PostTurnJobState::Failed))
        .count();
    let mut remove = completed.saturating_sub(POST_TURN_COMPLETED_JOBS_MAX);
    if remove == 0 {
        return;
    }
    jobs.retain(|job| {
        if remove > 0 && matches!(job.state, PostTurnJobState::Done | PostTurnJobState::Failed) {
            remove -= 1;
            false
        } else {
            true
        }
    });
}

struct BoundedWriter<'a> {
    bytes: &'a mut Vec<u8>,
}

impl<'a> BoundedWriter<'a> {
    const fn new(bytes: &'a mut Vec<u8>) -> Self {
        Self { bytes }
    }
}

impl Write for BoundedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > POST_TURN_JOBS_BYTES_MAX {
            return Err(std::io::Error::other("post-turn queue limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn compare_keys(left: &PostTurnJobKey, right: &PostTurnJobKey) -> std::cmp::Ordering {
    left.scope
        .agent_id
        .as_str()
        .cmp(right.scope.agent_id.as_str())
        .then_with(|| {
            left.scope
                .conversation_id
                .as_str()
                .cmp(right.scope.conversation_id.as_str())
        })
        .then_with(|| left.turn_generation.cmp(&right.turn_generation))
        .then_with(|| left.kind.cmp(&right.kind))
}

fn conflict(path: &Path) -> StoreError {
    StoreError::new(StoreErrorKind::StorageConflict, path)
}

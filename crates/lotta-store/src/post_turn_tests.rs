use crate::post_turn::*;
use crate::{LocalStore, StoreError, StoreErrorKind, StorePaths};
use lotta_domain::{AgentId, ConversationId, RuntimeScope};
use lotta_testkit::roots::TemporaryRoot;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

fn post_turn_fixture(label: &str) -> (TemporaryRoot, LocalStore, RuntimeScope) {
    let root = TemporaryRoot::new(label).expect("temporary root");
    let store = LocalStore::new(StorePaths::new(root.path().join("backend")).expect("paths"));
    let scope = RuntimeScope {
        agent_id: AgentId::accept("agent-post-turn").expect("agent"),
        conversation_id: ConversationId::accept("conversation-post-turn").expect("conversation"),
        acting_user_id: None,
    };
    (root, store, scope)
}

#[test]
fn post_turn_duplicate_enqueue_is_idempotent() {
    let (_root, store, scope) = post_turn_fixture("post-turn-idempotent");
    let queue = PostTurnQueue::new(&store);
    queue.enqueue_turn(&scope, 7).expect("enqueue");
    queue.enqueue_turn(&scope, 7).expect("duplicate");
    assert_eq!(queue.jobs().expect("jobs").len(), 2);
}

#[test]
fn post_turn_concurrent_enqueue_4x100_has_no_loss() {
    let (_root, store, scope) = post_turn_fixture("post-turn-concurrent-enqueue");
    let queue = Arc::new(PostTurnQueue::new(&store));
    let mut threads = Vec::new();
    for shard in 0_u64..4 {
        let queue = Arc::clone(&queue);
        let scope = scope.clone();
        threads.push(std::thread::spawn(move || {
            for turn in 0_u64..100 {
                loop {
                    match queue.enqueue_turn(&scope, shard * 100 + turn) {
                        Ok(()) => break,
                        Err(error) if error.kind() == StoreErrorKind::LottaLock => {
                            std::thread::yield_now();
                        }
                        Err(error) => panic!("enqueue: {error}"),
                    }
                }
            }
        }));
    }
    for thread in threads {
        thread.join().expect("join");
    }
    assert_eq!(queue.jobs().expect("jobs").len(), 800);
}

#[test]
fn post_turn_claim_competition_claims_exactly_once() {
    let (_root, store, scope) = post_turn_fixture("post-turn-claim-competition");
    let queue = Arc::new(PostTurnQueue::new(&store));
    for turn in 0_u64..100 {
        queue.enqueue_turn(&scope, turn).expect("enqueue");
    }
    let claimed = Arc::new(Mutex::new(BTreeSet::new()));
    let mut threads = Vec::new();
    for _ in 0..4 {
        let queue = Arc::clone(&queue);
        let claimed = Arc::clone(&claimed);
        threads.push(std::thread::spawn(move || {
            loop {
                match queue.claim_next() {
                    Ok(Some(claim)) => {
                        claimed
                            .lock()
                            .expect("claimed lock")
                            .insert((claim.job.key.turn_generation, claim.job.key.kind));
                        loop {
                            match queue.complete(&claim) {
                                Ok(()) => break,
                                Err(error) if error.kind() == StoreErrorKind::LottaLock => {
                                    std::thread::yield_now();
                                }
                                Err(error) => panic!("complete: {error}"),
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(error) if error.kind() == StoreErrorKind::LottaLock => {
                        std::thread::yield_now();
                    }
                    Err(error) => panic!("claim: {error}"),
                }
            }
        }));
    }
    for thread in threads {
        thread.join().expect("join");
    }
    assert_eq!(claimed.lock().expect("claimed lock").len(), 200);
}

#[test]
fn post_turn_restart_recovers_stale_running() {
    let (_root, store, scope) = post_turn_fixture("post-turn-stale");
    let queue = PostTurnQueue::new(&store);
    queue.enqueue_turn(&scope, 1).expect("enqueue");
    let stale = queue.claim_next().expect("claim").expect("job");
    assert_eq!(stale.job.attempts, 1);
    let restarted = PostTurnQueue::new(&store);
    restarted.recover_stale().expect("recover");
    let recovered = restarted.claim_next().expect("claim").expect("job");
    assert_eq!(recovered.job.key, stale.job.key);
    assert_eq!(recovered.job.attempts, 2);
}

#[cfg(unix)]
#[test]
fn post_turn_security_modes_and_symlink_rejection() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let (root, store, scope) = post_turn_fixture("post-turn-security");
    let queue = PostTurnQueue::new(&store);
    queue.enqueue_turn(&scope, 1).expect("enqueue");
    assert_eq!(
        std::fs::metadata(queue.path()).expect("file").mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(queue.path().parent().expect("parent"))
            .expect("parent metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    std::fs::remove_file(queue.path()).expect("remove");
    let outside = root.path().join("outside.json");
    std::fs::write(&outside, b"{}\n").expect("outside");
    std::os::unix::fs::symlink(&outside, queue.path()).expect("symlink");
    assert_eq!(
        queue.enqueue_turn(&scope, 2).expect_err("reject").kind(),
        StoreErrorKind::InvalidPath
    );
}

#[test]
fn post_turn_bounded_read_rejects_oversize() {
    let (_root, store, _scope) = post_turn_fixture("post-turn-bounds");
    let queue = PostTurnQueue::new(&store);
    std::fs::create_dir_all(queue.path().parent().expect("parent")).expect("directory");
    std::fs::write(queue.path(), vec![b'x'; POST_TURN_JOBS_BYTES_MAX + 1]).expect("oversize");
    assert_eq!(
        queue.jobs().expect_err("limit").kind(),
        StoreErrorKind::Limit
    );
}

struct Success(Arc<Mutex<Vec<PostTurnJobKind>>>);
impl ReflectionJob for Success {
    fn reflect(&self, _: &PostTurnJob) -> Result<PostTurnExecution, StoreError> {
        self.0
            .lock()
            .expect("calls")
            .push(PostTurnJobKind::Reflection);
        Ok(PostTurnExecution::Complete)
    }
}
impl MemoryPushJob for Success {
    fn push_memory(&self, _: &PostTurnJob) -> Result<PostTurnExecution, StoreError> {
        self.0
            .lock()
            .expect("calls")
            .push(PostTurnJobKind::MemoryPush);
        Ok(PostTurnExecution::Complete)
    }
}

struct Fail;
impl ReflectionJob for Fail {
    fn reflect(&self, job: &PostTurnJob) -> Result<PostTurnExecution, StoreError> {
        Err(StoreError::new(
            StoreErrorKind::Io,
            format!("attempt-{}", job.attempts),
        ))
    }
}
impl MemoryPushJob for Fail {
    fn push_memory(&self, _: &PostTurnJob) -> Result<PostTurnExecution, StoreError> {
        Ok(PostTurnExecution::Unavailable)
    }
}

#[test]
fn post_turn_failure_retries_then_becomes_terminal() {
    let (_root, store, scope) = post_turn_fixture("post-turn-failure-retry");
    let queue = PostTurnQueue::new(&store);
    queue.enqueue_turn(&scope, 1).expect("enqueue");
    for _ in 0..POST_TURN_JOB_ATTEMPTS_MAX {
        PostTurnJobRunner::new(&queue, &Fail, &Fail)
            .drain()
            .expect("drain");
    }
    let jobs = queue.jobs().expect("jobs");
    let reflection = jobs
        .iter()
        .find(|job| job.key.kind == PostTurnJobKind::Reflection)
        .expect("reflection");
    assert_eq!(reflection.state, PostTurnJobState::Failed);
    assert_eq!(reflection.attempts, POST_TURN_JOB_ATTEMPTS_MAX);
    let memory = jobs
        .iter()
        .find(|job| job.key.kind == PostTurnJobKind::MemoryPush)
        .expect("memory");
    assert_eq!(memory.state, PostTurnJobState::Pending);
    assert_eq!(memory.attempts, 0);
}

#[test]
fn post_turn_success_executes_both_kinds() {
    let (mut root, store, scope) = post_turn_fixture("post-turn-success");
    let queue = PostTurnQueue::new(&store);
    queue.enqueue_turn(&scope, 1).expect("enqueue");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let success = Success(Arc::clone(&calls));
    PostTurnJobRunner::new(&queue, &success, &success)
        .drain()
        .expect("drain");
    assert_eq!(calls.lock().expect("calls").len(), 2);
    assert!(
        queue
            .jobs()
            .expect("jobs")
            .iter()
            .all(|job| job.state == PostTurnJobState::Done)
    );
    drop(success);
    drop(calls);
    drop(queue);
    drop(store);
    root.cleanup().expect("remove temporary root");
}

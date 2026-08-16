use super::{ConversationWorktreeContext, ProcessLiveness, WorktreeManager, WorktreeOwner};
use lotta_testkit::fakes::FakeChildProcess;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lotta-task39-lock-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join(".letta/worktrees/test/.git")).expect("dirs");
        Self(path.canonicalize().expect("root"))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Live(Mutex<BTreeSet<(u32, String)>>);
impl ProcessLiveness for Live {
    fn is_alive(&self, pid: u32, nonce: &str) -> bool {
        self.0.lock().expect("live").contains(&(pid, nonce.into()))
    }
}
fn owner(id: &str, pid: u32) -> WorktreeOwner {
    WorktreeOwner::new(
        "agent".into(),
        id.into(),
        pid,
        "host".into(),
        format!("nonce-{pid}"),
    )
    .expect("owner")
}
fn manager(root: &Root, owner: WorktreeOwner, live: Arc<Live>) -> Arc<WorktreeManager> {
    let context = Arc::new(ConversationWorktreeContext::new(&root.0, &root.0).expect("context"));
    Arc::new(
        WorktreeManager::new(
            &root.0,
            Arc::new(FakeChildProcess::default()),
            live,
            owner,
            context,
        )
        .expect("manager"),
    )
}
fn fixture() -> (Root, Arc<Live>, PathBuf) {
    let root = Root::new();
    let path = root
        .0
        .join(".letta/worktrees/test")
        .canonicalize()
        .expect("worktree");
    let live = Arc::new(Live(Mutex::new(BTreeSet::from([
        (1, "nonce-1".into()),
        (2, "nonce-2".into()),
    ]))));
    (root, live, path)
}

#[test]
fn second_enter_blocked() {
    let (root, live, path) = fixture();
    let first = manager(&root, owner("one", 1), Arc::clone(&live));
    let second = manager(&root, owner("two", 2), live);
    first.acquire(&path, false).expect("first");
    assert!(second.acquire(&path, false).is_err());
}
#[test]
fn enter_after_exit_succeeds() {
    let (root, live, path) = fixture();
    let first = manager(&root, owner("one", 1), Arc::clone(&live));
    first.acquire(&path, false).expect("first");
    first.acquire(&path, false).expect("same owner");
    first.release(&path).expect("exit");
    manager(&root, owner("two", 2), live)
        .acquire(&path, false)
        .expect("second");
}
#[test]
fn independent_manager_race_exactly_one() {
    let (root, live, path) = fixture();
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for (id, pid) in [("one", 1), ("two", 2)] {
        let manager = manager(&root, owner(id, pid), Arc::clone(&live));
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            manager.acquire(&path, false).is_ok()
        }));
    }
    barrier.wait();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| thread.join().expect("thread"))
            .filter(|acquired| *acquired)
            .count(),
        1
    );
}
#[test]
fn wrong_owner_release_and_stale_force() {
    let (root, live, path) = fixture();
    let first = manager(&root, owner("one", 1), Arc::clone(&live));
    let second = manager(&root, owner("two", 2), Arc::clone(&live));
    first.acquire(&path, false).expect("first");
    assert!(second.release(&path).is_err());
    assert!(second.acquire(&path, true).is_ok());
}
#[test]
fn drop_releases_exact_owner() {
    let (root, live, path) = fixture();
    {
        let first = manager(&root, owner("one", 1), Arc::clone(&live));
        first.acquire(&path, false).expect("first");
    }
    manager(&root, owner("two", 2), live)
        .acquire(&path, false)
        .expect("after drop");
}

use crate::sandbox::{KillPhase, SHELL_CHILD_KILL_GRACE_MS, kill_and_reap_observed};
use std::{
    fs,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::process::{Child, Command};

#[cfg(unix)]
static NEXT: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
struct ProcessFixture {
    root: PathBuf,
    child: Option<Child>,
    leader: u32,
    descendant: u32,
    armed: bool,
}

#[cfg(unix)]
impl ProcessFixture {
    async fn spawn(term: &'static str) -> Self {
        let root = unique_root();
        let ready = root.join("ready");
        let fifo = root.join("block");
        let status = std::process::Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .expect("create fifo");
        assert!(status.success());
        let parent_term = if term == "exit 0" {
            "trap - TERM; exit 0"
        } else {
            term
        };
        let descendant_term = if term == "exit 0" { "" } else { term };
        let script = format!(
            "trap '{parent_term}' TERM; \
             (trap '{descendant_term}' TERM; printf ready > \"$2\"; \
             while :; do /bin/sleep 3600 & wait $!; done) & descendant=$!; \
             read value < \"$2\"; printf '%s %s\\n' $$ $descendant > \"$1\"; \
             while :; do /bin/sleep 3600 & wait $!; done"
        );
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .arg("task38")
            .arg(&ready)
            .arg(&fifo)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
        let child = command.spawn().expect("spawn process fixture");
        let leader = child.id().expect("fixture leader pid");
        let line = bounded_read(&ready).await;
        let mut pids = line.split_whitespace();
        assert_eq!(pids.next(), Some(leader.to_string().as_str()));
        let descendant: u32 = pids
            .next()
            .expect("descendant pid")
            .parse()
            .expect("numeric pid");
        assert!(pids.next().is_none());
        assert_live(leader);
        assert_live(descendant);
        Self {
            root,
            child: Some(child),
            leader,
            descendant,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.child.take();
    }
}

#[cfg(unix)]
impl Drop for ProcessFixture {
    fn drop(&mut self) {
        use rustix::process::{Pid, Signal, kill_process_group};
        if self.armed
            && let Some(group) = Pid::from_raw(i32::try_from(self.leader).unwrap_or_default())
        {
            let _ = kill_process_group(group, Signal::KILL);
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
fn unique_root() -> PathBuf {
    for _ in 0..32 {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("lotta-task38-{}-{id}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("create fixture root: {error}"),
        }
    }
    panic!("bounded fixture root retries exhausted")
}

#[cfg(unix)]
async fn bounded_read(path: &std::path::Path) -> String {
    tokio::task::spawn_blocking({
        let path = path.to_owned();
        move || {
            for _ in 0..10_000 {
                match fs::read_to_string(&path) {
                    Ok(value) if !value.is_empty() => return value,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => panic!("read readiness: {error}"),
                }
                std::thread::yield_now();
            }
            panic!("bounded readiness timeout")
        }
    })
    .await
    .expect("readiness task")
}

#[cfg(unix)]
fn assert_live(pid: u32) {
    use rustix::process::{Pid, test_kill_process};
    test_kill_process(Pid::from_raw(i32::try_from(pid).expect("pid range")).expect("positive pid"))
        .expect("process must be live");
}

#[cfg(unix)]
async fn assert_dead(pid: u32, group: bool) {
    for _ in 0..1_024 {
        if !has_live_identity(pid, group) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("process identity remained live")
}

#[cfg(unix)]
fn has_live_identity(pid: u32, group: bool) -> bool {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,pgid=,state="])
        .output()
        .expect("inspect process state");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_process_row)
        .any(|(row_pid, row_group, state)| {
            let identity = if group { row_group } else { row_pid };
            identity == pid && state != 'Z'
        })
}

#[cfg(unix)]
fn parse_process_row(value: &str) -> Option<(u32, u32, char)> {
    let mut fields = value.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let group = fields.next()?.parse().ok()?;
    let state = fields.next()?.chars().next()?;
    Some((pid, group, state))
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn term_then_kill_at_exact_grace_and_reap() {
    assert_eq!(SHELL_CHILD_KILL_GRACE_MS, 2_000);
    let mut fixture = ProcessFixture::spawn("").await;
    let (leader, descendant) = (fixture.leader, fixture.descendant);
    let phases = Arc::new(Mutex::new(Vec::with_capacity(2)));
    let observed = Arc::clone(&phases);
    let mut child = fixture.child.take().expect("armed child");
    let cleanup = tokio::spawn(async move {
        kill_and_reap_observed(&mut child, |phase| {
            observed.lock().expect("phase lock").push(phase);
        })
        .await;
        assert!(child.try_wait().expect("try wait").is_some());
    });
    tokio::task::yield_now().await;
    assert_eq!(*phases.lock().expect("phase lock"), [KillPhase::Term]);
    assert_live(leader);
    assert_live(descendant);
    tokio::time::advance(std::time::Duration::from_millis(1_999)).await;
    tokio::task::yield_now().await;
    assert!(!cleanup.is_finished());
    assert_eq!(*phases.lock().expect("phase lock"), [KillPhase::Term]);
    assert_live(leader);
    assert_live(descendant);
    tokio::time::advance(std::time::Duration::from_millis(1)).await;
    cleanup.await.expect("cleanup task");
    assert_eq!(
        *phases.lock().expect("phase lock"),
        [KillPhase::Term, KillPhase::Kill]
    );
    assert_dead(leader, false).await;
    assert_dead(descendant, false).await;
    assert_dead(leader, true).await;
    fixture.disarm();
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn term_responsive_parent_leaves_descendant_until_exact_kill_deadline() {
    let mut fixture = ProcessFixture::spawn("exit 0").await;
    let (leader, descendant) = (fixture.leader, fixture.descendant);
    let phases = Arc::new(Mutex::new(Vec::with_capacity(2)));
    let observed = Arc::clone(&phases);
    let mut child = fixture.child.take().expect("armed child");
    let cleanup = tokio::spawn(async move {
        kill_and_reap_observed(&mut child, |phase| {
            observed.lock().expect("phase lock").push(phase);
        })
        .await;
        assert!(child.try_wait().expect("try wait").is_some());
    });
    tokio::task::yield_now().await;
    assert_eq!(*phases.lock().expect("phase lock"), [KillPhase::Term]);
    assert_live(descendant);
    tokio::time::advance(std::time::Duration::from_millis(1_999)).await;
    tokio::task::yield_now().await;
    assert!(!cleanup.is_finished());
    assert_live(descendant);
    tokio::time::advance(std::time::Duration::from_millis(1)).await;
    cleanup.await.expect("cleanup task");
    assert_eq!(
        *phases.lock().expect("phase lock"),
        [KillPhase::Term, KillPhase::Kill]
    );
    assert_dead(descendant, false).await;
    assert_dead(leader, true).await;
    fixture.disarm();
}

#[cfg(not(unix))]
#[test]
fn unsupported_platform_is_explicit() {
    assert_eq!(std::env::consts::FAMILY, "unix", "Task38 requires Unix");
}

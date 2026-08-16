use lotta_domain::{AgentId, ConversationId};
use lotta_store::{LocalStore, SidePaths, StorePaths};
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const AGENT_ID: &str = "agent-local-fixture";
pub const CONVERSATION_ID: &str = "default";
pub const SCHEDULE_ID: &str = "schedule-fixture";
const PROCESS_TIMEOUT_MS: u64 = 30_000;
const PROCESS_OUTPUT_BYTES_MAX: usize = 1_048_576;
const ROOT_CREATE_RETRIES_MAX: u64 = 3;
static ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

type Drain = std::thread::JoinHandle<Result<Vec<u8>, String>>;

pub struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    pub fn new(label: &str) -> Self {
        let sequence = ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        for attempt in 0..ROOT_CREATE_RETRIES_MAX {
            let path = std::env::temp_dir().join(format!(
                "lotta-task31-{label}-{}-{sequence}-{attempt}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    std::fs::write(path.join(".lotta-task31-owned"), b"owned\n")
                        .expect("write root ownership marker");
                    return Self { path };
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create isolated root: {error}"),
            }
        }
        panic!("isolated root collision retries exhausted")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn backend(&self) -> PathBuf {
        self.path.join("backend")
    }

    pub fn home(&self) -> PathBuf {
        self.path.join("home")
    }

    pub fn letta_home(&self) -> PathBuf {
        self.path.join("letta-home")
    }

    pub fn workspace(&self) -> PathBuf {
        self.path.join("workspace")
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub struct Lease {
    path: PathBuf,
}

impl Lease {
    pub fn acquire(root: &Path, owner: &str) -> Result<Self, String> {
        let path = root.join("runtime-owner.json");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| "concurrent runtime writer handoff lease exists".to_owned())?;
        let value = serde_json::json!({"owner": owner, "pid": std::process::id()});
        serde_json::to_writer(&mut file, &value).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        Ok(Self { path })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn pinned_root() -> PathBuf {
    let root = manifest_dir().join("../../letta-code");
    let root = root
        .canonicalize()
        .expect("canonical pinned letta-code root");
    for file in [
        "src/backend/local/local-store.ts",
        "src/backend/local/local-backend.ts",
        "src/backend/local/local-provider-auth-store.ts",
        "src/cron/cron-file.ts",
        "src/cron/run-log.ts",
        "src/settings-manager.ts",
        "src/channels/config.ts",
        "src/channels/accounts.ts",
        "src/channels/routing.ts",
        "src/channels/pairing.ts",
        "src/channels/targets.ts",
        "src/channels/pending-control-requests.ts",
        "bun.lock",
        "package.json",
    ] {
        assert!(root.join(file).is_file(), "missing pinned source {file}");
    }
    root
}

pub fn fixture(name: &str) -> PathBuf {
    manifest_dir().join("fixtures/persistence").join(name)
}

pub fn backend_store(root: &TestRoot) -> LocalStore {
    std::fs::create_dir_all(root.backend()).expect("create backend root");
    LocalStore::new(StorePaths::new(root.backend()).expect("valid backend paths"))
}

pub fn side_paths(root: &TestRoot) -> SidePaths {
    for path in [root.home(), root.letta_home(), root.workspace()] {
        std::fs::create_dir_all(path).expect("create side root");
    }
    SidePaths::new(root.home(), Some(root.letta_home()), [root.workspace()])
        .expect("valid side paths")
}

pub fn agent_id() -> AgentId {
    AgentId::accept(AGENT_ID).expect("valid agent id")
}

pub fn conversation_id() -> ConversationId {
    ConversationId::accept(CONVERSATION_ID).expect("valid conversation id")
}

pub fn run_ts(root: &TestRoot, operation: &str, artifact: &str) -> Value {
    run_ts_expect(root, operation, artifact, true)
}

pub fn run_ts_failure(root: &TestRoot, operation: &str, artifact: &str) -> Value {
    run_ts_expect(root, operation, artifact, false)
}

pub fn run_ts_with_paths(
    root: &TestRoot,
    operation: &str,
    artifact: &str,
    backend: PathBuf,
) -> Value {
    run_ts_request(root, operation, artifact, backend, false)
}

fn run_ts_expect(root: &TestRoot, operation: &str, artifact: &str, success: bool) -> Value {
    run_ts_request(root, operation, artifact, root.backend(), success)
}

fn run_ts_request(
    root: &TestRoot,
    operation: &str,
    artifact: &str,
    backend: PathBuf,
    success: bool,
) -> Value {
    let mut child = spawn_ts(root, operation, artifact, backend).expect("spawn TS runner");
    let output = child.finish(Duration::from_millis(PROCESS_TIMEOUT_MS));
    let output = output.unwrap_or_else(|error| panic!("runner failed: {error}"));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = sanitize(&output.stderr);
    assert_eq!(
        output.status.success(),
        success,
        "runner status; stdout={stdout}; stderr={stderr}"
    );
    serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!("runner response parse: {error}; stdout={stdout}; stderr={stderr}")
    })
}

#[derive(Debug)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: ExitStatus,
}

pub struct ManagedChild {
    child: Option<Child>,
    stdout: Option<Drain>,
    stderr: Option<Drain>,
    failed: Arc<AtomicBool>,
}

impl ManagedChild {
    fn new(mut child: Child) -> Result<Self, String> {
        let stdout = child.stdout.take().ok_or("missing child stdout")?;
        let stderr = child.stderr.take().ok_or("missing child stderr")?;
        let failed = Arc::new(AtomicBool::new(false));
        let stdout = spawn_drain(stdout, Arc::clone(&failed));
        let stderr = spawn_drain(stderr, Arc::clone(&failed));
        Ok(Self {
            child: Some(child),
            stdout: Some(stdout),
            stderr: Some(stderr),
            failed,
        })
    }

    pub fn finish(&mut self, timeout: Duration) -> Result<ProcessOutput, String> {
        let deadline = Instant::now() + timeout;
        let status = loop {
            if self.failed.load(Ordering::Acquire) {
                self.kill_reap()?;
                return self.join_error("process output limit or I/O failure");
            }
            let child = self.child.as_mut().ok_or("child already finished")?;
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                self.child.take();
                break status;
            }
            if Instant::now() >= deadline {
                self.kill_reap()?;
                self.join_drains()?;
                return Err("process timeout".to_owned());
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let (stdout, stderr) = self.join_drains()?;
        Ok(ProcessOutput {
            stdout,
            stderr,
            status,
        })
    }

    fn kill_reap(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            child
                .wait()
                .map_err(|error| format!("reap child: {error}"))?;
        }
        Ok(())
    }

    fn join_error<T>(&mut self, message: &str) -> Result<T, String> {
        let _ = self.join_drains();
        Err(message.to_owned())
    }

    fn join_drains(&mut self) -> Result<(Vec<u8>, Vec<u8>), String> {
        let stdout = self.stdout.take().ok_or("stdout drain missing")?;
        let stderr = self.stderr.take().ok_or("stderr drain missing")?;
        let stdout = stdout.join().map_err(|_| "stdout drain panicked")??;
        let stderr = stderr.join().map_err(|_| "stderr drain panicked")??;
        Ok((stdout, stderr))
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        let _ = self.kill_reap();
        let _ = self.join_drains();
    }
}

pub struct TsHolder {
    child: ManagedChild,
    ready: PathBuf,
    release: PathBuf,
}

impl TsHolder {
    pub fn spawn(root: &TestRoot) -> Result<Self, String> {
        let child = spawn_ts(root, "hold_lease", "agent", root.backend())?;
        let ready = root.path().join("holder-ready");
        let release = root.path().join("holder-release");
        Ok(Self {
            child,
            ready,
            release,
        })
    }

    pub fn wait_ready(&self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_millis(PROCESS_TIMEOUT_MS);
        while !self.ready.is_file() {
            if Instant::now() >= deadline {
                return Err("holder ready timeout".to_owned());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }

    pub fn release_and_finish(mut self) -> Result<Value, String> {
        std::fs::write(&self.release, b"release\n").map_err(|error| error.to_string())?;
        let output = self
            .child
            .finish(Duration::from_millis(PROCESS_TIMEOUT_MS))?;
        if !output.status.success() {
            return Err(format!(
                "holder status failed: {}",
                sanitize(&output.stderr)
            ));
        }
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())
    }
}

impl Drop for TsHolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.ready);
        let _ = std::fs::remove_file(&self.release);
    }
}

pub fn run_command_bounded(
    program: &str,
    args: &[&str],
    directory: &Path,
) -> Result<ProcessOutput, String> {
    let child = Command::new(program)
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn {program}: {error}"))?;
    ManagedChild::new(child)?.finish(Duration::from_millis(PROCESS_TIMEOUT_MS))
}

pub fn run_ts_raw(root: &TestRoot, operation: &str) -> Result<ProcessOutput, String> {
    let mut child = spawn_ts(root, operation, "bounded", root.backend())?;
    child.finish(Duration::from_millis(PROCESS_TIMEOUT_MS))
}

fn spawn_ts(
    root: &TestRoot,
    operation: &str,
    artifact: &str,
    backend: PathBuf,
) -> Result<ManagedChild, String> {
    let request = serde_json::json!({
        "operation": operation,
        "artifact": artifact,
        "root": root.path(),
        "backend": backend,
        "home": root.home(),
        "lettaHome": root.letta_home(),
        "workspace": root.workspace(),
    });
    let runner = manifest_dir().join("tests/conformance/harness/ts_runner.mjs");
    let mut child = Command::new("bun")
        .arg(runner)
        .current_dir(pinned_root())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", root.home())
        .env("LETTA_HOME", root.letta_home())
        .env("LETTA_LOCAL_BACKEND_DIR", root.backend())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if bytes.len() > PROCESS_OUTPUT_BYTES_MAX {
        return Err("request output limit".to_owned());
    }
    child
        .stdin
        .take()
        .ok_or("runner stdin")?
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    ManagedChild::new(child)
}

fn spawn_drain(mut source: impl Read + Send + 'static, failed: Arc<AtomicBool>) -> Drain {
    std::thread::spawn(move || {
        let fail = |message: String| {
            failed.store(true, Ordering::Release);
            message
        };
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 64 * 1_024];
        loop {
            let count = source
                .read(&mut buffer)
                .map_err(|error| fail(format!("process output read: {error}")))?;
            if count == 0 {
                return Ok(bytes);
            }
            let next = bytes
                .len()
                .checked_add(count)
                .ok_or_else(|| fail("process output byte overflow".to_owned()))?;
            if next > PROCESS_OUTPUT_BYTES_MAX {
                return Err(fail("process output limit exceeded".to_owned()));
            }
            bytes
                .try_reserve(count)
                .map_err(|_| fail("process output allocation failed".to_owned()))?;
            bytes.extend_from_slice(&buffer[..count]);
        }
    })
}

fn sanitize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|value| !value.is_control() || matches!(value, '\n' | '\r' | '\t'))
        .take(4_096)
        .collect()
}

pub fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    super::tree::copy_tree(source, target)
}

pub fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read json")).expect("parse json")
}

pub fn json_rows(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("read jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse jsonl row"))
        .collect()
}

pub fn assert_contains(
    expected: &Value,
    actual: &Value,
    path: &str,
    direction: &str,
    artifact: &str,
) {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            for (field, value) in expected {
                let pointer = format!("{path}/{}", field.replace('~', "~0").replace('/', "~1"));
                let found = actual.get(field).unwrap_or_else(|| {
                    panic!("{direction} {artifact}: lost field {field} at JSON Pointer {pointer}")
                });
                assert_contains(value, found, &pointer, direction, artifact);
            }
        }
        (Value::Array(expected), Value::Array(actual)) => {
            assert!(
                actual.len() >= expected.len(),
                "{direction} {artifact}: lost array rows at {path}"
            );
            for (index, value) in expected.iter().enumerate() {
                assert_contains(
                    value,
                    &actual[index],
                    &format!("{path}/{index}"),
                    direction,
                    artifact,
                );
            }
        }
        _ => assert_eq!(
            expected, actual,
            "{direction} {artifact}: semantic mismatch at {path}"
        ),
    }
}

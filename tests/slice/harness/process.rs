use super::{
    FixedClock, ObservationLog, RawFrame, STDERR_MAX, STDOUT_MAX, Service, TOKEN, TOKEN_SHA256,
    checkout, verify_checkout,
};
use lotta_app_server::config::ServerArgs;
use lotta_app_server::listener::start_listener_with_runtime_service_and_observer;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct ChildGuard(Option<Child>);
impl ChildGuard {
    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("owned child")
    }
    fn take(&mut self) -> Child {
        self.0.take().expect("owned child")
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn bounded_reader(mut reader: impl Read + Send + 'static, limit: usize) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::with_capacity(limit.saturating_add(1));
        reader
            .by_ref()
            .take(u64::try_from(limit.saturating_add(1)).unwrap())
            .read_to_end(&mut bytes)
            .expect("read bounded child output");
        bytes
    })
}

fn spawn_client(checkout: &Path, url: &str, mode: &str) -> Result<ChildGuard, String> {
    let source = checkout
        .join("src/app-server-client.ts")
        .canonicalize()
        .map_err(|error| format!("canonical client source: {error}"))?;
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/slice/harness/client.ts");
    let child = Command::new("bun")
        .arg("run")
        .arg(script)
        .arg(source)
        .arg(url)
        .arg(TOKEN)
        .arg(mode)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(checkout)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn bounded Bun client: {error}"))?;
    Ok(ChildGuard(Some(child)))
}

fn wait_client(guard: &mut ChildGuard, timeout: Duration) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = guard
            .child_mut()
            .try_wait()
            .map_err(|error| format!("poll Bun client: {error}"))?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let kill = guard
                .child_mut()
                .kill()
                .map_err(|error| format!("kill timed out Bun client: {error}"));
            let reaped = guard
                .child_mut()
                .wait()
                .map_err(|error| format!("reap timed out Bun client: {error}"));
            return match (kill, reaped) {
                (Ok(()), Ok(status)) => Err(format!(
                    "Bun client timeout after {timeout:?}; killed=true; reaped={status}"
                )),
                (Err(error), _) | (_, Err(error)) => Err(error),
            };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn scrub(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace(TOKEN, "<redacted>")
}

fn check_output_bounds(stdout: &[u8], stderr: &[u8]) -> Result<(), String> {
    if stdout.len() <= STDOUT_MAX && stderr.len() <= STDERR_MAX {
        return Ok(());
    }
    Err(format!(
        "Bun client output exceeded bound: stdout={} stderr={} stdout_text={} stderr_text={}",
        stdout.len(),
        stderr.len(),
        scrub(stdout),
        scrub(stderr)
    ))
}

pub(super) fn run_client(checkout: &Path, url: &str) -> Result<Vec<RawFrame>, String> {
    run_client_mode(checkout, url, "capture", Duration::from_secs(10))
}

fn run_client_mode(
    checkout: &Path,
    url: &str,
    mode: &str,
    timeout: Duration,
) -> Result<Vec<RawFrame>, String> {
    let mut guard = spawn_client(checkout, url, mode)?;
    let stdout = bounded_reader(
        guard.child_mut().stdout.take().expect("piped stdout"),
        STDOUT_MAX,
    );
    let stderr = bounded_reader(
        guard.child_mut().stderr.take().expect("piped stderr"),
        STDERR_MAX,
    );
    let status_result = wait_client(&mut guard, timeout);
    let wait_result = guard.take().wait();
    let stdout = stdout.join().map_err(|_| "join stdout reader".to_owned())?;
    let stderr = stderr.join().map_err(|_| "join stderr reader".to_owned())?;
    check_output_bounds(&stdout, &stderr)?;
    wait_result.map_err(|error| format!("wait Bun client: {error}"))?;
    let status = status_result?;
    if !status.success() {
        return Err(format!(
            "Bun client failed: status={status} stderr={} stdout={}",
            scrub(&stderr),
            scrub(&stdout)
        ));
    }
    serde_json::from_slice(&stdout).map_err(|error| {
        format!(
            "invalid client observations: {error}; stdout={}; stderr={}",
            scrub(&stdout),
            scrub(&stderr)
        )
    })
}

#[derive(Debug)]
pub struct ModeProof {
    pub error: String,
    pub listener_waited: bool,
    pub elapsed: Duration,
    pub checkout_clean: bool,
}

async fn run_failure_mode(mode: &'static str, timeout: Duration) -> ModeProof {
    let started = Instant::now();
    let checkout = checkout();
    verify_checkout(&checkout);
    let observations = Arc::new(ObservationLog::default());
    let service = Service::new(observations.clone());
    let prepared = ServerArgs {
        listen_enabled: true,
        listen: Some("ws://127.0.0.1:0/ws".into()),
        ws_auth: Some("capability-token".into()),
        ws_token_sha256: Some(TOKEN_SHA256.into()),
        ..ServerArgs::default()
    }
    .prepare()
    .expect("prepare failure probe listener");
    let listener = start_listener_with_runtime_service_and_observer(
        prepared,
        Arc::new(FixedClock::default()),
        service,
        observations,
    )
    .await
    .expect("start failure probe listener");
    let client_checkout = checkout.clone();
    let client_url = listener.websocket_url().to_owned();
    let client = tokio::task::spawn_blocking(move || {
        run_client_mode(&client_checkout, &client_url, mode, timeout)
    })
    .await;
    let listener_result = listener.wait().await;
    let listener_waited = listener_result.is_ok();
    let error = match client {
        Ok(Err(error)) => error,
        Ok(Ok(_)) => "client mode unexpectedly succeeded".into(),
        Err(error) => format!("join Bun client: {error}"),
    };
    verify_checkout(&checkout);
    ModeProof {
        error,
        listener_waited,
        elapsed: started.elapsed(),
        checkout_clean: true,
    }
}

pub async fn assert_failure_cleanup_and_timeout() {
    let failure = run_failure_mode("fail_after_connect", Duration::from_secs(1)).await;
    assert!(failure.error.contains("fail_after_connect"), "{failure:?}");
    assert!(failure.listener_waited, "{failure:?}");
    assert!(failure.checkout_clean, "{failure:?}");
    assert!(failure.elapsed < Duration::from_secs(2), "{failure:?}");
    eprintln!("mode proof: {failure:?}");

    let timeout = run_failure_mode("stay_alive", Duration::from_millis(200)).await;
    assert!(
        timeout.error.contains("Bun client timeout after 200ms"),
        "{timeout:?}"
    );
    assert!(
        timeout.error.contains("killed=true; reaped="),
        "{timeout:?}"
    );
    assert!(timeout.listener_waited, "{timeout:?}");
    assert!(timeout.checkout_clean, "{timeout:?}");
    assert!(timeout.elapsed < Duration::from_secs(2), "{timeout:?}");
    eprintln!("mode proof: {timeout:?}");
}

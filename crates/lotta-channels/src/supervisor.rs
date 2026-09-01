//! Bounded supervision of the real channel-host child process.

use crate::{
    control_plane::{
        ChildFrame, ControlError, ControlPlane, ParentFrame, RuntimeKey, RuntimeTool, read_line,
        write_line,
    },
    topology::{
        CHANNEL_STARTUP_DEADLINE_MS, ChannelStore, ChildCapability, TopologyError,
        sandboxed_child_command,
    },
};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStdin},
    sync::{oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

/// Maximum restarts after the initial child launch.
pub const CHANNEL_RESTART_ATTEMPTS_MAX: usize = 3;
/// Initial restart delay.
pub const CHANNEL_RESTART_BACKOFF_MS: u64 = 100;
/// Maximum restart delay.
pub const CHANNEL_RESTART_BACKOFF_MS_MAX: u64 = 2_000;
/// Grace allowed after a shutdown management request.
pub const CHANNEL_SHUTDOWN_GRACE_MS: u64 = 2_000;
/// Maximum stderr line bytes drained without logging content.
pub const CHANNEL_STDERR_LINE_BYTES_MAX: usize = 16 * 1024;

/// Stable supervised service failure.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    /// Topology preparation failed.
    #[error("channel child topology failed")]
    Topology(#[from] TopologyError),
    /// Management protocol failed.
    #[error("channel child management failed")]
    Control(#[from] ControlError),
    /// Child spawn or pipe acquisition failed.
    #[error("channel child spawn failed")]
    Spawn,
    /// Startup handshake exceeded its deadline or was rejected.
    #[error("channel child startup failed")]
    Startup,
    /// Child cleanup or waiter failed.
    #[error("channel child cleanup failed")]
    Cleanup,
    /// Supervisor task failed.
    #[error("channel supervisor task failed")]
    Task,
}

/// Immutable launch configuration containing no backend-store path.
pub struct ChannelLaunchConfig {
    /// Absolute child executable.
    pub executable: PathBuf,
    /// Canonical shared channels store.
    pub store: ChannelStore,
    /// Dedicated loopback App Server WebSocket URL.
    pub websocket_url: String,
    /// Per-child capability, delivered only over stdin.
    pub capability: ChildCapability,
}

/// Running supervised channel service.
pub struct ChannelSupervisor {
    cancellation: CancellationToken,
    pid: watch::Receiver<Option<u32>>,
    plane: Arc<Mutex<ControlPlane>>,
    task: JoinHandle<Result<(), SupervisorError>>,
}

impl ChannelSupervisor {
    /// Spawns the supervisor and waits for the first authenticated child handshake.
    ///
    /// # Errors
    /// Returns a topology, spawn, startup, protocol, cleanup, or task failure.
    pub async fn start(config: ChannelLaunchConfig) -> Result<Self, SupervisorError> {
        let owner = config.capability.owner().to_owned();
        let plane = Arc::new(Mutex::new(ControlPlane::new(owner)?));
        let cancellation = CancellationToken::new();
        let (pid_sender, pid) = watch::channel(None);
        let (started_sender, started_receiver) = oneshot::channel();
        let actor_plane = Arc::clone(&plane);
        let actor_cancel = cancellation.clone();
        let task = tokio::spawn(async move {
            run_supervisor(
                config,
                actor_plane,
                actor_cancel,
                pid_sender,
                started_sender,
            )
            .await
        });
        let startup = tokio::time::timeout(
            Duration::from_millis(CHANNEL_STARTUP_DEADLINE_MS),
            started_receiver,
        )
        .await;
        if let Ok(Ok(Ok(()))) = startup {
            Ok(Self {
                cancellation,
                pid,
                plane,
                task,
            })
        } else {
            cancellation.cancel();
            let _ = task.await;
            Err(SupervisorError::Startup)
        }
    }

    /// Returns the currently live child process identifier.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        *self.pid.borrow()
    }

    /// Returns one exact runtime tool snapshot from the parent registry.
    #[must_use]
    pub fn runtime_tools(&self, runtime: &RuntimeKey) -> Option<Vec<RuntimeTool>> {
        self.plane
            .lock()
            .ok()
            .and_then(|plane| plane.registry().snapshot(runtime))
    }

    /// Requests graceful close, then bounded kill/reap, and waits for the supervisor.
    ///
    /// # Errors
    /// Returns a protocol, cleanup, or supervisor-task failure.
    pub async fn shutdown(self) -> Result<(), SupervisorError> {
        self.cancellation.cancel();
        self.task.await.map_err(|_| SupervisorError::Task)?
    }
}

async fn run_supervisor(
    mut config: ChannelLaunchConfig,
    plane: Arc<Mutex<ControlPlane>>,
    cancellation: CancellationToken,
    pid_sender: watch::Sender<Option<u32>>,
    started: oneshot::Sender<Result<(), SupervisorError>>,
) -> Result<(), SupervisorError> {
    let mut started = Some(started);
    let mut delay = CHANNEL_RESTART_BACKOFF_MS;
    for attempt in 0..=CHANNEL_RESTART_ATTEMPTS_MAX {
        if cancellation.is_cancelled() {
            break;
        }
        let outcome = run_attempt(
            &mut config,
            &plane,
            &cancellation,
            &pid_sender,
            &mut started,
        )
        .await;
        if cancellation.is_cancelled() {
            break;
        }
        if attempt == CHANNEL_RESTART_ATTEMPTS_MAX {
            return outcome;
        }
        release_stale(&plane);
        tokio::time::sleep(Duration::from_millis(delay)).await;
        delay = delay.saturating_mul(2).min(CHANNEL_RESTART_BACKOFF_MS_MAX);
    }
    release_stale(&plane);
    config.capability.revoke();
    let _ = pid_sender.send(None);
    Ok(())
}

async fn run_attempt(
    config: &mut ChannelLaunchConfig,
    plane: &Arc<Mutex<ControlPlane>>,
    cancellation: &CancellationToken,
    pid_sender: &watch::Sender<Option<u32>>,
    started: &mut Option<oneshot::Sender<Result<(), SupervisorError>>>,
) -> Result<(), SupervisorError> {
    if !config.capability.is_valid() {
        return Err(SupervisorError::Startup);
    }
    let mut command = sandboxed_child_command(&config.executable, &config.store)?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| SupervisorError::Spawn)?;
    let pid = child.id().ok_or(SupervisorError::Spawn)?;
    let mut input = child.stdin.take().ok_or(SupervisorError::Spawn)?;
    let output = child.stdout.take().ok_or(SupervisorError::Spawn)?;
    let stderr = child.stderr.take().ok_or(SupervisorError::Spawn)?;
    let _ = pid_sender.send(Some(pid));
    let stderr_task = tokio::spawn(drain_stderr(stderr));
    let bootstrap_id = format!("bootstrap-{pid}");
    write_bootstrap(config, &mut input, &bootstrap_id).await?;
    let mut reader = BufReader::new(output);
    let startup = read_ready(&mut reader, plane, &bootstrap_id, pid).await;
    if let Some(sender) = started.take() {
        let startup_result = match &startup {
            Ok(()) => Ok(()),
            Err(error) => Err(copy_error(error)),
        };
        let _ = sender.send(startup_result);
    }
    startup?;
    supervise_session(
        child,
        input,
        reader,
        stderr_task,
        plane,
        cancellation,
        config.store.clone(),
    )
    .await?;
    let _ = pid_sender.send(None);
    Ok(())
}

async fn read_ready<R>(
    reader: &mut BufReader<R>,
    plane: &Arc<Mutex<ControlPlane>>,
    bootstrap_id: &str,
    pid: u32,
) -> Result<(), SupervisorError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let frame = tokio::time::timeout(
        Duration::from_millis(CHANNEL_STARTUP_DEADLINE_MS),
        read_line::<_, ChildFrame>(reader),
    )
    .await
    .map_err(|_| SupervisorError::Startup)??
    .ok_or(SupervisorError::Startup)?;
    match &frame {
        ChildFrame::Ready {
            correlation_id,
            pid: child_pid,
            ..
        } if correlation_id == bootstrap_id && *child_pid == pid => {}
        _ => return Err(SupervisorError::Startup),
    }
    let channels = Vec::new();
    let mut plane = plane.lock().map_err(|_| SupervisorError::Task)?;
    plane.dispatch(frame, &channels)?;
    plane.register_message_channel(RuntimeKey {
        agent_id: "channel-host".into(),
        conversation_id: "channel-host".into(),
    })?;
    Ok(())
}

async fn supervise_session<R>(
    mut child: Child,
    mut input: ChildStdin,
    mut reader: BufReader<R>,
    stderr_task: JoinHandle<()>,
    plane: &Arc<Mutex<ControlPlane>>,
    cancellation: &CancellationToken,
    store: ChannelStore,
) -> Result<(), SupervisorError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let result = loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                let request_id = format!("shutdown-{}", child.id().map_or(0, |pid| pid));
                let _ = write_line(&mut input, &ParentFrame::Shutdown { request_id }).await;
                break stop_and_reap(&mut child).await;
            }
            frame = read_line::<_, ChildFrame>(&mut reader) => {
                let Some(frame) = frame? else { break wait_once(&mut child).await; };
                let channels = store.channel_state()?;
                let response = plane.lock().map_err(|_| SupervisorError::Task)?
                    .dispatch(frame, &channels)?;
                if let Some(response) = response { write_line(&mut input, &response).await?; }
            }
            status = child.wait() => {
                break status.map(|_| ()).map_err(|_| SupervisorError::Cleanup);
            }
        }
    };
    drop(input);
    if tokio::time::timeout(
        Duration::from_millis(CHANNEL_SHUTDOWN_GRACE_MS),
        stderr_task,
    )
    .await
    .is_err()
    {
        return Err(SupervisorError::Cleanup);
    }
    result
}

async fn write_bootstrap(
    config: &ChannelLaunchConfig,
    input: &mut ChildStdin,
    request_id: &str,
) -> Result<(), SupervisorError> {
    let root = config.store.root().to_str().ok_or(SupervisorError::Spawn)?;
    let frame = ParentFrame::Bootstrap {
        request_id: request_id.to_owned(),
        owner: config.capability.owner().to_owned(),
        websocket_url: config.websocket_url.clone(),
        token: config.capability.expose_for_pipe().to_owned(),
        channels_root: root.to_owned(),
    };
    write_line(input, &frame)
        .await
        .map_err(SupervisorError::from)
}

async fn stop_and_reap(child: &mut Child) -> Result<(), SupervisorError> {
    match tokio::time::timeout(
        Duration::from_millis(CHANNEL_SHUTDOWN_GRACE_MS),
        child.wait(),
    )
    .await
    {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err(SupervisorError::Cleanup),
        Err(_) => {
            child.kill().await.map_err(|_| SupervisorError::Cleanup)?;
            wait_once(child).await
        }
    }
}

async fn wait_once(child: &mut Child) -> Result<(), SupervisorError> {
    child
        .wait()
        .await
        .map(|_| ())
        .map_err(|_| SupervisorError::Cleanup)
}

async fn drain_stderr<R>(stderr: R)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stderr);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(_) if bytes.len() > CHANNEL_STDERR_LINE_BYTES_MAX => {
                bytes.truncate(CHANNEL_STDERR_LINE_BYTES_MAX);
            }
            Ok(_) => {}
        }
    }
}

fn release_stale(plane: &Arc<Mutex<ControlPlane>>) {
    if let Ok(mut plane) = plane.lock() {
        let _ = plane.release_stale();
    }
}

fn copy_error(error: &SupervisorError) -> SupervisorError {
    match error {
        SupervisorError::Topology(value) => SupervisorError::Topology(*value),
        SupervisorError::Control(value) => SupervisorError::Control(*value),
        SupervisorError::Spawn => SupervisorError::Spawn,
        SupervisorError::Startup => SupervisorError::Startup,
        SupervisorError::Cleanup => SupervisorError::Cleanup,
        SupervisorError::Task => SupervisorError::Task,
    }
}

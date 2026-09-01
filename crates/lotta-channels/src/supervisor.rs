//! Single-actor supervision of the sandboxed channel-host process.

use crate::{
    control_plane::{
        ChildFrame, ControlError, ControlPlane, ParentFrame, RuntimeKey, read_line, write_line,
    },
    topology::{CHANNEL_STARTUP_DEADLINE_MS, ChannelStore, TopologyError, sandboxed_child_command},
};
use lotta_app_server::auth::channel_session::{
    ChannelSessionAuthenticator, ChannelSessionCapability,
};
use lotta_tools::external::ChannelExternalToolManager;
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
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
/// Grace allowed for the correlated shutdown acknowledgement.
pub const CHANNEL_SHUTDOWN_GRACE_MS: u64 = 2_000;
/// Maximum stderr bytes consumed from one child generation.
pub const CHANNEL_STDERR_TOTAL_BYTES_MAX: usize = 256 * 1024;
/// Maximum stderr bytes retained for one logical line.
pub const CHANNEL_STDERR_LINE_BYTES_MAX: usize = 16 * 1024;
/// Fixed allocation-free stderr drain chunk.
pub const CHANNEL_STDERR_CHUNK_BYTES: usize = 4 * 1024;

/// Stable supervised service failure.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    /// Topology preparation failed.
    #[error("channel child topology failed")]
    Topology(#[from] TopologyError),
    /// Management protocol failed.
    #[error("channel child management failed")]
    Control(#[from] ControlError),
    /// Child spawn, entropy, or pipe acquisition failed.
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

/// Immutable launch configuration containing no backend-store path or reusable credential.
pub struct ChannelLaunchConfig {
    /// Absolute child executable.
    pub executable: PathBuf,
    /// Canonical shared channels store.
    pub store: ChannelStore,
    /// Dedicated exact loopback App Server WebSocket URL.
    pub websocket_url: String,
    /// Owner prefix; a generation suffix is added for every spawn.
    pub owner_prefix: String,
    /// Dynamic authority bound to the exact dedicated listener instance.
    pub authenticator: ChannelSessionAuthenticator,
    /// Canonical external-tool manager backed by production turn setup's registry.
    pub tools: Arc<ChannelExternalToolManager>,
}

/// Running supervised channel service.
pub struct ChannelSupervisor {
    cancellation: CancellationToken,
    pid: watch::Receiver<Option<u32>>,
    plane: Arc<Mutex<Option<ControlPlane>>>,
    task: JoinHandle<Result<(), SupervisorError>>,
}

impl ChannelSupervisor {
    /// Spawns the one supervisor actor and waits for its first authenticated generation.
    ///
    /// # Errors
    /// Returns a topology, spawn, startup, protocol, cleanup, or actor failure.
    pub async fn start(config: ChannelLaunchConfig) -> Result<Self, SupervisorError> {
        let cancellation = CancellationToken::new();
        let plane = Arc::new(Mutex::new(None));
        let (pid_sender, pid) = watch::channel(None);
        let (started_sender, started_receiver) = oneshot::channel();
        let actor_cancel = cancellation.clone();
        let actor_plane = Arc::clone(&plane);
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
        if matches!(startup, Ok(Ok(Ok(())))) {
            return Ok(Self {
                cancellation,
                pid,
                plane,
                task,
            });
        }
        cancellation.cancel();
        let _ = task.await;
        Err(SupervisorError::Startup)
    }

    /// Returns the currently live child process identifier.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        *self.pid.borrow()
    }

    /// Observes exact runtime ownership in the canonical production tool manager.
    #[must_use]
    pub fn runtime_tools(&self, runtime: &RuntimeKey) -> Option<()> {
        self.plane
            .lock()
            .ok()?
            .as_ref()?
            .contains_runtime(runtime)
            .then_some(())
    }

    /// Requests graceful close, then bounded kill/reap, and joins the actor.
    ///
    /// # Errors
    /// Returns a protocol, cleanup, or actor failure.
    pub async fn shutdown(self) -> Result<(), SupervisorError> {
        self.cancellation.cancel();
        self.task.await.map_err(|_| SupervisorError::Task)?
    }
}

async fn run_supervisor(
    config: ChannelLaunchConfig,
    plane: Arc<Mutex<Option<ControlPlane>>>,
    cancellation: CancellationToken,
    pid_sender: watch::Sender<Option<u32>>,
    started: oneshot::Sender<Result<(), SupervisorError>>,
) -> Result<(), SupervisorError> {
    let mut started = Some(started);
    let mut backoff_ms = CHANNEL_RESTART_BACKOFF_MS;
    let mut terminal = Ok(());
    for generation in 1..=CHANNEL_RESTART_ATTEMPTS_MAX + 1 {
        if cancellation.is_cancelled() {
            break;
        }
        let outcome = run_generation(
            &config,
            generation as u64,
            &plane,
            &cancellation,
            &pid_sender,
            &mut started,
        )
        .await;
        terminal = outcome;
        if cancellation.is_cancelled() || generation > CHANNEL_RESTART_ATTEMPTS_MAX {
            break;
        }
        tokio::select! {
            () = cancellation.cancelled() => break,
            () = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
        }
        backoff_ms = backoff_ms
            .saturating_mul(2)
            .min(CHANNEL_RESTART_BACKOFF_MS_MAX);
    }
    if let Some(sender) = started.take() {
        let _ = sender.send(Err(copy_error(
            terminal.as_ref().err().unwrap_or(&SupervisorError::Startup),
        )));
    }
    terminal
}

struct Generation {
    owner: String,
    number: u64,
    pid: u32,
    bootstrap_id: String,
    shutdown_id: String,
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    stderr: JoinHandle<()>,
    capability: ChannelSessionCapability,
}

async fn run_generation(
    config: &ChannelLaunchConfig,
    generation: u64,
    plane_slot: &Arc<Mutex<Option<ControlPlane>>>,
    cancellation: &CancellationToken,
    pid_sender: &watch::Sender<Option<u32>>,
    started: &mut Option<oneshot::Sender<Result<(), SupervisorError>>>,
) -> Result<(), SupervisorError> {
    let mut session = spawn_generation(config, generation, pid_sender).await?;
    let plane = ControlPlane::new(session.owner.clone(), generation, Arc::clone(&config.tools));
    let install = plane.and_then(|plane| {
        let mut slot = plane_slot.lock().map_err(|_| ControlError::Io)?;
        *slot = Some(plane);
        Ok(())
    });
    if let Err(error) = install {
        let _ = cleanup_generation(config, &mut session, plane_slot, pid_sender).await;
        return Err(error.into());
    }
    let startup = start_generation(config, &mut session, plane_slot).await;
    if startup.is_ok()
        && let Some(sender) = started.take()
    {
        let _ = sender.send(Ok(()));
    }
    let outcome = match startup {
        Ok(()) => supervise_generation(config, &mut session, plane_slot, cancellation).await,
        Err(error) => Err(error),
    };
    let cleanup = cleanup_generation(config, &mut session, plane_slot, pid_sender).await;
    match (outcome, cleanup) {
        (_, Err(error)) | (Err(error), Ok(())) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn spawn_generation(
    config: &ChannelLaunchConfig,
    generation: u64,
    pid_sender: &watch::Sender<Option<u32>>,
) -> Result<Generation, SupervisorError> {
    let bootstrap_id = random_id("bootstrap")?;
    let shutdown_id = random_id("shutdown")?;
    let mut command = sandboxed_child_command(&config.executable, &config.store)?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| SupervisorError::Spawn)?;
    let pid = child.id().ok_or(SupervisorError::Spawn)?;
    let owner = format!("{}-{generation}", config.owner_prefix);
    let sandbox_root = config.store.root().to_str().ok_or(SupervisorError::Spawn)?;
    let runtimes = crate::state_store::ChannelStateStore::new(&config.store)
        .restorable_routes()?
        .into_iter()
        .map(|route| {
            (
                route.agent_id.as_str().to_owned(),
                route.conversation_id.as_str().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let Ok(capability) =
        config
            .authenticator
            .install_scoped(&owner, generation, pid, sandbox_root, &runtimes)
    else {
        reap_failed_spawn(&mut child).await;
        return Err(SupervisorError::Spawn);
    };
    let input = child.stdin.take();
    let output = child.stdout.take();
    let stderr = child.stderr.take();
    let (Some(input), Some(output), Some(stderr)) = (input, output, stderr) else {
        let _ = config.authenticator.revoke(&owner, generation, pid);
        reap_failed_spawn(&mut child).await;
        return Err(SupervisorError::Spawn);
    };
    let _ = pid_sender.send(Some(pid));
    Ok(Generation {
        owner,
        number: generation,
        pid,
        bootstrap_id,
        shutdown_id,
        child,
        input: Some(input),
        output: BufReader::new(output),
        stderr: tokio::spawn(drain_stderr(stderr)),
        capability,
    })
}

async fn start_generation(
    config: &ChannelLaunchConfig,
    session: &mut Generation,
    plane: &Arc<Mutex<Option<ControlPlane>>>,
) -> Result<(), SupervisorError> {
    let root = config.store.root().to_str().ok_or(SupervisorError::Spawn)?;
    let frame = ParentFrame::Bootstrap {
        request_id: session.bootstrap_id.clone(),
        owner: session.owner.clone(),
        websocket_url: config.websocket_url.clone(),
        token: session.capability.expose_for_pipe().to_owned(),
        channels_root: root.to_owned(),
    };
    write_line(
        session.input.as_mut().ok_or(SupervisorError::Cleanup)?,
        &frame,
    )
    .await?;
    let frame = tokio::time::timeout(
        Duration::from_millis(CHANNEL_STARTUP_DEADLINE_MS),
        read_line::<_, ChildFrame>(&mut session.output),
    )
    .await
    .map_err(|_| SupervisorError::Startup)??
    .ok_or(SupervisorError::Startup)?;
    match &frame {
        ChildFrame::Ready {
            correlation_id,
            pid,
            owner,
        } if correlation_id == &session.bootstrap_id
            && *pid == session.pid
            && owner == &session.owner => {}
        _ => return Err(SupervisorError::Startup),
    }
    dispatch(plane, frame, &[])?;
    Ok(())
}

async fn supervise_generation(
    config: &ChannelLaunchConfig,
    session: &mut Generation,
    plane: &Arc<Mutex<Option<ControlPlane>>>,
    cancellation: &CancellationToken,
) -> Result<(), SupervisorError> {
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Ok(()),
            frame = read_line::<_, ChildFrame>(&mut session.output) => {
                let Some(frame) = frame? else { return Err(SupervisorError::Cleanup); };
                let channels = config.store.channel_state()?;
                if let Some(response) = dispatch(plane, frame, &channels)? {
                    let input = session.input.as_mut().ok_or(SupervisorError::Cleanup)?;
                    write_line(input, &response).await?;
                }
            }
            status = session.child.wait() => {
                status.map_err(|_| SupervisorError::Cleanup)?;
                return Err(SupervisorError::Cleanup);
            }
        }
    }
}

fn dispatch(
    plane: &Arc<Mutex<Option<ControlPlane>>>,
    frame: ChildFrame,
    channels: &[crate::control_plane::ChannelState],
) -> Result<Option<ParentFrame>, SupervisorError> {
    plane
        .lock()
        .map_err(|_| SupervisorError::Task)?
        .as_mut()
        .ok_or(SupervisorError::Task)?
        .dispatch(frame, channels)
        .map_err(Into::into)
}

async fn cleanup_generation(
    config: &ChannelLaunchConfig,
    session: &mut Generation,
    plane: &Arc<Mutex<Option<ControlPlane>>>,
    pid_sender: &watch::Sender<Option<u32>>,
) -> Result<(), SupervisorError> {
    let shutdown_id = session.shutdown_id.clone();
    let mut shutdown_failed = false;
    let child_live = if let Ok(status) = session.child.try_wait() {
        status.is_none()
    } else {
        shutdown_failed = true;
        false
    };
    if child_live && let Some(input) = &mut session.input {
        let wrote = write_line(
            input,
            &ParentFrame::Shutdown {
                request_id: shutdown_id.clone(),
            },
        )
        .await
        .is_ok();
        let acknowledged = wrote
            && matches!(
                tokio::time::timeout(
                    Duration::from_millis(CHANNEL_SHUTDOWN_GRACE_MS),
                    wait_shutdown_ack(&mut session.output, &session.owner, &shutdown_id, plane),
                )
                .await,
                Ok(Ok(()))
            );
        shutdown_failed |= !acknowledged;
    }
    session.input.take();
    let _ = config
        .authenticator
        .revoke(&session.owner, session.number, session.pid);
    if let Ok(mut slot) = plane.lock()
        && let Some(current) = slot.take()
    {
        let _ = current.release_stale();
    }
    let _ = pid_sender.send(None);
    let process = finish_generation_process(session).await;
    if shutdown_failed {
        Err(SupervisorError::Cleanup)
    } else {
        process
    }
}

async fn finish_generation_process(session: &mut Generation) -> Result<(), SupervisorError> {
    let wait = tokio::time::timeout(
        Duration::from_millis(CHANNEL_SHUTDOWN_GRACE_MS),
        session.child.wait(),
    )
    .await;
    let mut failed = matches!(wait, Ok(Err(_)));
    if wait.is_err() {
        failed |= session.child.kill().await.is_err();
        failed |= session.child.wait().await.is_err();
    }
    if tokio::time::timeout(
        Duration::from_millis(CHANNEL_SHUTDOWN_GRACE_MS),
        &mut session.stderr,
    )
    .await
    .is_err()
    {
        session.stderr.abort();
        failed |= (&mut session.stderr).await.is_err();
    }
    if failed {
        Err(SupervisorError::Cleanup)
    } else {
        Ok(())
    }
}

async fn wait_shutdown_ack<R>(
    output: &mut BufReader<R>,
    owner: &str,
    correlation: &str,
    plane: &Arc<Mutex<Option<ControlPlane>>>,
) -> Result<(), SupervisorError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    loop {
        let frame = read_line::<_, ChildFrame>(output)
            .await?
            .ok_or(SupervisorError::Cleanup)?;
        let acknowledged = matches!(&frame, ChildFrame::ShutdownComplete {
            correlation_id, owner: frame_owner,
        } if correlation_id == correlation && frame_owner == owner);
        dispatch(plane, frame, &[])?;
        if acknowledged {
            return Ok(());
        }
    }
}

async fn reap_failed_spawn(child: &mut Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn drain_stderr<R>(mut stderr: R)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunk = [0_u8; CHANNEL_STDERR_CHUNK_BYTES];
    let mut total = 0_usize;
    let mut line = 0_usize;
    loop {
        let remaining = CHANNEL_STDERR_TOTAL_BYTES_MAX.saturating_sub(total);
        if remaining == 0 {
            break;
        }
        let limit = remaining.min(chunk.len());
        let Ok(length) = stderr.read(&mut chunk[..limit]).await else {
            break;
        };
        if length == 0 {
            break;
        }
        total += length;
        for byte in &chunk[..length] {
            if *byte == b'\n' {
                line = 0;
            } else {
                line = line.saturating_add(1).min(CHANNEL_STDERR_LINE_BYTES_MAX);
            }
        }
    }
}

fn random_id(prefix: &str) -> Result<String, SupervisorError> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| SupervisorError::Spawn)?;
    let mut value = String::with_capacity(prefix.len() + 33);
    value.push_str(prefix);
    value.push('-');
    for byte in random {
        use std::fmt::Write as _;
        write!(&mut value, "{byte:02x}").map_err(|_| SupervisorError::Spawn)?;
    }
    Ok(value)
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

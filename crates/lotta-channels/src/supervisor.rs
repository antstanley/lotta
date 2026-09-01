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

/// Non-secret structured evidence emitted at actual supervisor boundaries.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ChannelSupervisorEvent {
    /// The child authenticated its dedicated socket and returned its correlated ready frame.
    ChildConnected {
        /// Exact child owner.
        owner: String,
        /// Exact child generation.
        generation: u64,
        /// Operating-system child identifier.
        pid: u32,
    },
    /// Active child filesystem probes completed at the arranged scopes.
    StartupIsolation {
        /// Exact child owner.
        owner: String,
        /// Exact child generation.
        generation: u64,
        /// A write below the channels root succeeded.
        channels_write: bool,
        /// A write at the parent backend sibling was denied.
        backend_sibling_write_denied: bool,
    },
    /// The actual canonical `/channels` request was answered.
    ChannelsResponse {
        /// Exact child owner.
        owner: String,
        /// Exact child generation.
        generation: u64,
        /// Exact bounded canonical response rows.
        channels: Vec<crate::control_plane::ChannelState>,
    },
    /// The canonical external manager installed and selected an exact runtime tool set.
    RuntimeToolsPublished {
        /// Exact child owner.
        owner: String,
        /// Exact child generation.
        generation: u64,
        /// Exact runtime receiving the registration.
        runtime: RuntimeKey,
        /// Names observed from the manager's selected registrations after publication.
        manager_tools: Vec<String>,
    },
    /// The dedicated Runtime listener installed `MessageChannel` in the same canonical manager.
    MessageChannelRegistered {
        /// Exact child owner.
        owner: String,
        /// Exact child generation.
        generation: u64,
        /// Exact runtime receiving the registration.
        runtime: RuntimeKey,
        /// Names observed from the manager after the Runtime start frame was applied.
        manager_tools: Vec<String>,
    },
    /// The canonical manager released one runtime's exact generation.
    RuntimeToolsReleased {
        /// Exact child owner.
        owner: String,
        /// Exact child generation.
        generation: u64,
        /// Number of runtime registrations removed.
        released: usize,
    },
    /// The exact generation capability was revoked in the bound authenticator.
    CapabilityRevoked {
        /// Exact child owner.
        owner: String,
        /// Exact child generation.
        generation: u64,
        /// Operating-system child identifier.
        pid: u32,
    },
    /// The exact child was waited and is no longer owned by the supervisor.
    ChildReaped {
        /// Exact child owner.
        owner: String,
        /// Exact child generation.
        generation: u64,
        /// Operating-system child identifier.
        pid: u32,
    },
}

/// Non-panicking observer for structured supervisor evidence.
pub type ChannelSupervisorObserver = Arc<dyn Fn(ChannelSupervisorEvent) + Send + Sync>;

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
    /// Optional structured non-secret evidence observer.
    pub observer: Option<ChannelSupervisorObserver>,
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
        metadata: crate::control_plane::FrameMetadata::new(session.number),
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
            channels_probe_success,
            parent_sibling_probe_denied,
            ..
        } if correlation_id == &session.bootstrap_id
            && *pid == session.pid
            && owner == &session.owner
            && *channels_probe_success
            && *parent_sibling_probe_denied => {}
        _ => return Err(SupervisorError::Startup),
    }
    dispatch(config, plane, frame, &[])?;
    emit(
        config,
        ChannelSupervisorEvent::ChildConnected {
            owner: session.owner.clone(),
            generation: session.number,
            pid: session.pid,
        },
    );
    emit(
        config,
        ChannelSupervisorEvent::StartupIsolation {
            owner: session.owner.clone(),
            generation: session.number,
            channels_write: true,
            backend_sibling_write_denied: true,
        },
    );
    Ok(())
}

async fn supervise_generation(
    config: &ChannelLaunchConfig,
    session: &mut Generation,
    plane: &Arc<Mutex<Option<ControlPlane>>>,
    cancellation: &CancellationToken,
) -> Result<(), SupervisorError> {
    let mut observed_message_channels = std::collections::BTreeSet::new();
    let mut observation_tick = tokio::time::interval(Duration::from_millis(10));
    observation_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Ok(()),
            frame = read_line::<_, ChildFrame>(&mut session.output) => {
                let Some(frame) = frame? else { return Err(SupervisorError::Cleanup); };
                let channels = config.store.channel_state()?;
                if let Some(response) = dispatch(config, plane, frame, &channels)? {
                    let input = session.input.as_mut().ok_or(SupervisorError::Cleanup)?;
                    write_line(input, &response).await?;
                }
            }
            _ = observation_tick.tick() => {
                observe_message_channels(config, session, &mut observed_message_channels)?;
            }
            status = session.child.wait() => {
                status.map_err(|_| SupervisorError::Cleanup)?;
                return Err(SupervisorError::Cleanup);
            }
        }
    }
}

fn observe_message_channels(
    config: &ChannelLaunchConfig,
    session: &Generation,
    observed: &mut std::collections::BTreeSet<RuntimeKey>,
) -> Result<(), SupervisorError> {
    for route in crate::state_store::ChannelStateStore::new(&config.store).restorable_routes()? {
        let runtime = RuntimeKey {
            agent_id: route.agent_id.as_str().to_owned(),
            conversation_id: route.conversation_id.as_str().to_owned(),
        };
        if observed.contains(&runtime) {
            continue;
        }
        let canonical = lotta_tools::external::ChannelRuntimeKey {
            agent_id: runtime.agent_id.clone(),
            conversation_id: runtime.conversation_id.clone(),
        };
        let Some(registrations) = config.tools.runtime_registrations(&canonical) else {
            continue;
        };
        let manager_tools = registrations
            .into_iter()
            .map(|registration| registration.definition.model_name.as_str().to_owned())
            .collect::<Vec<_>>();
        if manager_tools != ["MessageChannel"] {
            continue;
        }
        observed.insert(runtime.clone());
        emit(
            config,
            ChannelSupervisorEvent::MessageChannelRegistered {
                owner: session.owner.clone(),
                generation: session.number,
                runtime,
                manager_tools,
            },
        );
    }
    Ok(())
}

fn dispatch(
    config: &ChannelLaunchConfig,
    plane: &Arc<Mutex<Option<ControlPlane>>>,
    frame: ChildFrame,
    channels: &[crate::control_plane::ChannelState],
) -> Result<Option<ParentFrame>, SupervisorError> {
    let publication = match &frame {
        ChildFrame::PublishRuntimeTools { runtime, .. } => Some(runtime.clone()),
        _ => None,
    };
    let runtime_release = matches!(frame, ChildFrame::ReleaseRuntimeTools { .. });
    let channels_request = matches!(frame, ChildFrame::Channels { .. });
    let response = plane
        .lock()
        .map_err(|_| SupervisorError::Task)?
        .as_mut()
        .ok_or(SupervisorError::Task)?
        .dispatch(frame, channels)?;
    if channels_request {
        emit(
            config,
            ChannelSupervisorEvent::ChannelsResponse {
                owner: plane_owner(plane)?,
                generation: plane_generation(plane)?,
                channels: channels.to_vec(),
            },
        );
    }
    if runtime_release {
        emit(
            config,
            ChannelSupervisorEvent::RuntimeToolsReleased {
                owner: plane_owner(plane)?,
                generation: plane_generation(plane)?,
                released: 1,
            },
        );
    }
    if let Some(runtime) = publication {
        let canonical = lotta_tools::external::ChannelRuntimeKey {
            agent_id: runtime.agent_id.clone(),
            conversation_id: runtime.conversation_id.clone(),
        };
        let manager_tools = config
            .tools
            .runtime_registrations(&canonical)
            .ok_or(SupervisorError::Task)?
            .into_iter()
            .map(|registration| registration.definition.model_name.as_str().to_owned())
            .collect();
        emit(
            config,
            ChannelSupervisorEvent::RuntimeToolsPublished {
                owner: plane_owner(plane)?,
                generation: plane_generation(plane)?,
                runtime,
                manager_tools,
            },
        );
    }
    Ok(response)
}

fn plane_owner(plane: &Arc<Mutex<Option<ControlPlane>>>) -> Result<String, SupervisorError> {
    plane
        .lock()
        .map_err(|_| SupervisorError::Task)?
        .as_ref()
        .map(ControlPlane::owner)
        .ok_or(SupervisorError::Task)
}

fn plane_generation(plane: &Arc<Mutex<Option<ControlPlane>>>) -> Result<u64, SupervisorError> {
    plane
        .lock()
        .map_err(|_| SupervisorError::Task)?
        .as_ref()
        .map(ControlPlane::generation)
        .ok_or(SupervisorError::Task)
}

fn emit(config: &ChannelLaunchConfig, event: ChannelSupervisorEvent) {
    if let Some(observer) = &config.observer {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| observer(event)));
    }
}

async fn cleanup_generation(
    config: &ChannelLaunchConfig,
    session: &mut Generation,
    plane: &Arc<Mutex<Option<ControlPlane>>>,
    pid_sender: &watch::Sender<Option<u32>>,
) -> Result<(), SupervisorError> {
    request_graceful_shutdown(config, session, plane).await?;
    session.input.take();
    let released = if let Ok(mut slot) = plane.lock()
        && let Some(current) = slot.take()
    {
        current.release_stale()
    } else {
        0
    };
    if released > 0 {
        emit(
            config,
            ChannelSupervisorEvent::RuntimeToolsReleased {
                owner: session.owner.clone(),
                generation: session.number,
                released,
            },
        );
    }
    if config
        .authenticator
        .revoke(&session.owner, session.number, session.pid)
    {
        emit(
            config,
            ChannelSupervisorEvent::CapabilityRevoked {
                owner: session.owner.clone(),
                generation: session.number,
                pid: session.pid,
            },
        );
    }
    let _ = pid_sender.send(None);
    finish_generation_process(session).await?;
    emit(
        config,
        ChannelSupervisorEvent::ChildReaped {
            owner: session.owner.clone(),
            generation: session.number,
            pid: session.pid,
        },
    );
    Ok(())
}

async fn request_graceful_shutdown(
    config: &ChannelLaunchConfig,
    session: &mut Generation,
    plane: &Arc<Mutex<Option<ControlPlane>>>,
) -> Result<(), SupervisorError> {
    if session
        .child
        .try_wait()
        .map_err(|_| SupervisorError::Cleanup)?
        .is_some()
    {
        return Ok(());
    }
    let Some(input) = &mut session.input else {
        return Ok(());
    };
    let shutdown_id = session.shutdown_id.clone();
    let wrote = write_line(
        input,
        &ParentFrame::Shutdown {
            metadata: crate::control_plane::FrameMetadata::new(session.number),
            owner: session.owner.clone(),
            request_id: shutdown_id.clone(),
        },
    )
    .await
    .is_ok();
    if wrote {
        let acknowledgement = wait_shutdown_ack(
            config,
            &mut session.output,
            &session.owner,
            &shutdown_id,
            plane,
        );
        let _ = tokio::time::timeout(
            Duration::from_millis(CHANNEL_SHUTDOWN_GRACE_MS),
            acknowledgement,
        )
        .await;
    }
    Ok(())
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
    config: &ChannelLaunchConfig,
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
            correlation_id, owner: frame_owner, ..
        } if correlation_id == correlation && frame_owner == owner);
        dispatch(config, plane, frame, &[])?;
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
    while let Ok(length) = stderr.read(&mut chunk).await {
        if length == 0 {
            break;
        }
        // Always drain to EOF so a noisy child cannot block on a full pipe. No child
        // bytes are retained here; diagnostics remain bounded at the observing edge.
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

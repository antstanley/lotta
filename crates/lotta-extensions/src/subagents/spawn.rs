//! Pinned Letta Code process validation, shell-free launching, and parent manager.

use super::confinement::{ConfinementPlan, ReflectionMergeError, ReflectionWorktreePort};
use super::snapshot::{
    SUBAGENT_STREAM_EVENT_BYTES_MAX, StatusError, StatusPort, StreamEvent, SubagentSnapshot,
    TaskState, serialize_event, serialize_snapshot,
};
use super::types::{ContextResolver, SubagentRequest, SubagentType};
#[cfg(test)]
use super::types::{ModelPolicy, ToolPolicy};
use crate::sidecar::framing::{read_frame, write_frame};
use crate::sidecar::handshake::{SidecarSessionPolicy, ValidatedSidecarReader};
use crate::sidecar::{SIDECAR_PROTOCOL_VERSION, SidecarFrameLimit};
use crate::sidecar::{
    SidecarCapability, SidecarEnvelope, SidecarEnvelopeKind, SidecarOwnerIdentity,
};
use lotta_runtime::boundary::{CommitMessage, WorktreeId};
use lotta_runtime::ports::MemFsPort;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Canonical total task bound per parent.
pub const SUBAGENTS_PER_PARENT_MAX: usize = 128;
/// Canonical concurrent child bound per parent.
pub const SUBAGENTS_CONCURRENT_PER_PARENT_MAX: usize = 16;
/// Canonical child stdout/stderr aggregate bound.
pub const CHILD_PROCESS_OUTPUT_BYTES_MAX: usize = 16 * 1_024 * 1_024;
/// Maximum bounded manager commands waiting per parent.
pub const SUBAGENT_QUEUE_ITEMS_MAX: usize = 128;
/// Backwards-compatible public name for the canonical queue ceiling.
pub const SUBAGENT_MANAGER_QUEUE_ITEMS_MAX: usize = SUBAGENT_QUEUE_ITEMS_MAX;
/// Maximum retained terminal task results per parent.
pub const SUBAGENT_RESULTS_RETAINED_MAX: usize = 128;
/// Maximum retained final report bytes.
pub const SUBAGENT_RESULT_BYTES_MAX: usize = 256 * 1_024;
/// Bounded child process event channel capacity.
pub const SUBAGENT_PROCESS_EVENT_QUEUE_ITEMS_MAX: usize = 128;
/// Maximum child startup/call time accepted by this adapter.
pub const SUBAGENT_TIMEOUT_MS_DEFAULT: u64 = 300_000;
/// Exact pinned package version.
pub const PINNED_LETTA_CODE_VERSION: &str = "0.30.20";
/// Exact pinned baseline commit.
pub const PINNED_LETTA_CODE_COMMIT: &str = "300f923f16cc8eee50656d7da732902c1dea2b65";
/// Exact package manifest SHA-256 at the pinned baseline.
pub const PINNED_PACKAGE_SHA256: &str =
    "9bd4323b8b055c07fbfdf6755b3454e013abdbadd6049f2bc1209be228f76b00";
/// Exact pinned Bun lockfile SHA-256.
pub const PINNED_LOCKFILE_SHA256: &str =
    "0a8cad33168b97cfd08958d29b853f683eff9a36f42d1709e761990a74adc26e";
/// Task 46 sidecar frame ceiling.
pub const SUBAGENT_FRAME_BYTES_MAX: usize = 4 * 1_024 * 1_024;

/// Validated absolute source/runtime inputs with no PATH fallback.
#[derive(Clone, Debug)]
pub struct PinnedLettaCodeSpec {
    /// Explicit Bun executable.
    pub bun_executable: PathBuf,
    /// Exact pinned source checkout.
    pub source_root: PathBuf,
    /// Explicit sandboxed child working directory.
    pub cwd: PathBuf,
    /// Explicit environment, replacing ambient environment.
    pub environment: BTreeMap<String, String>,
}

/// Stable adapter failure without child-controlled contents.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SpawnError {
    /// Pinned source/runtime validation failed.
    #[error("pinned Letta Code validation failed")]
    Pin,
    /// Parent task limit was reached.
    #[error("subagent parent task limit reached")]
    TotalLimit,
    /// Manager or child process is unavailable.
    #[error("subagent unavailable")]
    Unavailable,
    /// Child protocol failed.
    #[error("subagent protocol failed")]
    Protocol,
    /// Child failed before its Hello frame; diagnostic is bounded and redacted.
    #[error("subagent failed before Hello: {diagnostic}")]
    PreHello {
        /// Secret-safe bounded adapter stderr diagnostic.
        diagnostic: String,
    },
    /// Child was cancelled and reaped.
    #[error("subagent cancelled")]
    Cancelled,
    /// Child exceeded a configured bound.
    #[error("subagent bound exceeded")]
    Bound,
    /// Reflection succeeded but its retained worktree could not merge into primary memory.
    #[error("reflection merge conflict; worktree retained: {worktree}: {diagnostic}")]
    ReflectionConflict {
        /// Retained diagnostic worktree.
        worktree: String,
        /// Stable merge diagnostic.
        diagnostic: String,
    },
}

impl From<StatusError> for SpawnError {
    fn from(_: StatusError) -> Self {
        Self::Unavailable
    }
}

impl PinnedLettaCodeSpec {
    /// Canonicalizes and validates source version, manifest hash, and explicit executable.
    pub fn validate(&self) -> Result<Self, SpawnError> {
        let bun = canonical_file(&self.bun_executable)?;
        let root = canonical_directory(&self.source_root)?;
        let cwd = canonical_directory(&self.cwd)?;
        let package = root.join("package.json");
        let bytes = std::fs::read(&package).map_err(|_| SpawnError::Pin)?;
        if format!("{:x}", Sha256::digest(&bytes)) != PINNED_PACKAGE_SHA256 {
            return Err(SpawnError::Pin);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| SpawnError::Pin)?;
        if value.get("version").and_then(serde_json::Value::as_str)
            != Some(PINNED_LETTA_CODE_VERSION)
        {
            return Err(SpawnError::Pin);
        }
        validate_checkout_head(&root)?;
        validate_file_hash(&root.join("bun.lock"), PINNED_LOCKFILE_SHA256)?;
        let entry = root.join("src/index.ts");
        if !entry.is_file()
            || self
                .environment
                .iter()
                .any(|(key, value)| invalid_env(key, value))
        {
            return Err(SpawnError::Pin);
        }
        Ok(Self {
            bun_executable: bun,
            source_root: root,
            cwd,
            environment: self.environment.clone(),
        })
    }

    /// Runs the exact pinned source version command without consulting PATH.
    pub async fn version(&self) -> Result<String, SpawnError> {
        let spec = self.validate()?;
        let output = Command::new(&spec.bun_executable)
            .arg(spec.source_root.join("scripts/dev.cjs"))
            .arg("--version")
            .current_dir(&spec.source_root)
            .env_clear()
            .envs(&spec.environment)
            .output()
            .await
            .map_err(|_| SpawnError::Unavailable)?;
        if !output.status.success() {
            return Err(SpawnError::Pin);
        }
        let version = String::from_utf8(output.stdout).map_err(|_| SpawnError::Pin)?;
        let version = version.trim();
        let expected = format!("{PINNED_LETTA_CODE_VERSION} (Letta Code)");
        if version != expected {
            return Err(SpawnError::Pin);
        }
        Ok(PINNED_LETTA_CODE_VERSION.into())
    }

    fn command(&self, request: &SubagentRequest) -> Result<Command, SpawnError> {
        let spec = self.validate()?;
        let plan = ConfinementPlan::new(
            request.subagent_type,
            &request.filesystem_roots,
            request.memory_scope.as_ref(),
            request.reflection_worktree.as_deref(),
        )
        .map_err(|_| SpawnError::Protocol)?;
        let adapter = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/subagents/task42_adapter.ts")
            .canonicalize()
            .map_err(|_| SpawnError::Unavailable)?;
        let mut inner = vec![
            spec.bun_executable.to_string_lossy().into_owned(),
            "run".into(),
            adapter.to_string_lossy().into_owned(),
            "--bun".into(),
            spec.bun_executable.to_string_lossy().into_owned(),
            "--source-root".into(),
            spec.source_root.to_string_lossy().into_owned(),
            "--cwd".into(),
            spec.cwd.to_string_lossy().into_owned(),
        ];
        let (program, arguments) = sandbox_command(&spec, &plan, &adapter, &mut inner)?;
        let mut command = Command::new(program);
        command
            .args(arguments)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(&spec.environment)
            .env("LETTA_CODE_AGENT_ROLE", "subagent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        Ok(command)
    }
}

/// One child stream result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubagentResult {
    /// Collected final stream-json result text when present.
    pub report: String,
    /// Whether the child exited successfully.
    pub success: bool,
    /// Secret-safe bounded adapter stderr, retained for test diagnostics.
    pub diagnostic: String,
}

/// Production launcher seam used by the parent manager.
pub trait SubagentLauncher: Send + Sync {
    /// Launch future.
    type Run<'a>: Future<Output = Result<SubagentResult, SpawnError>> + Send + 'a
    where
        Self: 'a;

    /// Runs one child and delivers non-empty ordered stream events.
    fn run(
        &self,
        task_id: u64,
        request: SubagentRequest,
        owner: SidecarOwnerIdentity,
        cancellation: CancellationToken,
        events: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Self::Run<'_>;
}

/// Shell-free pinned process launcher.
#[derive(Clone)]
pub struct PinnedProcessLauncher {
    spec: PinnedLettaCodeSpec,
}
impl PinnedProcessLauncher {
    /// Validates the pinned process source eagerly.
    pub fn new(spec: PinnedLettaCodeSpec) -> Result<Self, SpawnError> {
        Ok(Self {
            spec: spec.validate()?,
        })
    }
}
impl SubagentLauncher for PinnedProcessLauncher {
    type Run<'a> = Pin<Box<dyn Future<Output = Result<SubagentResult, SpawnError>> + Send + 'a>>;

    fn run(
        &self,
        task_id: u64,
        request: SubagentRequest,
        owner: SidecarOwnerIdentity,
        cancellation: CancellationToken,
        events: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Self::Run<'_> {
        Box::pin(async move {
            request.validate().map_err(|_| SpawnError::Protocol)?;
            let mut command = self.spec.command(&request)?;
            let mut child = command.spawn().map_err(|_| SpawnError::Unavailable)?;
            write_bridge_request(&mut child, task_id, &request, owner.clone()).await?;
            collect_child(task_id, child, owner, cancellation, events).await
        })
    }
}

/// Parent-scoped immutable task handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskHandle {
    /// Parent-scoped task ID.
    pub task_id: u64,
}

/// Foreground calls retain their result; background calls return a task handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpawnOutcome {
    /// Foreground terminal result.
    Completed(SubagentResult),
    /// Background task identity.
    Background(TaskHandle),
}

#[derive(Clone)]
struct TaskRecord {
    owner_agent_id: String,
    state: TaskState,
    cancellation: CancellationToken,
    result: Option<Result<SubagentResult, SpawnError>>,
    completion: Arc<tokio::sync::Notify>,
}

struct ManagerState {
    next_id: u64,
    tasks: BTreeMap<u64, TaskRecord>,
    terminal_order: VecDeque<u64>,
}

/// Atomic parent-scoped task manager with FIFO semaphore admission.
pub struct SubagentManager<L, S, C = NoContextResolver> {
    launcher: Arc<L>,
    status: Arc<S>,
    context_resolver: Arc<C>,
    reflection: Option<Arc<ReflectionWorktreePort<dyn MemFsPort>>>,
    owner: SidecarOwnerIdentity,
    owner_agent_id: String,
    state: Arc<Mutex<ManagerState>>,
    dispatcher: mpsc::Sender<DispatchCommand<L, S>>,
}

/// Resolver used only by legacy constructors; production composition injects a real resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoContextResolver;
impl ContextResolver for NoContextResolver {
    fn resolve<'a>(
        &'a self,
        _: &'a SubagentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::types::RequestError>> + Send + 'a>>
    {
        Box::pin(async { Ok(None) })
    }
}

impl<L, S> SubagentManager<L, S, NoContextResolver>
where
    L: SubagentLauncher + 'static,
    S: StatusPort + 'static,
{
    /// Creates one manager for one exact parent owner.
    #[must_use]
    pub fn new(
        launcher: Arc<L>,
        status: Arc<S>,
        owner: SidecarOwnerIdentity,
        owner_agent_id: String,
    ) -> Self {
        Self::with_context_resolver(
            launcher,
            status,
            Arc::new(NoContextResolver),
            owner,
            owner_agent_id,
        )
    }
}

impl<L, S, C> SubagentManager<L, S, C>
where
    L: SubagentLauncher + 'static,
    S: StatusPort + 'static,
    C: ContextResolver + 'static,
{
    /// Creates production composition with context resolved before launch.
    #[must_use]
    pub fn with_context_resolver(
        launcher: Arc<L>,
        status: Arc<S>,
        context_resolver: Arc<C>,
        owner: SidecarOwnerIdentity,
        owner_agent_id: String,
    ) -> Self {
        Self {
            launcher,
            status,
            context_resolver,
            reflection: None,
            owner,
            owner_agent_id,
            state: Arc::new(Mutex::new(ManagerState {
                next_id: 1,
                tasks: BTreeMap::new(),
                terminal_order: VecDeque::new(),
            })),
            dispatcher: dispatcher_channel(),
        }
    }

    /// Installs the production reflection worktree lifecycle port.
    #[must_use]
    pub fn with_reflection_worktree(
        mut self,
        reflection: Arc<ReflectionWorktreePort<dyn MemFsPort>>,
    ) -> Self {
        self.reflection = Some(reflection);
        self
    }

    /// Atomically admits one task. The seventeenth waits FIFO; the 129th is rejected.
    pub async fn spawn(&self, mut request: SubagentRequest) -> Result<SpawnOutcome, SpawnError> {
        request.validate().map_err(|_| SpawnError::Protocol)?;
        request.resolved_context = self
            .context_resolver
            .resolve(&request)
            .await
            .map_err(|_| SpawnError::Protocol)?;
        request.validate().map_err(|_| SpawnError::Protocol)?;
        let (task_id, cancellation) = self.admit()?;
        self.publish(task_id, Vec::new()).await?;
        let background = request.background;
        let handle = TaskHandle { task_id };
        let (done_sender, done_receiver) = oneshot::channel();
        self.enqueue_task(task_id, request, cancellation, done_sender)
            .await?;
        if background {
            Ok(SpawnOutcome::Background(handle))
        } else {
            done_receiver.await.map_err(|_| SpawnError::Unavailable)??;
            Ok(SpawnOutcome::Completed(self.await_result(task_id).await?))
        }
    }

    /// Cancels one exact owned task and waits until its process tree is reaped.
    pub async fn stop(&self, task_id: u64) -> Result<(), SpawnError> {
        let completion = {
            let state = self.state.lock().map_err(|_| SpawnError::Unavailable)?;
            let task = state.tasks.get(&task_id).ok_or(SpawnError::Unavailable)?;
            task.cancellation.cancel();
            Arc::clone(&task.completion)
        };
        loop {
            if self.task_state(task_id)?.is_terminal() {
                return Ok(());
            }
            completion.notified().await;
        }
    }

    /// Waits for and returns the bounded retained terminal result.
    pub async fn await_result(&self, task_id: u64) -> Result<SubagentResult, SpawnError> {
        let completion = {
            let state = self.state.lock().map_err(|_| SpawnError::Unavailable)?;
            Arc::clone(
                &state
                    .tasks
                    .get(&task_id)
                    .ok_or(SpawnError::Unavailable)?
                    .completion,
            )
        };
        loop {
            if let Some(result) = self.task_output(task_id)? {
                return result;
            }
            completion.notified().await;
        }
    }

    /// Returns a retained result, or `None` while the task remains active.
    pub fn task_output(
        &self,
        task_id: u64,
    ) -> Result<Option<Result<SubagentResult, SpawnError>>, SpawnError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| SpawnError::Unavailable)?
            .tasks
            .get(&task_id)
            .ok_or(SpawnError::Unavailable)?
            .result
            .clone())
    }

    /// Returns an immutable state snapshot for one task.
    pub fn task_state(&self, task_id: u64) -> Result<TaskState, SpawnError> {
        self.state
            .lock()
            .map_err(|_| SpawnError::Unavailable)?
            .tasks
            .get(&task_id)
            .map(|task| task.state)
            .ok_or(SpawnError::Unavailable)
    }

    fn admit(&self) -> Result<(u64, CancellationToken), SpawnError> {
        let mut state = self.state.lock().map_err(|_| SpawnError::Unavailable)?;
        if state.tasks.len() >= SUBAGENTS_PER_PARENT_MAX {
            evict_oldest_terminal(&mut state);
        }
        if state.tasks.len() >= SUBAGENTS_PER_PARENT_MAX {
            return Err(SpawnError::TotalLimit);
        }
        let task_id = state.next_id;
        state.next_id = state.next_id.checked_add(1).ok_or(SpawnError::Bound)?;
        let cancellation = CancellationToken::new();
        state.tasks.insert(
            task_id,
            TaskRecord {
                owner_agent_id: self.owner_agent_id.clone(),
                state: TaskState::Queued,
                cancellation: cancellation.clone(),
                result: None,
                completion: Arc::new(tokio::sync::Notify::new()),
            },
        );
        Ok((task_id, cancellation))
    }

    async fn enqueue_task(
        &self,
        task_id: u64,
        request: SubagentRequest,
        cancellation: CancellationToken,
        done: oneshot::Sender<Result<(), SpawnError>>,
    ) -> Result<(), SpawnError> {
        self.dispatcher
            .send(DispatchCommand {
                context: ManagedContext {
                    launcher: Arc::clone(&self.launcher),
                    status: Arc::clone(&self.status),
                    state: Arc::clone(&self.state),
                    owner: self.owner.clone(),
                    reflection: self.reflection.clone(),
                },
                task_id,
                request,
                cancellation,
                done,
            })
            .await
            .map_err(|_| SpawnError::Unavailable)
    }

    async fn publish(&self, task_id: u64, events: Vec<StreamEvent>) -> Result<(), SpawnError> {
        publish_snapshot(&self.state, self.status.as_ref(), task_id, events).await
    }
}

struct ManagedContext<L, S> {
    launcher: Arc<L>,
    status: Arc<S>,
    state: Arc<Mutex<ManagerState>>,
    owner: SidecarOwnerIdentity,
    reflection: Option<Arc<ReflectionWorktreePort<dyn MemFsPort>>>,
}

struct DispatchCommand<L, S> {
    context: ManagedContext<L, S>,
    task_id: u64,
    request: SubagentRequest,
    cancellation: CancellationToken,
    done: oneshot::Sender<Result<(), SpawnError>>,
}

fn dispatcher_channel<L, S>() -> mpsc::Sender<DispatchCommand<L, S>>
where
    L: SubagentLauncher + 'static,
    S: StatusPort + 'static,
{
    let (sender, mut receiver) = mpsc::channel::<DispatchCommand<L, S>>(SUBAGENT_QUEUE_ITEMS_MAX);
    tokio::spawn(async move {
        let permits = Arc::new(tokio::sync::Semaphore::new(
            SUBAGENTS_CONCURRENT_PER_PARENT_MAX,
        ));
        while let Some(command) = receiver.recv().await {
            let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
                break;
            };
            tokio::spawn(async move {
                let result = run_managed(
                    command.context,
                    command.task_id,
                    command.request,
                    command.cancellation,
                )
                .await;
                drop(permit);
                let _ = command.done.send(result);
            });
        }
    });
    sender
}

async fn run_managed<L: SubagentLauncher, S: StatusPort>(
    context: ManagedContext<L, S>,
    task_id: u64,
    mut request: SubagentRequest,
    cancellation: CancellationToken,
) -> Result<(), SpawnError> {
    if cancellation.is_cancelled() {
        store_terminal(
            &context.state,
            task_id,
            TaskState::Cancelled,
            Err(SpawnError::Cancelled),
        )?;
        publish_snapshot(&context.state, context.status.as_ref(), task_id, Vec::new()).await?;
        return Err(SpawnError::Cancelled);
    }
    let reflection = if request.subagent_type == SubagentType::Reflection {
        let Some(port) = context.reflection.as_ref() else {
            return terminate_before_launch(&context, task_id, SpawnError::Unavailable).await;
        };
        let Ok(created) = port.create().await else {
            return terminate_before_launch(&context, task_id, SpawnError::Unavailable).await;
        };
        if port.confine_request(&mut request, &created.1).is_err() {
            return terminate_before_launch(&context, task_id, SpawnError::Protocol).await;
        }
        Some(created.0)
    } else {
        None
    };
    set_state(&context.state, task_id, TaskState::Running)?;
    publish_snapshot(&context.state, context.status.as_ref(), task_id, Vec::new()).await?;
    let (event_sender, mut event_receiver) =
        tokio::sync::mpsc::channel(SUBAGENT_PROCESS_EVENT_QUEUE_ITEMS_MAX);
    let silent = request.silent;
    let run = context.launcher.run(
        task_id,
        request,
        context.owner.clone(),
        cancellation.clone(),
        event_sender,
    );
    tokio::pin!(run);
    let result = loop {
        tokio::select! {
            event = event_receiver.recv() => {
                if let Some(event) = event && !silent {
                    context.status
                        .publish_subagent_event(serialize_event(&event)?)
                        .await?;
                }
            }
            result = &mut run => {
                while let Some(event) = event_receiver.recv().await {
                    if !silent {
                        context.status
                            .publish_subagent_event(serialize_event(&event)?)
                            .await?;
                    }
                }
                break result;
            },
        }
    };
    let result = merge_reflection(&context, reflection.as_ref(), result, &cancellation).await;
    finalize(
        &context.state,
        context.status.as_ref(),
        task_id,
        result,
        cancellation,
    )
    .await
}

async fn terminate_before_launch<L, S: StatusPort>(
    context: &ManagedContext<L, S>,
    task_id: u64,
    error: SpawnError,
) -> Result<(), SpawnError> {
    store_terminal(
        &context.state,
        task_id,
        TaskState::Failed,
        Err(error.clone()),
    )?;
    publish_snapshot(&context.state, context.status.as_ref(), task_id, Vec::new()).await?;
    Err(error)
}

async fn merge_reflection<L, S>(
    context: &ManagedContext<L, S>,
    worktree: Option<&WorktreeId>,
    result: Result<SubagentResult, SpawnError>,
    cancellation: &CancellationToken,
) -> Result<SubagentResult, SpawnError> {
    let Some(worktree) = worktree else {
        return result;
    };
    if cancellation.is_cancelled() || !result.as_ref().is_ok_and(|value| value.success) {
        return result;
    }
    let port = context.reflection.as_ref().ok_or(SpawnError::Unavailable)?;
    let message = CommitMessage::new("Merge successful reflection subagent".into())
        .map_err(|_| SpawnError::Protocol)?;
    match port.merge(worktree, &message).await {
        Ok(_) => result,
        Err(ReflectionMergeError::Conflict {
            worktree,
            diagnostic,
        }) => Err(SpawnError::ReflectionConflict {
            worktree: worktree.as_str().into(),
            diagnostic,
        }),
    }
}

async fn finalize<S: StatusPort>(
    state: &Arc<Mutex<ManagerState>>,
    status: &S,
    task_id: u64,
    result: Result<SubagentResult, SpawnError>,
    cancellation: CancellationToken,
) -> Result<(), SpawnError> {
    let terminal = if cancellation.is_cancelled() || matches!(result, Err(SpawnError::Cancelled)) {
        TaskState::Cancelled
    } else if result.as_ref().is_ok_and(|value| value.success) {
        TaskState::Completed
    } else {
        TaskState::Failed
    };
    let returned = result.clone().map(|_| ());
    store_terminal(state, task_id, terminal, result)?;
    publish_snapshot(state, status, task_id, Vec::new()).await?;
    returned
}

fn store_terminal(
    state: &Arc<Mutex<ManagerState>>,
    task_id: u64,
    next: TaskState,
    result: Result<SubagentResult, SpawnError>,
) -> Result<(), SpawnError> {
    let completion = {
        let mut state = state.lock().map_err(|_| SpawnError::Unavailable)?;
        let task = state
            .tasks
            .get_mut(&task_id)
            .ok_or(SpawnError::Unavailable)?;
        let result = result.map(|mut value| {
            if value.report.len() > SUBAGENT_RESULT_BYTES_MAX {
                value.report.truncate(SUBAGENT_RESULT_BYTES_MAX);
            }
            value
        });
        task.state = next;
        task.result = Some(result);
        let completion = Arc::clone(&task.completion);
        state.terminal_order.push_back(task_id);
        while state.terminal_order.len() > SUBAGENT_RESULTS_RETAINED_MAX {
            evict_oldest_terminal(&mut state);
        }
        completion
    };
    completion.notify_waiters();
    Ok(())
}

fn set_state(
    state: &Arc<Mutex<ManagerState>>,
    task_id: u64,
    next: TaskState,
) -> Result<(), SpawnError> {
    let mut state = state.lock().map_err(|_| SpawnError::Unavailable)?;
    let task = state
        .tasks
        .get_mut(&task_id)
        .ok_or(SpawnError::Unavailable)?;
    task.state = next;
    Ok(())
}

async fn publish_snapshot<S: StatusPort>(
    state: &Arc<Mutex<ManagerState>>,
    status: &S,
    task_id: u64,
    events: Vec<StreamEvent>,
) -> Result<(), SpawnError> {
    let snapshot = {
        let state = state.lock().map_err(|_| SpawnError::Unavailable)?;
        let task = state.tasks.get(&task_id).ok_or(SpawnError::Unavailable)?;
        SubagentSnapshot {
            task_id,
            owner_agent_id: task.owner_agent_id.clone(),
            state: task.state,
            task_count: state.tasks.len(),
            active_task_count: state
                .tasks
                .values()
                .filter(|task| task.state == TaskState::Running)
                .count(),
            events,
        }
    };
    status
        .update_subagent_state(serialize_snapshot(&snapshot)?)
        .await?;
    Ok(())
}

async fn write_bridge_request(
    child: &mut Child,
    task_id: u64,
    request: &SubagentRequest,
    owner: SidecarOwnerIdentity,
) -> Result<(), SpawnError> {
    let mut stdin = child.stdin.take().ok_or(SpawnError::Unavailable)?;
    let request_id = format!("task46-{task_id}");
    let hello = SidecarEnvelope {
        version: SIDECAR_PROTOCOL_VERSION,
        owner: owner.clone(),
        capability: SidecarCapability::Subagent,
        timeout_ms: SUBAGENT_TIMEOUT_MS_DEFAULT,
        request_id: request_id.clone(),
        correlation_id: None,
        kind: SidecarEnvelopeKind::Hello,
        payload: serde_json::json!({"protocol":"task42","adapter":"letta-code-0.30.20"}),
    };
    let envelope = SidecarEnvelope {
        version: SIDECAR_PROTOCOL_VERSION,
        owner,
        capability: SidecarCapability::Subagent,
        timeout_ms: SUBAGENT_TIMEOUT_MS_DEFAULT,
        request_id: request_id.clone(),
        correlation_id: Some(request_id),
        kind: SidecarEnvelopeKind::Request,
        payload: serde_json::to_value(request).map_err(|_| SpawnError::Protocol)?,
    };
    let limit = SidecarFrameLimit::bounded(SUBAGENT_FRAME_BYTES_MAX);
    write_frame(&mut stdin, limit, &hello)
        .await
        .map_err(|_| SpawnError::Protocol)?;
    write_frame(&mut stdin, limit, &envelope)
        .await
        .map_err(|_| SpawnError::Protocol)?;
    stdin.shutdown().await.map_err(|_| SpawnError::Unavailable)
}

async fn collect_child(
    task_id: u64,
    mut child: Child,
    owner: SidecarOwnerIdentity,
    cancellation: CancellationToken,
    events: tokio::sync::mpsc::Sender<StreamEvent>,
) -> Result<SubagentResult, SpawnError> {
    let stdout = child.stdout.take().ok_or(SpawnError::Unavailable)?;
    let stderr = child.stderr.take().ok_or(SpawnError::Unavailable)?;
    let stderr_task = tokio::spawn(read_bounded(stderr));
    let limit = SidecarFrameLimit::bounded(SUBAGENT_FRAME_BYTES_MAX);
    let mut stdout = BufReader::new(stdout);
    let hello: SidecarEnvelope = if let Ok(hello) = read_frame(&mut stdout, limit).await {
        hello
    } else {
        terminate(&mut child);
        let _ = child.wait().await;
        let stderr = stderr_task.await.unwrap_or_default();
        return Err(SpawnError::PreHello {
            diagnostic: safe_stderr_diagnostic(&stderr),
        });
    };
    let policy = SidecarSessionPolicy::new(
        SIDECAR_PROTOCOL_VERSION,
        owner,
        [SidecarCapability::Subagent],
        SUBAGENT_TIMEOUT_MS_DEFAULT,
        limit,
    );
    let mut hello_wire = Vec::new();
    write_frame(&mut hello_wire, limit, &hello)
        .await
        .map_err(|_| SpawnError::Protocol)?;
    let replay = std::io::Cursor::new(hello_wire).chain(stdout);
    let mut reader = ValidatedSidecarReader::new(replay, policy);
    reader
        .accept_handshake()
        .await
        .map_err(|_| SpawnError::Protocol)?;
    let mut total = 0_usize;
    let mut sequence = 0_u64;
    let mut report = String::new();
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                terminate(&mut child);
                let _ = child.wait().await;
                let _ = stderr_task.await;
                return Err(SpawnError::Cancelled);
            }
            read = reader.read_payload() => {
                let envelope = match read {
                    Ok(envelope) => envelope,
                    Err(crate::sidecar::handshake::SidecarSessionError::Io) => break,
                    Err(_) => {
                        terminate(&mut child);
                        let _ = child.wait().await;
                        return Err(SpawnError::Protocol);
                    }
                };
                let bytes = serde_json::to_vec(&envelope).map_err(|_| SpawnError::Protocol)?;
                if bytes.len() > SUBAGENT_STREAM_EVENT_BYTES_MAX {
                    terminate(&mut child);
                    let _ = child.wait().await;
                    return Err(SpawnError::Bound);
                }
                total = total.checked_add(bytes.len()).ok_or(SpawnError::Bound)?;
                if total > CHILD_PROCESS_OUTPUT_BYTES_MAX {
                    terminate(&mut child);
                    let _ = child.wait().await;
                    return Err(SpawnError::Bound);
                }
                sequence = sequence.checked_add(1).ok_or(SpawnError::Bound)?;
                if envelope.kind == SidecarEnvelopeKind::Response
                    && let Some(text) = envelope.payload.get("result").and_then(serde_json::Value::as_str)
                { report = text.into(); }
                events.send(StreamEvent { task_id, sequence, bytes })
                    .await.map_err(|_| SpawnError::Unavailable)?;
            }
        }
    }
    let status = child.wait().await.map_err(|_| SpawnError::Unavailable)?;
    let stderr = stderr_task.await.map_err(|_| SpawnError::Unavailable)?;
    if total
        .checked_add(stderr.len())
        .is_none_or(|bytes| bytes > CHILD_PROCESS_OUTPUT_BYTES_MAX)
    {
        return Err(SpawnError::Bound);
    }
    Ok(SubagentResult {
        report,
        success: status.success(),
        diagnostic: safe_stderr_diagnostic(&stderr),
    })
}

fn safe_stderr_diagnostic(stderr: &[u8]) -> String {
    const DIAGNOSTIC_BYTES_MAX: usize = 4 * 1_024;
    let bounded = &stderr[..stderr.len().min(DIAGNOSTIC_BYTES_MAX)];
    String::from_utf8_lossy(bounded)
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                '�'
            } else {
                character
            }
        })
        .collect()
}

async fn read_bounded<R: tokio::io::AsyncRead + Unpin>(reader: R) -> Vec<u8> {
    let mut reader = BufReader::new(reader).take(CHILD_PROCESS_OUTPUT_BYTES_MAX as u64);
    let mut output = Vec::new();
    let _ = reader.read_to_end(&mut output).await;
    output
}

#[cfg(test)]
pub(crate) fn pinned_arguments(request: &SubagentRequest) -> Vec<String> {
    let mut args = vec![
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--include-partial-messages".into(),
    ];
    match (
        &request.existing_conversation_id,
        &request.existing_agent_id,
    ) {
        (Some(conversation), _) => args.extend(["--conv".into(), conversation.as_str().into()]),
        (None, Some(agent)) => {
            args.extend(["--agent".into(), agent.as_str().into(), "--new".into()]);
        }
        (None, None) => args.extend([
            "--new-agent".into(),
            "--system".into(),
            type_name(request.subagent_type).into(),
        ]),
    }
    match &request.model {
        ModelPolicy::Explicit(model) | ModelPolicy::Inherit(model) => {
            args.extend(["--model".into(), model.clone()]);
        }
        ModelPolicy::Auto => {}
        ModelPolicy::AutoFast => {
            args.extend(["--model".into(), "haiku-4.5".into()]);
        }
    }
    if let ToolPolicy::Only(tools) = &request.tools {
        args.extend(["--tools".into(), tools.join(",")]);
    }
    if let Some(turns) = request.max_turns {
        args.extend(["--max-turns".into(), turns.to_string()]);
    }
    if request.subagent_type == SubagentType::Reflection {
        args.extend(["--no-system-info-reminder".into(), "--no-skills".into()]);
    }
    if matches!(
        request.subagent_type,
        SubagentType::Reflection
            | SubagentType::Memory
            | SubagentType::HistoryAnalyzer
            | SubagentType::Init
    ) {
        args.extend(["--base-tools".into(), "none".into()]);
    }
    args
}

#[cfg(test)]
fn type_name(value: SubagentType) -> &'static str {
    match value {
        SubagentType::GeneralPurpose => "general-purpose",
        SubagentType::Fork => "fork",
        SubagentType::Recall => "recall",
        SubagentType::Reflection => "reflection",
        SubagentType::Memory => "memory",
        SubagentType::HistoryAnalyzer => "history-analyzer",
        SubagentType::Init => "init",
    }
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    if let Some(group) = child.id().and_then(|id| i32::try_from(id).ok()) {
        use rustix::process::{Pid, Signal, kill_process_group};
        if let Some(group) = Pid::from_raw(group) {
            let _ = kill_process_group(group, Signal::KILL);
        }
    }
    let _ = child.start_kill();
}

fn evict_oldest_terminal(state: &mut ManagerState) {
    while let Some(task_id) = state.terminal_order.pop_front() {
        if state
            .tasks
            .get(&task_id)
            .is_some_and(|task| task.state.is_terminal())
        {
            state.tasks.remove(&task_id);
            break;
        }
    }
}

fn sandbox_command(
    spec: &PinnedLettaCodeSpec,
    plan: &ConfinementPlan,
    adapter: &Path,
    inner: &mut Vec<String>,
) -> Result<(PathBuf, Vec<String>), SpawnError> {
    let mut readonly = vec![
        spec.bun_executable.clone(),
        spec.source_root.clone(),
        adapter
            .parent()
            .ok_or(SpawnError::Unavailable)?
            .to_path_buf(),
    ];
    if let Some(home) = spec.environment.get("HOME") {
        readonly.push(canonicalize_existing_or_parent(Path::new(home))?);
    }
    if let Some(cache) = spec.environment.get("BUN_INSTALL_CACHE_DIR") {
        readonly.push(PathBuf::from(cache));
    }
    readonly.extend(plan.filesystem_roots().iter().cloned());
    readonly.extend(plan.memory_readonly_roots().iter().cloned());
    let mut writable = plan.memory_writable_roots().to_vec();
    writable.push(spec.cwd.clone());
    if let Some(home) = spec.environment.get("HOME") {
        writable.push(canonicalize_existing_or_parent(Path::new(home))?);
    }
    if let Some(temp) = spec.environment.get("TMPDIR") {
        writable.push(canonicalize_existing_or_parent(Path::new(temp))?);
    }
    if let Some(storage) = spec.environment.get("LETTA_CODE_DEV_BACKEND_DIR") {
        writable.push(canonicalize_existing_or_parent(Path::new(storage))?);
    }
    #[cfg(target_os = "macos")]
    {
        let backend = PathBuf::from(lotta_tools::sandbox::SEATBELT_PROGRAM);
        if !backend.is_file() {
            return Err(SpawnError::Unavailable);
        }
        let profile = seatbelt_profile(&readonly, &writable)?;
        let mut arguments = vec!["-p".into(), profile, "--".into()];
        arguments.append(inner);
        Ok((backend, arguments))
    }
    #[cfg(target_os = "linux")]
    {
        let backend = resolve_bwrap().ok_or(SpawnError::Unavailable)?;
        let mut arguments = vec![
            "--die-with-parent".into(),
            "--unshare-net".into(),
            "--dev".into(),
            "/dev".into(),
            "--proc".into(),
            "/proc".into(),
            "--tmpfs".into(),
            "/tmp".into(),
        ];
        for root in readonly {
            let value = root.to_string_lossy().into_owned();
            arguments.extend(["--ro-bind".into(), value.clone(), value]);
        }
        for root in &writable {
            let value = root.to_string_lossy().into_owned();
            arguments.extend(["--bind".into(), value.clone(), value]);
        }
        arguments.push("--".into());
        arguments.append(inner);
        Ok((backend, arguments))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (spec, plan, adapter, inner, readonly, writable);
        Err(SpawnError::Unavailable)
    }
}

#[cfg(target_os = "macos")]
fn seatbelt_profile(readonly: &[PathBuf], writable: &[PathBuf]) -> Result<String, SpawnError> {
    let mut profile = String::from(
        "(version 1)\n(deny default)\n(allow process*)\n(allow sysctl-read)\n(allow mach-lookup)\n(allow signal)\n(allow file-read-metadata)\n(allow file-read-data (literal \"/\"))\n(allow file-read-data (regex #\"^/(usr|System|Library|dev)(/|$)\"))\n(allow file-read-data (subpath \"/private/etc\"))\n(allow file-read-data (subpath \"/private/var/db\"))\n(allow file-read-data (subpath \"/private/var/run\"))\n",
    );
    for root in readonly {
        profile.push_str("(allow file-read* (subpath \"");
        profile.push_str(&escape_sbpl(root)?);
        profile.push_str("\"))\n");
    }
    for root in writable {
        profile.push_str("(allow file-read* file-write* (subpath \"");
        profile.push_str(&escape_sbpl(root)?);
        profile.push_str("\"))\n");
    }
    profile.push_str("(allow file-read* file-write* (subpath \"/dev\"))\n");
    Ok(profile)
}

#[cfg(target_os = "macos")]
fn canonicalize_existing_or_parent(path: &Path) -> Result<PathBuf, SpawnError> {
    if path.exists() {
        return path.canonicalize().map_err(|_| SpawnError::Unavailable);
    }
    let parent = path.parent().ok_or(SpawnError::Unavailable)?;
    let name = path.file_name().ok_or(SpawnError::Unavailable)?;
    Ok(parent
        .canonicalize()
        .map_err(|_| SpawnError::Unavailable)?
        .join(name))
}

#[cfg(target_os = "macos")]
fn escape_sbpl(path: &Path) -> Result<String, SpawnError> {
    let value = path.to_str().ok_or(SpawnError::Unavailable)?;
    if value.contains(['\0', '\n', '\r']) {
        return Err(SpawnError::Unavailable);
    }
    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "linux")]
fn resolve_bwrap() -> Option<PathBuf> {
    ["/usr/bin/bwrap", "/bin/bwrap"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

fn validate_checkout_head(root: &Path) -> Result<(), SpawnError> {
    let output = std::process::Command::new("/usr/bin/git")
        .args(["-C"])
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .env_clear()
        .output()
        .map_err(|_| SpawnError::Pin)?;
    if !output.status.success()
        || String::from_utf8(output.stdout)
            .map_err(|_| SpawnError::Pin)?
            .trim()
            != PINNED_LETTA_CODE_COMMIT
    {
        return Err(SpawnError::Pin);
    }
    Ok(())
}

fn validate_file_hash(path: &Path, expected: &str) -> Result<(), SpawnError> {
    let bytes = std::fs::read(path).map_err(|_| SpawnError::Pin)?;
    if format!("{:x}", Sha256::digest(bytes)) == expected {
        Ok(())
    } else {
        Err(SpawnError::Pin)
    }
}

fn canonical_file(path: &Path) -> Result<PathBuf, SpawnError> {
    let path = path.canonicalize().map_err(|_| SpawnError::Pin)?;
    if path.is_file() {
        Ok(path)
    } else {
        Err(SpawnError::Pin)
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, SpawnError> {
    let path = path.canonicalize().map_err(|_| SpawnError::Pin)?;
    if path.is_dir() {
        Ok(path)
    } else {
        Err(SpawnError::Pin)
    }
}

fn invalid_env(key: &str, value: &str) -> bool {
    key.is_empty() || key.contains(['=', '\0']) || value.contains('\0')
}

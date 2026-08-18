use lotta_domain::RuntimeScope;
use lotta_runtime::bounds::TOOL_RESULT_BYTES_MAX;
use lotta_runtime::{
    RuntimeError,
    boundary::{
        ConfinedPath, EnvironmentEntry, EnvironmentName, EnvironmentValue, ProcessArgument,
        ProcessArguments, ProcessEnvironment, ProcessOutputBytesMax, ProcessStdin, Program,
    },
    ports::{
        InteractiveSandboxPort, ProcessEvent, ProcessInput, ProcessOutcome, ProcessRequest,
        ProcessSession, SandboxPort,
    },
};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

/// Aggregate stdout and stderr capture ceiling.
pub const CHILD_PROCESS_OUTPUT_BYTES_MAX: usize = 16 * 1024 * 1024;
/// SIGTERM-to-SIGKILL grace for shell process groups.
pub const SHELL_CHILD_KILL_GRACE_MS: u64 = crate::sandbox::SHELL_CHILD_KILL_GRACE_MS;
/// Maximum retained process sessions.
pub const SHELL_SESSIONS_MAX: usize = 32;
/// Maximum session identifier bytes.
pub const SHELL_SESSION_ID_BYTES_MAX: usize = 64;
/// Maximum command bytes accepted by a shell tool.
pub const SHELL_COMMAND_BYTES_MAX: usize = 256 * 1024;
/// Maximum retained session output, below raw tool admission.
pub const SHELL_SESSION_OUTPUT_BYTES_MAX: usize = 1_000_000;
/// Maximum stdin bytes per write.
pub const SHELL_STDIN_WRITE_BYTES_MAX: usize = 64 * 1024;
/// Maximum aggregate stdin bytes per session.
pub const SHELL_STDIN_TOTAL_BYTES_MAX: usize = 1024 * 1024;
const PROCESS_EVENT_CHANNEL_ITEMS_MAX: usize = 16;
#[cfg(test)]
pub(super) const PROCESS_CHANNEL_ITEMS_MAX: usize = PROCESS_EVENT_CHANNEL_ITEMS_MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

struct Session {
    owner: Option<TurnOwner>,
    status: SessionStatus,
    output: VecDeque<u8>,
    aggregate_output_bytes: usize,
    peak_retained_bytes: usize,
    exit_code: Option<i32>,
    stdin_bytes: usize,
    cancellation: CancellationToken,
    input: Option<mpsc::Sender<ProcessInput>>,
    join: Option<tokio::task::JoinHandle<()>>,
    terminal: Option<ManagerError>,
    stopped: bool,
    notify: Arc<Notify>,
    read_offset: usize,
    started: tokio::time::Instant,
    ordinal: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TurnOwner {
    pub(super) scope: RuntimeScope,
    pub(super) lease_generation: u64,
}

pub(super) struct ProcessManager {
    workspace_root: PathBuf,
    scope: RuntimeScope,
    sandbox: Arc<dyn ShellSandbox>,
    sessions: Arc<Mutex<BTreeMap<String, Session>>>,
    next_id: AtomicU64,
    launch_owner: Mutex<Option<TurnOwner>>,
}

pub(super) struct LaunchOptions<'a> {
    pub(super) interactive: bool,
    pub(super) shell: Option<&'a str>,
    pub(super) login: bool,
}

impl ProcessManager {
    pub(super) fn new(root: PathBuf, scope: RuntimeScope, sandbox: Arc<dyn ShellSandbox>) -> Self {
        Self {
            workspace_root: root,
            scope,
            sandbox,
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            next_id: AtomicU64::new(1),
            launch_owner: Mutex::new(None),
        }
    }

    pub(super) async fn one_shot(
        &self,
        command: &str,
        cwd: &Path,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Result<RunResult, ManagerError> {
        validate_command(command)?;
        let request = self.request(command, cwd, timeout)?;
        let (sender, receiver) = mpsc::channel(PROCESS_EVENT_CHANNEL_ITEMS_MAX);
        let run = self.sandbox.execute(request, sender, cancellation.clone());
        tokio::pin!(run);
        let collect = collect_events(receiver);
        tokio::pin!(collect);
        let cancelled = cancellation.clone();
        let (outcome, bytes) = tokio::try_join!(
            async { run.await.map_err(|error| map_runtime(&error)) },
            collect,
        )?;
        if cancelled.is_cancelled() {
            return Err(ManagerError::Interrupted);
        }
        if outcome.timed_out {
            return Err(ManagerError::Timeout);
        }
        if bytes.len() > TOOL_RESULT_BYTES_MAX.value {
            return Err(ManagerError::Limit);
        }
        Ok(RunResult {
            output: text(&bytes),
            exit_code: outcome.exit_code,
        })
    }

    pub(super) async fn start(
        &self,
        command: &str,
        cwd: &Path,
        timeout: Duration,
        prefix: &str,
        interactive: bool,
    ) -> Result<String, ManagerError> {
        self.start_launcher(
            command,
            cwd,
            timeout,
            prefix,
            LaunchOptions {
                interactive,
                shell: None,
                login: true,
            },
        )
        .await
    }

    pub(super) async fn start_launcher(
        &self,
        command: &str,
        cwd: &Path,
        timeout: Duration,
        prefix: &str,
        options: LaunchOptions<'_>,
    ) -> Result<String, ManagerError> {
        let LaunchOptions {
            interactive,
            shell,
            login,
        } = options;
        validate_command(command)?;
        validate_prefix(prefix)?;
        let id = self.allocate_id(prefix)?;
        let cancellation = CancellationToken::new();
        self.reserve(&id, command, cancellation.clone(), interactive)?;
        let request = match self.request_with_launcher(command, cwd, timeout, (shell, login)) {
            Ok(request) => request,
            Err(error) => {
                self.rollback(&id);
                return Err(error);
            }
        };
        let (input, events, terminal) = if interactive {
            let session = match self
                .sandbox
                .start_session(request, cancellation.clone())
                .await
            {
                Ok(session) => session,
                Err(error) => {
                    self.rollback(&id);
                    return Err(map_runtime(&error));
                }
            };
            split_session(session)?
        } else {
            let (sender, receiver) = mpsc::channel(PROCESS_EVENT_CHANNEL_ITEMS_MAX);
            let sandbox = Arc::clone(&self.sandbox);
            let token = cancellation.clone();
            let terminal =
                tokio::spawn(async move { sandbox.execute(request, sender, token).await });
            (None, receiver, terminal)
        };
        if let Err(error) = self.attach_input(&id, input) {
            cancellation.cancel();
            let _ = terminal.await;
            self.rollback(&id);
            return Err(error);
        }
        let sessions = Arc::clone(&self.sessions);
        let id_for_task = id.clone();
        let join = tokio::spawn(async move {
            let outcome_task = async {
                terminal.await.map_err(|_| RuntimeError::AdapterFailure {
                    code: "session_task",
                    context: "shell session".into(),
                })?
            };
            let collect_task =
                collect_session_events(Arc::clone(&sessions), id_for_task.clone(), events);
            let (outcome, collected) = tokio::join!(outcome_task, collect_task);
            finish_session(&sessions, &id_for_task, outcome, collected);
        });
        self.attach_join(&id, join).await?;
        Ok(id)
    }

    pub(super) fn output(&self, id: &str) -> Result<(String, SessionStatus), ManagerError> {
        validate_id(id)?;
        let guard = self
            .sessions
            .lock()
            .map_err(|_| ManagerError::Infrastructure)?;
        let session = guard.get(id).ok_or(ManagerError::Unknown)?;
        Ok((deque_text(&session.output), session.status))
    }

    pub(super) async fn wait_output(
        &self,
        id: &str,
        timeout: Duration,
    ) -> Result<(String, SessionStatus), ManagerError> {
        let notify = {
            let guard = self
                .sessions
                .lock()
                .map_err(|_| ManagerError::Infrastructure)?;
            let session = guard.get(id).ok_or(ManagerError::Unknown)?;
            if session.status != SessionStatus::Running || timeout.is_zero() {
                return Ok((deque_text(&session.output), session.status));
            }
            Arc::clone(&session.notify)
        };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return self.output(id);
            }
            let _ = tokio::time::timeout(remaining, notify.notified()).await;
            let current = self.output(id)?;
            if current.1 != SessionStatus::Running || tokio::time::Instant::now() >= deadline {
                return Ok(current);
            }
        }
    }

    pub(super) fn read_exec(&self, id: &str) -> Result<ExecRead, ManagerError> {
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| ManagerError::Infrastructure)?;
        let session = guard.get_mut(id).ok_or(ManagerError::Unknown)?;
        let output = deque_text_from(&session.output, session.read_offset);
        session.read_offset = session.output.len();
        Ok(ExecRead {
            output,
            status: session.status,
            exit_code: session.exit_code,
            terminal: session.terminal,
            wall_time: session.started.elapsed(),
        })
    }

    pub(super) fn release(&self, id: &str) -> Result<(), ManagerError> {
        self.sessions
            .lock()
            .map_err(|_| ManagerError::Infrastructure)?
            .remove(id);
        Ok(())
    }

    pub(super) async fn stop(&self, id: &str) -> Result<bool, ManagerError> {
        validate_id(id)?;
        let join = {
            let mut guard = self
                .sessions
                .lock()
                .map_err(|_| ManagerError::Infrastructure)?;
            let Some(session) = guard.get_mut(id) else {
                return Ok(false);
            };
            if session.status != SessionStatus::Running || session.stopped {
                return Ok(false);
            }
            session.stopped = true;
            session.cancellation.cancel();
            session.join.take()
        };
        if let Some(join) = join {
            join.await.map_err(|_| ManagerError::Infrastructure)?;
        }
        Ok(true)
    }

    pub(super) async fn write(
        &self,
        id: &str,
        bytes: &[u8],
        eof: bool,
    ) -> Result<(), ManagerError> {
        validate_id(id)?;
        if bytes.len() > SHELL_STDIN_WRITE_BYTES_MAX {
            return Err(ManagerError::Limit);
        }
        let sender = {
            let mut guard = self
                .sessions
                .lock()
                .map_err(|_| ManagerError::Infrastructure)?;
            let session = guard.get_mut(id).ok_or(ManagerError::Unknown)?;
            if session.status != SessionStatus::Running {
                return Err(ManagerError::Closed);
            }
            let next = session
                .stdin_bytes
                .checked_add(bytes.len())
                .ok_or(ManagerError::Limit)?;
            if next > SHELL_STDIN_TOTAL_BYTES_MAX {
                return Err(ManagerError::Limit);
            }
            session.stdin_bytes = next;
            session.input.clone().ok_or(ManagerError::Closed)?
        };
        if !bytes.is_empty() {
            let input = ProcessStdin::new(bytes.to_vec()).map_err(|error| map_runtime(&error))?;
            sender
                .send(ProcessInput::Bytes(input))
                .await
                .map_err(|_| ManagerError::Closed)?;
        }
        if eof {
            sender
                .send(ProcessInput::Eof)
                .await
                .map_err(|_| ManagerError::Closed)?;
            let mut guard = self
                .sessions
                .lock()
                .map_err(|_| ManagerError::Infrastructure)?;
            if let Some(session) = guard.get_mut(id) {
                session.input = None;
            }
        }
        Ok(())
    }

    fn reserve(
        &self,
        id: &str,
        _command: &str,
        cancellation: CancellationToken,
        _interactive: bool,
    ) -> Result<(), ManagerError> {
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| ManagerError::Infrastructure)?;
        if guard.len() >= SHELL_SESSIONS_MAX {
            let oldest = guard
                .iter()
                .filter(|(_, session)| session.status != SessionStatus::Running)
                .min_by_key(|(_, session)| session.ordinal)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                guard.remove(&oldest);
            } else {
                return Err(ManagerError::Limit);
            }
        }
        guard.insert(
            id.to_owned(),
            Session {
                owner: self
                    .launch_owner
                    .lock()
                    .ok()
                    .and_then(|owner| owner.clone()),
                status: SessionStatus::Running,
                output: VecDeque::new(),
                aggregate_output_bytes: 0,
                peak_retained_bytes: 0,
                exit_code: None,
                stdin_bytes: 0,
                cancellation,
                input: None,
                join: None,
                terminal: None,
                stopped: false,
                notify: Arc::new(Notify::new()),
                read_offset: 0,
                started: tokio::time::Instant::now(),
                ordinal: self.next_id.load(Ordering::Relaxed),
            },
        );
        Ok(())
    }

    fn rollback(&self, id: &str) {
        if let Ok(mut guard) = self.sessions.lock() {
            guard.remove(id);
        }
    }

    fn attach_input(
        &self,
        id: &str,
        input: Option<mpsc::Sender<ProcessInput>>,
    ) -> Result<(), ManagerError> {
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| ManagerError::Infrastructure)?;
        guard.get_mut(id).ok_or(ManagerError::Unknown)?.input = input;
        Ok(())
    }

    async fn attach_join(
        &self,
        id: &str,
        join: tokio::task::JoinHandle<()>,
    ) -> Result<(), ManagerError> {
        let mut owned = Some(join);
        let attached = self
            .sessions
            .lock()
            .ok()
            .and_then(|mut guard| guard.get_mut(id).map(|session| session.join = owned.take()))
            .is_some();
        if attached {
            return Ok(());
        }
        if let Some(join) = owned {
            let cancellation = self
                .sessions
                .lock()
                .ok()
                .and_then(|guard| guard.get(id).map(|session| session.cancellation.clone()));
            if let Some(cancellation) = cancellation {
                cancellation.cancel();
            }
            let _ = join.await;
        }
        self.rollback(id);
        Err(ManagerError::Infrastructure)
    }

    pub(super) fn set_launch_owner(&self, owner: TurnOwner) -> Result<(), ManagerError> {
        *self
            .launch_owner
            .lock()
            .map_err(|_| ManagerError::Infrastructure)? = Some(owner);
        Ok(())
    }

    pub(super) fn has_operations(&self) -> bool {
        self.sessions.lock().is_ok_and(|guard| !guard.is_empty())
    }

    pub(super) fn has_owned_operations(&self, owner: &TurnOwner) -> bool {
        self.sessions.lock().is_ok_and(|guard| {
            guard
                .values()
                .any(|session| session.owner.as_ref() == Some(owner))
        })
    }

    #[cfg(test)]
    pub(super) fn session_count(&self) -> usize {
        self.sessions.lock().map_or(0, |guard| guard.len())
    }

    #[cfg(test)]
    pub(super) fn inspect(&self, id: &str) -> Option<SessionInspection> {
        let guard = self.sessions.lock().ok()?;
        let session = guard.get(id)?;
        Some(SessionInspection {
            status: session.status,
            terminal: session.terminal,
            aggregate_bytes: session.aggregate_output_bytes,
            retained_bytes: session.output.len(),
            read_offset: session.read_offset,
            session_count: guard.len(),
            join_present: session.join.is_some(),
            join_finished: session
                .join
                .as_ref()
                .is_none_or(tokio::task::JoinHandle::is_finished),
            peak_retained_bytes: session.peak_retained_bytes,
        })
    }

    pub(super) async fn shutdown_owner(&self, owner: &TurnOwner) -> Result<(), ManagerError> {
        let joins = {
            let mut guard = self
                .sessions
                .lock()
                .map_err(|_| ManagerError::Infrastructure)?;
            let ids = guard
                .iter()
                .filter(|(_, session)| session.owner.as_ref() == Some(owner))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            let mut joins = Vec::new();
            for id in ids {
                if let Some(mut session) = guard.remove(&id) {
                    session.cancellation.cancel();
                    if let Some(join) = session.join.take() {
                        joins.push(join);
                    }
                }
            }
            joins
        };
        for join in joins {
            join.await.map_err(|_| ManagerError::Infrastructure)?;
        }
        Ok(())
    }

    pub(super) async fn shutdown(&self) -> Result<(), ManagerError> {
        let joins = {
            let mut guard = self
                .sessions
                .lock()
                .map_err(|_| ManagerError::Infrastructure)?;
            for session in guard.values() {
                session.cancellation.cancel();
            }
            guard
                .values_mut()
                .filter_map(|session| session.join.take())
                .collect::<Vec<_>>()
        };
        for join in joins {
            join.await.map_err(|_| ManagerError::Infrastructure)?;
        }
        self.sessions
            .lock()
            .map_err(|_| ManagerError::Infrastructure)?
            .clear();
        Ok(())
    }

    fn allocate_id(&self, prefix: &str) -> Result<String, ManagerError> {
        let value = self.next_id.fetch_add(1, Ordering::Relaxed);
        let id = if prefix == "exec" {
            value.to_string()
        } else {
            format!("{prefix}_{:x}_{value}", std::process::id())
        };
        validate_id(&id)?;
        Ok(id)
    }

    fn request(
        &self,
        command: &str,
        cwd: &Path,
        timeout: Duration,
    ) -> Result<ProcessRequest, ManagerError> {
        self.request_with_launcher(command, cwd, timeout, (None, true))
    }

    fn request_with_launcher(
        &self,
        command: &str,
        cwd: &Path,
        timeout: Duration,
        launcher_options: (Option<&str>, bool),
    ) -> Result<ProcessRequest, ManagerError> {
        let (shell, login) = launcher_options;
        let stdin = None;
        let (program, arguments) = launcher(command, shell, login)?;
        let cwd = if cwd.as_os_str().is_empty() {
            self.workspace_root.clone()
        } else if cwd.is_absolute() {
            cwd.to_owned()
        } else {
            self.workspace_root.join(cwd)
        };
        ProcessRequest::new(
            self.scope.clone(),
            Program::new(program).map_err(|error| map_runtime(&error))?,
            arguments,
            ConfinedPath::new(self.workspace_root.clone(), cwd)
                .map_err(|error| map_runtime(&error))?,
            build_environment(Vec::new())?,
            stdin
                .map(ProcessStdin::new)
                .transpose()
                .map_err(|error| map_runtime(&error))?,
            ProcessOutputBytesMax::new(CHILD_PROCESS_OUTPUT_BYTES_MAX)
                .map_err(|error| map_runtime(&error))?,
            timeout,
        )
        .map_err(|error| map_runtime(&error))
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        if let Ok(guard) = self.sessions.lock() {
            for session in guard.values() {
                session.cancellation.cancel();
            }
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(super) struct SessionInspection {
    pub(super) status: SessionStatus,
    pub(super) terminal: Option<ManagerError>,
    pub(super) aggregate_bytes: usize,
    pub(super) retained_bytes: usize,
    pub(super) read_offset: usize,
    pub(super) session_count: usize,
    pub(super) join_present: bool,
    pub(super) join_finished: bool,
    pub(super) peak_retained_bytes: usize,
}

#[derive(Debug)]
pub(super) struct RunResult {
    pub(super) output: String,
    pub(super) exit_code: Option<i32>,
}

pub(super) struct ExecRead {
    pub(super) output: String,
    pub(super) status: SessionStatus,
    pub(super) exit_code: Option<i32>,
    pub(super) terminal: Option<ManagerError>,
    pub(super) wall_time: Duration,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ManagerError {
    Invalid,
    Limit,
    Unknown,
    Closed,
    Timeout,
    Interrupted,
    Spawn,
    Infrastructure,
}

async fn collect_events(
    mut receiver: mpsc::Receiver<ProcessEvent>,
) -> Result<Vec<u8>, ManagerError> {
    let mut output = Vec::new();
    output
        .try_reserve(64 * 1024)
        .map_err(|_| ManagerError::Limit)?;
    while let Some(event) = receiver.recv().await {
        let bytes = match event {
            ProcessEvent::Stdout(value) | ProcessEvent::Stderr(value) => value.into_vec(),
        };
        let next = output
            .len()
            .checked_add(bytes.len())
            .ok_or(ManagerError::Limit)?;
        if next > CHILD_PROCESS_OUTPUT_BYTES_MAX {
            return Err(ManagerError::Limit);
        }
        output
            .try_reserve(bytes.len())
            .map_err(|_| ManagerError::Limit)?;
        output.extend_from_slice(&bytes);
    }
    Ok(output)
}

async fn collect_session_events(
    sessions: Arc<Mutex<BTreeMap<String, Session>>>,
    id: String,
    mut receiver: mpsc::Receiver<ProcessEvent>,
) -> Result<(), ManagerError> {
    while let Some(event) = receiver.recv().await {
        let bytes = match event {
            ProcessEvent::Stdout(value) | ProcessEvent::Stderr(value) => value.into_vec(),
        };
        let mut guard = sessions.lock().map_err(|_| ManagerError::Infrastructure)?;
        let session = guard.get_mut(&id).ok_or(ManagerError::Unknown)?;
        let next = session
            .aggregate_output_bytes
            .checked_add(bytes.len())
            .ok_or(ManagerError::Limit)?;
        if next > CHILD_PROCESS_OUTPUT_BYTES_MAX {
            session.cancellation.cancel();
            return Err(ManagerError::Limit);
        }
        session.aggregate_output_bytes = next;
        append_retained(session, &bytes)?;
        session.notify.notify_waiters();
    }
    Ok(())
}

fn append_retained(session: &mut Session, bytes: &[u8]) -> Result<(), ManagerError> {
    let keep = bytes.len().min(SHELL_SESSION_OUTPUT_BYTES_MAX);
    let incoming = &bytes[bytes.len() - keep..];
    let remove = session
        .output
        .len()
        .saturating_add(incoming.len())
        .saturating_sub(SHELL_SESSION_OUTPUT_BYTES_MAX);
    if remove != 0 {
        session.output.drain(..remove);
        session.read_offset = session.read_offset.saturating_sub(remove);
    }
    session
        .output
        .try_reserve(incoming.len())
        .map_err(|_| ManagerError::Limit)?;
    session.output.extend(incoming.iter().copied());
    session.peak_retained_bytes = session.peak_retained_bytes.max(session.output.len());
    Ok(())
}

fn finish_session(
    sessions: &Mutex<BTreeMap<String, Session>>,
    id: &str,
    outcome: Result<lotta_runtime::ports::ProcessOutcome, RuntimeError>,
    collected: Result<(), ManagerError>,
) {
    let Ok(mut guard) = sessions.lock() else {
        return;
    };
    let Some(session) = guard.get_mut(id) else {
        return;
    };
    if let Err(error) = collected {
        session.terminal = Some(error);
        session.status = SessionStatus::Failed;
    } else if let Ok(result) = outcome {
        session.exit_code = result.exit_code;
        session.status = if session.stopped {
            SessionStatus::Stopped
        } else if result.exit_code == Some(0) {
            SessionStatus::Completed
        } else {
            SessionStatus::Failed
        };
    } else if let Err(error) = outcome {
        session.terminal = Some(map_runtime(&error));
        session.status = if session.stopped {
            SessionStatus::Stopped
        } else {
            SessionStatus::Failed
        };
    }
    session.notify.notify_waiters();
}

/// Combined one-shot and interactive shell sandbox boundary.
pub trait ShellSandbox: SandboxPort + InteractiveSandboxPort {}
impl<T> ShellSandbox for T where T: SandboxPort + InteractiveSandboxPort {}

type SessionParts = (
    Option<mpsc::Sender<ProcessInput>>,
    mpsc::Receiver<ProcessEvent>,
    tokio::task::JoinHandle<Result<ProcessOutcome, RuntimeError>>,
);

fn split_session(session: ProcessSession) -> Result<SessionParts, ManagerError> {
    let (input, events, _, terminal) = session
        .into_parts()
        .map_err(|_| ManagerError::Infrastructure)?;
    Ok((Some(input), events, terminal))
}

fn validate_prefix(value: &str) -> Result<(), ManagerError> {
    if value.is_empty()
        || value.len() > 16
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(ManagerError::Invalid);
    }
    Ok(())
}

fn launcher(
    command: &str,
    selected: Option<&str>,
    login: bool,
) -> Result<(String, ProcessArguments), ManagerError> {
    #[cfg(target_os = "macos")]
    let default = "/bin/zsh";
    #[cfg(not(target_os = "macos"))]
    let default = "/bin/bash";
    let program = selected.unwrap_or(default);
    if !matches!(program, "/bin/bash" | "/bin/zsh" | "/bin/sh") {
        return Err(ManagerError::Invalid);
    }
    let name = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let flag = if login && matches!(name, "bash" | "zsh") {
        "-lc"
    } else {
        "-c"
    };
    let mut args = Vec::new();
    args.try_reserve_exact(2).map_err(|_| ManagerError::Limit)?;
    args.push(ProcessArgument::new(flag.to_owned()).map_err(|error| map_runtime(&error))?);
    args.push(ProcessArgument::new(command.to_owned()).map_err(|error| map_runtime(&error))?);
    Ok((
        program.to_owned(),
        ProcessArguments::new(args).map_err(|_| ManagerError::Limit)?,
    ))
}

fn build_environment(values: Vec<(String, String)>) -> Result<ProcessEnvironment, ManagerError> {
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(values.len() + 7)
        .map_err(|_| ManagerError::Limit)?;
    for (name, value) in [
        (
            "PATH".to_owned(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_owned(),
        ),
        ("HOME".to_owned(), "/tmp".to_owned()),
        ("PAGER".to_owned(), "cat".to_owned()),
        ("GIT_PAGER".to_owned(), "cat".to_owned()),
        ("MANPAGER".to_owned(), "cat".to_owned()),
        ("TERM".to_owned(), "xterm-256color".to_owned()),
        ("COLORTERM".to_owned(), "truecolor".to_owned()),
    ]
    .into_iter()
    .chain(values)
    {
        entries.push(
            EnvironmentEntry::new(
                EnvironmentName::new(name).map_err(|error| map_runtime(&error))?,
                EnvironmentValue::new(value).map_err(|error| map_runtime(&error))?,
            )
            .map_err(|error| map_runtime(&error))?,
        );
    }
    ProcessEnvironment::new(entries).map_err(|_| ManagerError::Limit)
}

pub(super) fn validate_command(value: &str) -> Result<(), ManagerError> {
    if value.is_empty() || value.len() > SHELL_COMMAND_BYTES_MAX || value.contains('\0') {
        return Err(ManagerError::Invalid);
    }
    Ok(())
}
pub(super) fn validate_id(value: &str) -> Result<(), ManagerError> {
    if value.is_empty()
        || value.len() > SHELL_SESSION_ID_BYTES_MAX
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ManagerError::Invalid);
    }
    Ok(())
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
fn deque_text(bytes: &VecDeque<u8>) -> String {
    let (front, back) = bytes.as_slices();
    if back.is_empty() {
        return text(front);
    }
    let mut contiguous = Vec::new();
    if contiguous.try_reserve(bytes.len()).is_err() {
        return String::new();
    }
    contiguous.extend_from_slice(front);
    contiguous.extend_from_slice(back);
    text(&contiguous)
}
fn deque_text_from(bytes: &VecDeque<u8>, offset: usize) -> String {
    let contiguous = bytes.iter().skip(offset).copied().collect::<Vec<_>>();
    text(&contiguous)
}
fn map_runtime(error: &RuntimeError) -> ManagerError {
    match error {
        RuntimeError::Cancelled { .. } => ManagerError::Interrupted,
        RuntimeError::LimitExceeded { .. } => ManagerError::Limit,
        RuntimeError::AdapterFailure { .. } => ManagerError::Spawn,
        _ => ManagerError::Infrastructure,
    }
}

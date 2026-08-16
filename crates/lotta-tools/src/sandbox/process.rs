use super::{
    bwrap::bubblewrap_arguments,
    policy::WorkspacePolicy,
    seatbelt::{SEATBELT_PROGRAM, seatbelt_arguments},
    unsupported::unsupported_workspace_sandbox,
};
use lotta_runtime::{
    RuntimeError,
    boundary::ProcessOutputChunk,
    bounds::PROCESS_OUTPUT_CHUNK_BYTES_MAX,
    ports::{
        InteractiveSandboxPort, PROCESS_TIMEOUT_DISABLED, PortFuture, ProcessEvent, ProcessInput,
        ProcessOutcome, ProcessRequest, ProcessSession, ProcessSessionFuture, SandboxPort,
    },
};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::mpsc::{self, Receiver, Sender},
};
use tokio_util::sync::CancellationToken;

const PROCESS_INTERNAL_EVENTS_MAX: usize = 16;

const PROCESS_INPUT_CHANNEL_ITEMS_MAX: usize = 16;
const PROCESS_EVENT_CHANNEL_ITEMS_MAX: usize = 16;
pub(crate) const SHELL_CHILD_KILL_GRACE_MS: u64 = 2_000;
const PROCESS_GROUP_KILL_RETRIES_MAX: usize = 32;
#[cfg(target_os = "linux")]
const SANDBOX_PATH_ENTRIES_MAX: usize = 256;

/// Selected inspectable OS sandbox backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SandboxBackend {
    /// macOS Seatbelt at its pinned executable.
    Seatbelt,
    /// Linux Bubblewrap at a canonical executable path.
    Bubblewrap {
        /// Canonical Bubblewrap executable.
        program: PathBuf,
    },
    /// Explicit unavailable backend; execution always errors before spawn.
    Unsupported,
}

/// OS-backed workspace sandbox implementing the runtime sandbox port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OsSandbox {
    policy: WorkspacePolicy,
    backend: SandboxBackend,
}

impl OsSandbox {
    /// Detects the one supported current-platform backend.
    #[must_use]
    pub fn detect(policy: WorkspacePolicy) -> Self {
        let backend = detect_backend();
        Self { policy, backend }
    }

    /// Creates a deterministic backend selection for tests and explicit wiring.
    #[must_use]
    pub const fn with_backend(policy: WorkspacePolicy, backend: SandboxBackend) -> Self {
        Self { policy, backend }
    }

    /// Returns the selected backend.
    #[must_use]
    pub const fn backend(&self) -> &SandboxBackend {
        &self.backend
    }
}

impl InteractiveSandboxPort for OsSandbox {
    fn start_session(
        &self,
        mut request: ProcessRequest,
        cancellation: CancellationToken,
    ) -> ProcessSessionFuture<'_> {
        Box::pin(async move {
            if self.backend == SandboxBackend::Unsupported {
                return Err(unsupported_workspace_sandbox());
            }
            let cwd = validate_request(&self.policy, &request)?;
            if cancellation.is_cancelled() {
                return Err(cancelled());
            }
            request.stdin = None;
            let (input_sender, input_receiver) = mpsc::channel(PROCESS_INPUT_CHANNEL_ITEMS_MAX);
            let (event_sender, event_receiver) = mpsc::channel(PROCESS_EVENT_CHANNEL_ITEMS_MAX);
            let policy = self.policy.clone();
            let backend = self.backend.clone();
            let child_cancellation = cancellation.clone();
            let terminal = tokio::spawn(async move {
                run_interactive_child(
                    &policy,
                    &backend,
                    request,
                    cwd,
                    input_receiver,
                    event_sender,
                    child_cancellation,
                )
                .await
            });
            Ok(ProcessSession::new(
                input_sender,
                event_receiver,
                cancellation,
                terminal,
            ))
        })
    }
}

impl SandboxPort for OsSandbox {
    fn execute(
        &self,
        request: ProcessRequest,
        events: Sender<ProcessEvent>,
        cancellation: CancellationToken,
    ) -> PortFuture<'_, ProcessOutcome> {
        Box::pin(async move {
            if self.backend == SandboxBackend::Unsupported {
                return Err(unsupported_workspace_sandbox());
            }
            let cwd = validate_request(&self.policy, &request)?;
            if cancellation.is_cancelled() {
                return Err(cancelled());
            }
            run_child(
                &self.policy,
                &self.backend,
                request,
                cwd,
                events,
                cancellation,
            )
            .await
        })
    }
}

fn validate_request(
    policy: &WorkspacePolicy,
    request: &ProcessRequest,
) -> Result<PathBuf, RuntimeError> {
    if request.working_directory.root() != policy.root() {
        return Err(invalid("sandbox working directory"));
    }
    validate_process_text(request.program.as_str())?;
    for argument in request.arguments.as_slice() {
        validate_process_text(argument.as_str())?;
    }
    for entry in request.environment.as_slice() {
        validate_process_text(entry.name().as_str())?;
        validate_process_text(entry.value().as_str())?;
    }
    let cwd = request
        .working_directory
        .value()
        .canonicalize()
        .map_err(|_| invalid("sandbox working directory"))?;
    if !cwd.is_dir() || !cwd.starts_with(policy.root()) {
        return Err(invalid("sandbox working directory"));
    }
    Ok(cwd)
}

fn validate_process_text(value: &str) -> Result<(), RuntimeError> {
    if value.contains('\0') {
        Err(invalid("sandbox process input"))
    } else {
        Ok(())
    }
}

async fn run_child(
    policy: &WorkspacePolicy,
    backend: &SandboxBackend,
    request: ProcessRequest,
    cwd: PathBuf,
    events: Sender<ProcessEvent>,
    cancellation: CancellationToken,
) -> Result<ProcessOutcome, RuntimeError> {
    let source_arguments = request.arguments.as_slice();
    let mut arguments = Vec::new();
    arguments
        .try_reserve_exact(source_arguments.len())
        .map_err(|_| limit("process arguments"))?;
    arguments.extend(
        source_arguments
            .iter()
            .map(|value| value.as_str().to_owned()),
    );
    let (outer, outer_arguments, sentinel) = outer_command(policy, backend, &request, &arguments)?;
    let mut command = Command::new(outer);
    configure_process_group(&mut command);
    command
        .args(outer_arguments)
        .current_dir(cwd)
        .env_clear()
        .env("LETTA_SANDBOX", sentinel)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for entry in request.environment.as_slice() {
        command.env(entry.name().as_str(), entry.value().as_str());
    }
    let mut child = command.spawn().map_err(|_| adapter("spawn"))?;
    let stdin = child.stdin.take().ok_or_else(|| adapter("stdin"))?;
    supervise(
        child,
        stdin,
        request.stdin,
        events,
        cancellation,
        request.timeout,
        request.output_bytes_max.get(),
    )
    .await
}

fn outer_command(
    policy: &WorkspacePolicy,
    backend: &SandboxBackend,
    request: &ProcessRequest,
    arguments: &[String],
) -> Result<(PathBuf, Vec<String>, &'static str), RuntimeError> {
    match backend {
        SandboxBackend::Seatbelt => Ok((
            PathBuf::from(SEATBELT_PROGRAM),
            seatbelt_arguments(
                policy.root(),
                policy.isolation_root(),
                request.program.as_str(),
                arguments,
            ),
            "seatbelt",
        )),
        SandboxBackend::Bubblewrap { program } => Ok((
            program.clone(),
            bubblewrap_arguments(
                policy.root(),
                policy.isolation_root(),
                request.program.as_str(),
                arguments,
            ),
            "bwrap",
        )),
        SandboxBackend::Unsupported => Err(unsupported_workspace_sandbox()),
    }
}

async fn run_interactive_child(
    policy: &WorkspacePolicy,
    backend: &SandboxBackend,
    request: ProcessRequest,
    cwd: PathBuf,
    input: Receiver<ProcessInput>,
    events: Sender<ProcessEvent>,
    cancellation: CancellationToken,
) -> Result<ProcessOutcome, RuntimeError> {
    let source_arguments = request.arguments.as_slice();
    let mut arguments = Vec::new();
    arguments
        .try_reserve_exact(source_arguments.len())
        .map_err(|_| limit("process arguments"))?;
    arguments.extend(
        source_arguments
            .iter()
            .map(|value| value.as_str().to_owned()),
    );
    let (outer, outer_arguments, sentinel) = outer_command(policy, backend, &request, &arguments)?;
    let (pty, pts) = pty_process::open().map_err(|_| adapter("pty"))?;
    pty.resize(pty_process::Size::new(24, 80))
        .map_err(|_| adapter("pty size"))?;
    let environment = request
        .environment
        .as_slice()
        .iter()
        .map(|entry| (entry.name().as_str(), entry.value().as_str()));
    // pty-process spawn performs setsid(2) and makes the child the session/process-group leader;
    // group-directed cleanup below therefore reaches every PTY descendant without unsafe hooks.
    let command = pty_process::Command::new(outer)
        .args(outer_arguments)
        .current_dir(cwd)
        .env_clear()
        .env("LETTA_SANDBOX", sentinel)
        .envs(environment)
        .kill_on_drop(true);
    let child = command.spawn(pts).map_err(|_| adapter("spawn"))?;
    let (reader, writer) = pty.into_split();
    supervise_pty(
        child,
        (reader, writer),
        input,
        events,
        cancellation,
        request.timeout,
        request.output_bytes_max.get(),
    )
    .await
}

async fn write_pty_stdin(
    mut writer: pty_process::OwnedWritePty,
    mut input: Receiver<ProcessInput>,
) -> Result<(), RuntimeError> {
    while let Some(message) = input.recv().await {
        match message {
            ProcessInput::Bytes(bytes) => writer
                .write_all(bytes.as_slice())
                .await
                .map_err(|_| adapter("stdin"))?,
            ProcessInput::Eof => break,
        }
    }
    writer.shutdown().await.map_err(|_| adapter("stdin"))
}

async fn supervise_pty(
    mut child: Child,
    pty: (pty_process::OwnedReadPty, pty_process::OwnedWritePty),
    input: Receiver<ProcessInput>,
    events: Sender<ProcessEvent>,
    cancellation: CancellationToken,
    timeout: Duration,
    output_bytes_max: usize,
) -> Result<ProcessOutcome, RuntimeError> {
    let (reader, writer) = pty;
    let (internal_sender, mut internal_receiver) = mpsc::channel(PROCESS_INTERNAL_EVENTS_MAX);
    let mut input_task = AbortOnDrop::new(tokio::spawn(write_pty_stdin(writer, input)));
    let mut reader_task =
        AbortOnDrop::new(tokio::spawn(read_stream(reader, internal_sender, true)));
    let deadline = process_deadline(timeout);
    tokio::pin!(deadline);
    let mut total = 0usize;
    let result = loop {
        tokio::select! {
            () = cancellation.cancelled() => break Err(cancelled()),
            () = &mut deadline => break Ok(ProcessOutcome { exit_code: None, timed_out: true }),
            value = internal_receiver.recv() => match value {
                Some(Ok(event)) => {
                    let forwarded =
                        forward_event(event, &events, &mut total, output_bytes_max).await;
                    if let Err(error) = forwarded {
                        break Err(error);
                    }
                }
                Some(Err(error)) => break Err(error),
                None => match child.wait().await {
                    Ok(status) => break Ok(ProcessOutcome {
                        exit_code: status.code(),
                        timed_out: false,
                    }),
                    Err(_) => break Err(adapter("wait")),
                },
            }
        }
    };
    if child.id().is_some() {
        kill_and_reap(&mut child).await;
    }
    input_task.abort();
    reader_task.abort();
    settle(&mut input_task).await;
    settle(&mut reader_task).await;
    result
}

async fn process_deadline(timeout: Duration) {
    if timeout == PROCESS_TIMEOUT_DISABLED {
        std::future::pending::<()>().await;
    } else {
        tokio::time::sleep(timeout).await;
    }
}

struct AbortOnDrop<T>(Option<tokio::task::JoinHandle<T>>);
type IoTask = AbortOnDrop<Result<(), RuntimeError>>;

impl<T> AbortOnDrop<T> {
    fn new(handle: tokio::task::JoinHandle<T>) -> Self {
        Self(Some(handle))
    }

    fn abort(&self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }

    async fn join(&mut self) -> Result<T, RuntimeError> {
        let result = self
            .0
            .as_mut()
            .ok_or_else(|| adapter("worker state"))?
            .await
            .map_err(|_| adapter("worker task"));
        self.0.take();
        result
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.abort();
    }
}

async fn write_stdin(
    mut stdin: tokio::process::ChildStdin,
    input: Option<lotta_runtime::boundary::ProcessStdin>,
) -> Result<(), RuntimeError> {
    if let Some(input) = input {
        stdin
            .write_all(input.as_slice())
            .await
            .map_err(|_| adapter("stdin"))?;
    }
    stdin.shutdown().await.map_err(|_| adapter("stdin"))
}

async fn supervise(
    mut child: Child,
    stdin: tokio::process::ChildStdin,
    input: Option<lotta_runtime::boundary::ProcessStdin>,
    events: Sender<ProcessEvent>,
    cancellation: CancellationToken,
    timeout: std::time::Duration,
    output_bytes_max: usize,
) -> Result<ProcessOutcome, RuntimeError> {
    let stdout = child.stdout.take().ok_or_else(|| adapter("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| adapter("stderr"))?;
    let (internal_sender, mut internal_receiver) = mpsc::channel(PROCESS_INTERNAL_EVENTS_MAX);
    let mut stdin_task = AbortOnDrop::new(tokio::spawn(write_stdin(stdin, input)));
    let (mut stdout_task, mut stderr_task) = spawn_readers(stdout, stderr, internal_sender);
    let deadline = process_deadline(timeout);
    tokio::pin!(deadline);
    let mut total = 0usize;
    let mut stdin_done = false;
    let result = loop {
        tokio::select! {
            () = cancellation.cancelled() => break Err(cancelled()),
            () = &mut deadline => break Ok(ProcessOutcome { exit_code: None, timed_out: true }),
            value = stdin_task.join(), if !stdin_done => {
                match value {
                    Ok(Ok(())) => stdin_done = true,
                    Ok(Err(error)) | Err(error) => break Err(error),
                }
            }
            value = internal_receiver.recv() => {
                match value {
                    Some(Ok(value)) => {
                        if let Err(error) =
                            forward_event(value, &events, &mut total, output_bytes_max).await
                        {
                            break Err(error);
                        }
                    }
                    Some(Err(error)) => break Err(error),
                    None => {
                        if let Err(error) = finish_io(
                            &mut stdin_task,
                            &mut stdout_task,
                            &mut stderr_task,
                            stdin_done,
                        ).await {
                            break Err(error);
                        }
                        match child.wait().await {
                            Ok(status) => break Ok(ProcessOutcome {
                                exit_code: status.code(),
                                timed_out: false,
                            }),
                            Err(_) => break Err(adapter("wait")),
                        }
                    }
                }
            }
        }
    };
    if child.id().is_some() {
        kill_and_reap(&mut child).await;
    }
    stdin_task.abort();
    stdout_task.abort();
    stderr_task.abort();
    settle(&mut stdin_task).await;
    settle(&mut stdout_task).await;
    settle(&mut stderr_task).await;
    result
}

fn spawn_readers(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    sender: Sender<Result<ProcessEvent, RuntimeError>>,
) -> (IoTask, IoTask) {
    let stdout = AbortOnDrop::new(tokio::spawn(read_stream(stdout, sender.clone(), true)));
    let stderr = AbortOnDrop::new(tokio::spawn(read_stream(stderr, sender, false)));
    (stdout, stderr)
}

async fn finish_io(
    stdin: &mut IoTask,
    stdout: &mut IoTask,
    stderr: &mut IoTask,
    stdin_done: bool,
) -> Result<(), RuntimeError> {
    if !stdin_done {
        stdin.join().await??;
    }
    stdout.join().await??;
    stderr.join().await??;
    Ok(())
}

async fn settle<T>(task: &mut AbortOnDrop<T>) {
    let _ignored = task.join().await;
}

async fn read_stream<R>(
    mut reader: R,
    sender: Sender<Result<ProcessEvent, RuntimeError>>,
    stdout: bool,
) -> Result<(), RuntimeError>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; PROCESS_OUTPUT_CHUNK_BYTES_MAX.value];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|_| adapter("output read"))?;
        if count == 0 {
            return Ok(());
        }
        let bytes = buffer[..count].to_vec();
        let chunk = ProcessOutputChunk::new(bytes).map_err(|_| adapter("output chunk"))?;
        let event = if stdout {
            ProcessEvent::Stdout(chunk)
        } else {
            ProcessEvent::Stderr(chunk)
        };
        sender
            .send(Ok(event))
            .await
            .map_err(|_| adapter("output channel"))?;
    }
}

async fn forward_event(
    event: ProcessEvent,
    sender: &Sender<ProcessEvent>,
    total: &mut usize,
    maximum: usize,
) -> Result<(), RuntimeError> {
    let count = match &event {
        ProcessEvent::Stdout(value) | ProcessEvent::Stderr(value) => value.as_slice().len(),
    };
    *total = total
        .checked_add(count)
        .ok_or_else(|| limit("process output"))?;
    if *total > maximum {
        return Err(limit("process output"));
    }
    sender
        .send(event)
        .await
        .map_err(|_| adapter("event receiver"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KillPhase {
    Term,
    Kill,
}

async fn kill_and_reap(child: &mut Child) {
    kill_and_reap_with(child, |_| {}).await;
}

#[cfg(test)]
pub(crate) async fn kill_and_reap_observed<F>(child: &mut Child, observer: F)
where
    F: FnMut(KillPhase),
{
    kill_and_reap_with(child, observer).await;
}

async fn kill_and_reap_with<F>(child: &mut Child, mut observer: F)
where
    F: FnMut(KillPhase),
{
    let Some(group) = child.id().and_then(|value| i32::try_from(value).ok()) else {
        let _ignored = child.wait().await;
        return;
    };
    observer(KillPhase::Term);
    if !signal_process_group(group, KillPhase::Term) {
        let _ignored = child.kill().await;
        let _ignored = child.wait().await;
        return;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(SHELL_CHILD_KILL_GRACE_MS);
    let direct_reaped = tokio::select! {
        _ = child.wait() => true,
        () = tokio::time::sleep_until(deadline) => false,
    };
    if !process_group_live(group) {
        return;
    }
    tokio::time::sleep_until(deadline).await;
    observer(KillPhase::Kill);
    force_kill_process_group(group).await;
    if !direct_reaped {
        let _ignored = child.wait().await;
    }
}

async fn force_kill_process_group(group: i32) {
    for _ in 0..PROCESS_GROUP_KILL_RETRIES_MAX {
        let _ignored = signal_process_group(group, KillPhase::Kill);
        tokio::task::yield_now().await;
        if !process_group_live(group) {
            return;
        }
    }
}

fn configure_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(unix)]
fn process_group_live(process_group: i32) -> bool {
    use rustix::io::Errno;
    use rustix::process::{Pid, test_kill_process_group};

    let Some(group) = Pid::from_raw(process_group) else {
        return false;
    };
    test_kill_process_group(group) != Err(Errno::SRCH)
}

#[cfg(not(unix))]
fn process_group_live(_: i32) -> bool {
    false
}

fn signal_process_group(process_group: i32, phase: KillPhase) -> bool {
    use rustix::process::{Pid, Signal, kill_process_group};

    let Some(group) = Pid::from_raw(process_group) else {
        return false;
    };
    let signal = match phase {
        KillPhase::Term => Signal::TERM,
        KillPhase::Kill => Signal::KILL,
    };
    kill_process_group(group, signal).is_ok()
}

fn detect_backend() -> SandboxBackend {
    #[cfg(target_os = "macos")]
    {
        if executable_regular_file(&PathBuf::from(SEATBELT_PROGRAM)) {
            return SandboxBackend::Seatbelt;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let PathSearch::Found(program) = resolve_bwrap() {
            return SandboxBackend::Bubblewrap { program };
        }
    }
    SandboxBackend::Unsupported
}

#[cfg(target_os = "linux")]
#[derive(Debug, Eq, PartialEq)]
enum PathSearch {
    Found(PathBuf),
    NotFound,
    TooMany,
}

#[cfg(target_os = "linux")]
fn search_path<I>(directories: I) -> PathSearch
where
    I: IntoIterator<Item = PathBuf>,
{
    for (index, directory) in directories.into_iter().enumerate() {
        if index == SANDBOX_PATH_ENTRIES_MAX {
            return PathSearch::TooMany;
        }
        let candidate = directory.join("bwrap");
        if executable_regular_file(&candidate)
            && let Ok(canonical) = candidate.canonicalize()
        {
            return PathSearch::Found(canonical);
        }
    }
    PathSearch::NotFound
}

#[cfg(target_os = "linux")]
fn resolve_bwrap() -> PathSearch {
    let Some(value) = std::env::var_os("PATH") else {
        return PathSearch::NotFound;
    };
    search_path(std::env::split_paths(&value))
}

#[cfg(unix)]
fn executable_regular_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}
fn limit(context: &'static str) -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: context.into(),
    }
}
fn cancelled() -> RuntimeError {
    RuntimeError::Cancelled {
        context: "workspace sandbox".into(),
    }
}
fn adapter(code: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code,
        context: "workspace sandbox".into(),
    }
}

//! Real interactive shell sessions backing the WebSocket terminal group.
//!
//! Each session owns one real child process started as a POSIX process-group
//! leader whose standard streams are OS pipes instead of a PTY. Consequences
//! of pipes over a PTY: the kernel performs no echo or line discipline,
//! `terminal_resize` is inert because no kernel window size exists, and exit
//! detection rides pipe EOF. When TTY semantics are needed, swap the launch to
//! a PTY. `TERM` and `COLORTERM` match the pinned environment.
//!
//! Output chunks stream to the owning connection through an injected sink:
//! each read emits an immediate lossily decoded chunk capped at
//! [`TERMINAL_OUTPUT_CHUNK_BYTES`]. That diverges from the pinned baseline's
//! 16 ms / 64 KiB output coalescing, though the wire shape is unchanged; add
//! matching coalescing here if outbound backpressure matters.
//!
//! Input bytes flow through a bounded channel to the child's stdin, and
//! termination reuses Task 38's two-stage discipline: TERM the whole process
//! group, wait one [`SHELL_CHILD_KILL_GRACE_MS`] grace, then KILL the group
//! and reap, so neither zombies nor orphaned descendants survive a kill or a
//! disconnect.

use std::{
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use lotta_tools::builtin::shell::SHELL_CHILD_KILL_GRACE_MS;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin},
    sync::mpsc::{self, Receiver, Sender},
};
use tokio_util::sync::CancellationToken;

/// Bounded item count for terminal stdin and output channels.
pub(super) const TERMINAL_IO_CHANNEL_ITEMS_MAX: usize = 32;
/// Largest single output chunk forwarded per emission in bytes.
const TERMINAL_OUTPUT_CHUNK_BYTES: usize = 4 * 1024;

/// Streaming sink for one decoded output chunk of one session.
pub(super) type OutputSink = Arc<dyn Fn(String) + Send + Sync>;
/// Terminal callback reporting `(pid, exit_code)` once the group is reaped.
pub(super) type ExitSink = Box<dyn FnOnce(u32, i32) + Send>;

/// Validated launch inputs for one interactive shell session.
#[derive(Clone, Copy)]
pub(super) struct LaunchRequest<'a> {
    /// Shell executable probed or pinned by the bridge.
    pub(super) shell: &'a str,
    /// Resolved working directory for the child.
    pub(super) cwd: &'a Path,
}

/// Caller-owned handles for one launched session.
pub(super) struct SpawnedSession {
    /// Child process identifier reported by `terminal_spawned`.
    pub(super) pid: u32,
    /// Bounded input, cancellation, and exit-state control.
    pub(super) control: SessionControl,
}

#[derive(Clone, Copy)]
enum GroupSignal {
    Term,
    Kill,
}

/// Bounded write side, cancellation token, and exit flag for one session.
///
/// Dropping every clone of this handle closes stdin and, with
/// `kill_on_drop`, terminates the direct child as a backstop.
pub(super) struct SessionControl {
    stdin: Sender<Vec<u8>>,
    cancellation: CancellationToken,
    exited: Arc<AtomicBool>,
}

impl SessionControl {
    /// Writes bytes into the bounded stdin channel.
    ///
    /// Returns whether the chunk was admitted; full, closed, or dead-session
    /// failures stay silent, matching the pinned swallow-on-write behavior.
    pub(super) fn write(&self, bytes: &[u8]) -> bool {
        !bytes.is_empty() && self.stdin.try_send(bytes.to_vec()).is_ok()
    }

    /// Reports whether the supervisor observed the process tree finish.
    #[must_use]
    pub(super) fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    /// Requests two-stage group termination of the session.
    pub(super) fn cancel(&self) {
        self.cancellation.cancel();
    }
}

/// Launches one real shell session wired to the supplied sinks.
///
/// # Errors
/// Returns a scrubbed failure string when the child cannot be spawned, which
/// the bridge reports as a spawn-failure `terminal_exited`.
pub(super) fn launch(
    request: LaunchRequest<'_>,
    on_output: OutputSink,
    on_exit: ExitSink,
) -> Result<SpawnedSession, String> {
    let mut command = tokio::process::Command::new(request.shell);
    command
        .current_dir(request.cwd)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    make_group_leader(&mut command);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let pid = child
        .id()
        .ok_or_else(|| "process exited immediately".to_owned())?;
    let stdin = take_pipe(&mut child, |child| child.stdin.take())?;
    let stdout = take_pipe(&mut child, |child| child.stdout.take())?;
    let stderr = take_pipe(&mut child, |child| child.stderr.take())?;
    let (input_sender, input_receiver) = mpsc::channel(TERMINAL_IO_CHANNEL_ITEMS_MAX);
    let (output_sender, output_receiver) = mpsc::channel(TERMINAL_IO_CHANNEL_ITEMS_MAX);
    tokio::spawn(pump_input(stdin, input_receiver));
    tokio::spawn(pump_output(stdout, output_sender.clone()));
    tokio::spawn(pump_output(stderr, output_sender));
    let cancellation = CancellationToken::new();
    let exited = Arc::new(AtomicBool::new(false));
    tokio::spawn(supervise(
        child,
        pid,
        cancellation.clone(),
        Arc::clone(&exited),
        output_receiver,
        on_output,
        on_exit,
    ));
    Ok(SpawnedSession {
        pid,
        control: SessionControl {
            stdin: input_sender,
            cancellation,
            exited,
        },
    })
}

async fn supervise(
    mut child: Child,
    pid: u32,
    cancellation: CancellationToken,
    exited: Arc<AtomicBool>,
    mut output: Receiver<Vec<u8>>,
    on_output: OutputSink,
    on_exit: ExitSink,
) {
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => break,
            chunk = output.recv() => match chunk {
                Some(data) => on_output(decode_chunk(&data)),
                None => break,
            },
        }
    }
    let exit_code = terminate_and_reap(&mut child).await;
    // Flag exit before the callback so reuse checks observe a dead session.
    exited.store(true, Ordering::SeqCst);
    on_exit(child.id().unwrap_or(pid), exit_code);
}

fn take_pipe<T>(
    child: &mut Child,
    take: impl FnOnce(&mut Child) -> Option<T>,
) -> Result<T, String> {
    take(child).ok_or_else(|| "child pipe unavailable".to_owned())
}

#[cfg(unix)]
fn make_group_leader(command: &mut tokio::process::Command) {
    // Group leadership lets group-directed cleanup reach every descendant.
    command.process_group(0);
}

#[cfg(not(unix))]
fn make_group_leader(_command: &mut tokio::process::Command) {}

async fn terminate_and_reap(child: &mut Child) -> i32 {
    let group = pid_of(child).and_then(|value| i32::try_from(value).ok());
    if let Some(group) = group {
        signal_group(group, GroupSignal::Term);
    }
    let graceful = tokio::time::timeout(kill_grace(), child.wait())
        .await
        .ok()
        .and_then(Result::ok);
    if graceful.is_none() {
        if let Some(group) = group {
            signal_group(group, GroupSignal::Kill);
        }
        let _ignored = child.wait().await;
    }
    graceful.and_then(|status| status.code()).unwrap_or(0)
}

fn kill_grace() -> Duration {
    Duration::from_millis(SHELL_CHILD_KILL_GRACE_MS)
}

fn pid_of(child: &Child) -> Option<u32> {
    child.id()
}

#[cfg(unix)]
fn signal_group(group: i32, signal: GroupSignal) {
    use rustix::process::{Pid, Signal, kill_process_group};
    let mapped = match signal {
        GroupSignal::Term => Signal::TERM,
        GroupSignal::Kill => Signal::KILL,
    };
    if let Some(pid) = Pid::from_raw(group) {
        let _ignored = kill_process_group(pid, mapped);
    }
}

#[cfg(not(unix))]
fn signal_group(_group: i32, _signal: GroupSignal) {}

async fn pump_input(mut stdin: ChildStdin, mut input: Receiver<Vec<u8>>) {
    while let Some(bytes) = input.recv().await {
        if stdin.write_all(&bytes).await.is_err() {
            break;
        }
    }
    let _ignored = stdin.shutdown().await;
}

async fn pump_output<R>(mut reader: R, output: Sender<Vec<u8>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; TERMINAL_OUTPUT_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                if output.send(buffer[..count].to_vec()).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn decode_chunk(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

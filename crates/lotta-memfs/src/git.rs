use crate::fs;
use lotta_runtime::RuntimeError;
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const GIT_OUTPUT_BYTES_MAX: usize = 16 * 1024 * 1024;
const GIT_COMMAND_DEADLINE_MS: u64 = 30_000;
const GIT_POLL_INTERVAL_MS: u64 = 10;

pub(crate) struct GitOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
}

type ReaderHandle = JoinHandle<Result<Vec<u8>, RuntimeError>>;

fn failure(code: &'static str, context: &'static str) -> RuntimeError {
    RuntimeError::AdapterFailure {
        code,
        context: context.into(),
    }
}

fn output_limit() -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: "GIT_OUTPUT_BYTES_MAX".into(),
    }
}

fn read_capped<R: Read>(
    mut reader: R,
    overflow: &AtomicBool,
    retain: bool,
) -> Result<Vec<u8>, RuntimeError> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut total = 0_usize;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| failure("git_pipe_read", "git output"))?;
        if read == 0 {
            return Ok(output);
        }
        total = total.checked_add(read).ok_or_else(output_limit)?;
        if total > GIT_OUTPUT_BYTES_MAX {
            overflow.store(true, Ordering::Release);
            return Err(output_limit());
        }
        if retain {
            output.try_reserve(read).map_err(|_| output_limit())?;
            output.extend_from_slice(&buffer[..read]);
        }
    }
}

fn reader_thread<R: Read + Send + 'static>(
    reader: R,
    overflow: Arc<AtomicBool>,
    retain: bool,
) -> Result<ReaderHandle, RuntimeError> {
    thread::Builder::new()
        .name("lotta-git-output".into())
        .spawn(move || read_capped(reader, &overflow, retain))
        .map_err(|_| failure("git_pipe_thread", "git output"))
}

fn kill_and_reap(child: &mut Child) -> Result<(), RuntimeError> {
    child
        .kill()
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|_| failure("git_kill", "git command"))?;
    child
        .wait()
        .map_err(|_| failure("git_wait", "git command"))?;
    Ok(())
}

fn await_child(
    child: &mut Child,
    overflow: &AtomicBool,
    deadline: Instant,
) -> Result<ExitStatus, RuntimeError> {
    loop {
        if overflow.load(Ordering::Acquire) {
            kill_and_reap(child)?;
            return Err(output_limit());
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(GIT_POLL_INTERVAL_MS));
            }
            Ok(None) => {
                kill_and_reap(child)?;
                return Err(RuntimeError::Timeout {
                    context: "git command".into(),
                });
            }
            Err(_) => {
                kill_and_reap(child)?;
                return Err(failure("git_wait", "git command"));
            }
        }
    }
}

fn join_reader(handle: ReaderHandle) -> Result<Vec<u8>, RuntimeError> {
    handle
        .join()
        .map_err(|_| failure("git_pipe_join", "git output"))?
}

fn spawn_readers(
    child: &mut Child,
    overflow: &Arc<AtomicBool>,
) -> Result<(ReaderHandle, ReaderHandle), RuntimeError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| failure("git_pipe", "git stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| failure("git_pipe", "git stderr"))?;
    let stdout_handle = reader_thread(stdout, Arc::clone(overflow), true)?;
    match reader_thread(stderr, Arc::clone(overflow), false) {
        Ok(stderr_handle) => Ok((stdout_handle, stderr_handle)),
        Err(error) => {
            drop(kill_and_reap(child));
            drop(join_reader(stdout_handle));
            Err(error)
        }
    }
}

fn execute(mut child: Child) -> Result<GitOutput, RuntimeError> {
    let overflow = Arc::new(AtomicBool::new(false));
    let (stdout_reader, stderr_reader) = spawn_readers(&mut child, &overflow)?;
    let deadline = Instant::now() + Duration::from_millis(GIT_COMMAND_DEADLINE_MS);
    let status = await_child(&mut child, &overflow, deadline);
    let stdout = join_reader(stdout_reader);
    let stderr = join_reader(stderr_reader);
    let status = status?;
    let stdout = stdout?;
    stderr?;
    if overflow.load(Ordering::Acquire) {
        return Err(output_limit());
    }
    Ok(GitOutput { status, stdout })
}

pub(crate) fn run<I, S>(
    repo: &Path,
    args: I,
    context: &'static str,
) -> Result<GitOutput, RuntimeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    fs::validate_root(repo)?;
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "");
    let child = command
        .spawn()
        .map_err(|_| failure("git_spawn", "git executable"))?;
    execute(child).map_err(|error| match error {
        RuntimeError::AdapterFailure { code, .. } => RuntimeError::AdapterFailure {
            code,
            context: context.into(),
        },
        other => other,
    })
}

pub(crate) fn checked<I, S>(
    repo: &Path,
    args: I,
    context: &'static str,
) -> Result<Vec<u8>, RuntimeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = run(repo, args, context)?;
    if !output.status.success() {
        return Err(failure("git_failed", context));
    }
    Ok(output.stdout)
}

pub(crate) fn identity_args() -> [&'static str; 8] {
    [
        "-c",
        "user.name=Letta Agent",
        "-c",
        "user.email=lotta-memfs@localhost",
        "-c",
        "commit.gpgSign=false",
        "-c",
        "core.hooksPath=/dev/null",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn capped_reader_below_at_and_above() {
        for size in [GIT_OUTPUT_BYTES_MAX - 1, GIT_OUTPUT_BYTES_MAX] {
            let overflow = AtomicBool::new(false);
            let bytes = read_capped(Cursor::new(vec![0_u8; size]), &overflow, true)
                .expect("bounded reader");
            assert_eq!(bytes.len(), size);
            assert!(!overflow.load(Ordering::Acquire));
        }
        let overflow = AtomicBool::new(false);
        let result = read_capped(
            Cursor::new(vec![0_u8; GIT_OUTPUT_BYTES_MAX + 1]),
            &overflow,
            true,
        );
        assert!(matches!(result, Err(RuntimeError::LimitExceeded { .. })));
        assert!(overflow.load(Ordering::Acquire));
    }

    #[test]
    fn real_git_command_smoke() {
        let repo = std::env::current_dir().expect("current directory");
        let output = checked(&repo, ["--version"], "git version").expect("git command");
        assert!(output.starts_with(b"git version "));
    }
}

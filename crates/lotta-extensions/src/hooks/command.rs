//! Sandboxed command-hook execution over the canonical process capability.

use super::events::{HookOutcome, HookPayload, HookReason};
use lotta_domain::RuntimeScope;
use lotta_runtime::{
    RuntimeError,
    boundary::{
        ConfinedPath, ProcessArguments, ProcessEnvironment, ProcessOutputBytesMax, ProcessStdin,
        Program,
    },
    ports::{ProcessEvent, ProcessRequest, SandboxPort},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{fmt, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Fixed server-owned command hook timeout.
pub const COMMAND_HOOK_TIMEOUT_MS_DEFAULT: u64 = 60_000;
/// Aggregate command hook output ceiling.
pub const COMMAND_HOOK_OUTPUT_BYTES_MAX: usize = 1_048_576;
const COMMAND_HOOK_EVENTS_MAX: usize = 16;

/// Pinned command hook configuration.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandHookConfig {
    /// Exact pinned hook type.
    #[serde(rename = "type")]
    pub kind: CommandHookType,
    /// Program path/name executed directly without shell interpolation.
    pub command: String,
    /// Caller value retained for compatibility but cannot extend the server deadline.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Suppresses presentation output in higher layers.
    #[serde(default)]
    pub quiet: bool,
}
impl fmt::Debug for CommandHookConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandHookConfig")
            .field("kind", &self.kind)
            .field("command", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .field("quiet", &self.quiet)
            .finish()
    }
}

/// Exact command hook discriminant.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CommandHookType {
    /// Direct command execution.
    #[serde(rename = "command")]
    Command,
}

/// Parsed production command-hook result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandHookOutcome {
    /// Baseline process exit code.
    #[serde(rename = "exitCode")]
    pub exit_code: i32,
    /// Trimmed standard output.
    pub stdout: String,
    /// Trimmed standard error.
    pub stderr: String,
    /// Whether the fixed deadline elapsed.
    #[serde(rename = "timedOut")]
    pub timed_out: bool,
    /// Duration in milliseconds, clamped to the fixed deadline.
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    /// Stable optional error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Concrete command executor.
pub struct CommandHookExecutor {
    sandbox: Arc<dyn SandboxPort>,
    scope: RuntimeScope,
    root: PathBuf,
}
impl CommandHookExecutor {
    /// Constructs an owner-scoped executor rooted at a canonical sandbox directory.
    pub fn new(
        sandbox: Arc<dyn SandboxPort>,
        scope: RuntimeScope,
        root: PathBuf,
    ) -> Result<Self, CommandHookError> {
        let root = root.canonicalize().map_err(|_| CommandHookError::Sandbox)?;
        if !root.is_dir() {
            return Err(CommandHookError::Sandbox);
        }
        Ok(Self {
            sandbox,
            scope,
            root,
        })
    }
    /// Executes one hook with bounded input, output, timeout, cancellation, and child join.
    pub async fn execute(
        &self,
        config: &CommandHookConfig,
        payload: &HookPayload,
        cancellation: CancellationToken,
    ) -> Result<HookOutcome, CommandHookError> {
        let stdin = serde_json::to_vec(payload.value()).map_err(|_| CommandHookError::Payload)?;
        let timeout_ms = config
            .timeout
            .unwrap_or(COMMAND_HOOK_TIMEOUT_MS_DEFAULT)
            .min(COMMAND_HOOK_TIMEOUT_MS_DEFAULT);
        let request = self.request(config, stdin, timeout_ms)?;
        let (sender, mut receiver) = mpsc::channel(COMMAND_HOOK_EVENTS_MAX);
        let run = self.sandbox.execute(request, sender, cancellation);
        tokio::pin!(run);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = loop {
            tokio::select! {
                result = &mut run => break result.map_err(map_runtime)?,
                event = receiver.recv() => match event {
                    Some(ProcessEvent::Stdout(chunk)) => append(&mut stdout, chunk.as_slice())?,
                    Some(ProcessEvent::Stderr(chunk)) => append(&mut stderr, chunk.as_slice())?,
                    None => break run.await.map_err(map_runtime)?,
                }
            }
        };
        while let Ok(event) = receiver.try_recv() {
            match event {
                ProcessEvent::Stdout(chunk) => append(&mut stdout, chunk.as_slice())?,
                ProcessEvent::Stderr(chunk) => append(&mut stderr, chunk.as_slice())?,
            }
        }
        parse_process_outcome(
            payload,
            outcome.exit_code,
            outcome.timed_out,
            stdout,
            stderr,
        )
    }
    fn request(
        &self,
        config: &CommandHookConfig,
        stdin: Vec<u8>,
        timeout_ms: u64,
    ) -> Result<ProcessRequest, CommandHookError> {
        let program = Program::new(config.command.clone()).map_err(|_| CommandHookError::Config)?;
        ProcessRequest::new(
            self.scope.clone(),
            program,
            ProcessArguments::new(Vec::new()).map_err(|_| CommandHookError::Config)?,
            ConfinedPath::new(self.root.clone(), self.root.clone())
                .map_err(|_| CommandHookError::Sandbox)?,
            ProcessEnvironment::new(Vec::new()).map_err(|_| CommandHookError::Config)?,
            Some(ProcessStdin::new(stdin).map_err(|_| CommandHookError::Payload)?),
            ProcessOutputBytesMax::new(COMMAND_HOOK_OUTPUT_BYTES_MAX)
                .map_err(|_| CommandHookError::Output)?,
            Duration::from_millis(timeout_ms),
        )
        .map_err(|_| CommandHookError::Config)
    }
}

fn append(target: &mut Vec<u8>, bytes: &[u8]) -> Result<(), CommandHookError> {
    if target.len().saturating_add(bytes.len()) > COMMAND_HOOK_OUTPUT_BYTES_MAX {
        return Err(CommandHookError::Output);
    }
    target.extend_from_slice(bytes);
    Ok(())
}
fn parse_process_outcome(
    payload: &HookPayload,
    exit: Option<i32>,
    timed_out: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
) -> Result<HookOutcome, CommandHookError> {
    let stdout = String::from_utf8(stdout).map_err(|_| CommandHookError::Output)?;
    let stderr = String::from_utf8(stderr).map_err(|_| CommandHookError::Output)?;
    if timed_out {
        return Err(CommandHookError::Timeout);
    }
    match exit {
        Some(0) => parse_allow(payload, stdout.trim()),
        Some(2) => Ok(HookOutcome::Block(
            HookReason::new(stderr.trim().to_owned()).map_err(|_| CommandHookError::Output)?,
        )),
        _ => Err(CommandHookError::Exit),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlainCommandOutput {
    ok: bool,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HookSpecificCommandOutput<T> {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: T,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreToolSpecificOutput {
    #[serde(rename = "updatedInput")]
    updated_input: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostToolSpecificOutput {
    #[serde(rename = "updatedResult")]
    updated_result: serde_json::Value,
}

fn parse_allow(input: &HookPayload, stdout: &str) -> Result<HookOutcome, CommandHookError> {
    if stdout.is_empty() {
        return Ok(HookOutcome::Allow);
    }
    match input.event() {
        super::events::HookEvent::PreToolUse => {
            parse_modify::<PreToolSpecificOutput>(input, stdout, "tool_input", |output| {
                output.updated_input
            })
        }
        super::events::HookEvent::PostToolUse => {
            parse_modify::<PostToolSpecificOutput>(input, stdout, "tool_result", |output| {
                output.updated_result
            })
        }
        _ => parse_plain(stdout),
    }
}

fn parse_plain(stdout: &str) -> Result<HookOutcome, CommandHookError> {
    let output: PlainCommandOutput =
        serde_json::from_str(stdout).map_err(|_| CommandHookError::Output)?;
    match (output.ok, output.reason) {
        (true, None) => Ok(HookOutcome::Allow),
        (false, Some(reason)) => Ok(HookOutcome::Block(
            HookReason::new(reason).map_err(|_| CommandHookError::Output)?,
        )),
        _ => Err(CommandHookError::Output),
    }
}

fn parse_modify<T: DeserializeOwned>(
    input: &HookPayload,
    stdout: &str,
    field: &str,
    updated: impl FnOnce(T) -> serde_json::Value,
) -> Result<HookOutcome, CommandHookError> {
    if let Ok(output) = serde_json::from_str::<HookSpecificCommandOutput<T>>(stdout) {
        let mut payload = input.value().clone();
        payload
            .as_object_mut()
            .ok_or(CommandHookError::Output)?
            .insert(field.into(), updated(output.hook_specific_output));
        return Ok(HookOutcome::Modify(
            HookPayload::new(input.event(), payload).map_err(|_| CommandHookError::Output)?,
        ));
    }
    parse_plain(stdout)
}
fn map_runtime(error: RuntimeError) -> CommandHookError {
    match error {
        RuntimeError::Cancelled { .. } => CommandHookError::Cancelled,
        RuntimeError::Timeout { .. } => CommandHookError::Timeout,
        RuntimeError::PermissionDenied { .. } | RuntimeError::InvalidData { .. } => {
            CommandHookError::Sandbox
        }
        _ => CommandHookError::Process,
    }
}

/// Stable command hook failure without command, payload, environment, or output text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CommandHookError {
    /// Invalid config.
    #[error("invalid command hook configuration")]
    Config,
    /// Invalid or overbound payload.
    #[error("invalid command hook payload")]
    Payload,
    /// Sandbox confinement failed.
    #[error("command hook sandbox denied")]
    Sandbox,
    /// Process adapter failed.
    #[error("command hook process failed")]
    Process,
    /// Process returned an error exit code.
    #[error("command hook failed")]
    Exit,
    /// Fixed server deadline elapsed.
    #[error("command hook timeout")]
    Timeout,
    /// Explicit cancellation won.
    #[error("command hook cancelled")]
    Cancelled,
    /// Output was malformed or overbound.
    #[error("command hook output invalid")]
    Output,
}

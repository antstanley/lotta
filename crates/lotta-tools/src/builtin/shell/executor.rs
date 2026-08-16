use super::{
    LOCAL_TOOL_EXECUTION_TIMEOUT_MS_MAX,
    manager::{LaunchOptions, ManagerError, ProcessManager, SessionStatus},
};
use crate::pipeline::{
    ExecutorError, ExecutorFuture, RawToolExecutionRequest, RawToolOutcome, ToolExecutor,
};
use lotta_runtime::ports::{ToolOutcome, ToolOutcomeCode, ToolOutcomeMessage};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};

pub(super) struct ShellExecutor {
    manager: Arc<ProcessManager>,
}
impl ShellExecutor {
    pub(super) const fn new(manager: Arc<ProcessManager>) -> Self {
        Self { manager }
    }
}

impl ToolExecutor for ShellExecutor {
    fn execute(&self, request: RawToolExecutionRequest) -> ExecutorFuture<'_> {
        let manager = Arc::clone(&self.manager);
        Box::pin(async move {
            let name = request.definition.internal_name.as_str();
            let input = request.input.as_value();
            let timeout = timeout(name, input, request.deadline.get())?;
            let result = match name {
                "Bash" => bash(&manager, input, timeout, request.cancellation, "bash").await,
                "run_shell_command" => {
                    bash(&manager, input, timeout, request.cancellation, "gemini").await
                }
                "exec_command" => exec(&manager, input, timeout, request.cancellation).await,
                "TaskOutput" => task_output(&manager, input).await,
                "TaskStop" => task_stop(&manager, input).await,
                "Monitor" => monitor(&manager, input, timeout).await,
                "write_stdin" => write_stdin(&manager, input).await,
                _ => Err(ManagerError::Invalid),
            };
            map_result(result)
        })
    }
}

type ToolResult = Result<String, ManagerError>;

async fn bash(
    manager: &ProcessManager,
    input: &Value,
    timeout: Duration,
    cancellation: tokio_util::sync::CancellationToken,
    prefix: &str,
) -> ToolResult {
    let command = required(input, "command")?;
    if input
        .get("run_in_background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let id = manager
            .start(command, &workdir(input, "dir_path"), timeout, prefix, false)
            .await?;
        return Ok(format!("Command running in background with ID: {id}"));
    }
    let result = manager
        .one_shot(command, &workdir(input, "dir_path"), timeout, cancellation)
        .await?;
    let output = if result.output.is_empty() {
        "(Command completed with no output)".to_owned()
    } else {
        result.output
    };
    if result.exit_code.is_some_and(|code| code != 0) {
        Ok(format!(
            "Exit code: {}\n{output}",
            result.exit_code.unwrap_or_default()
        ))
    } else {
        Ok(output)
    }
}

async fn exec(
    manager: &ProcessManager,
    input: &Value,
    timeout: Duration,
    cancellation: tokio_util::sync::CancellationToken,
) -> ToolResult {
    let command = required(input, "cmd")?;
    let tty = input.get("tty").and_then(Value::as_bool).unwrap_or(false);
    let shell = input
        .get("shell")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let login = input.get("login").and_then(Value::as_bool).unwrap_or(true);
    let id = manager
        .start_launcher(
            command,
            &workdir(input, "workdir"),
            timeout,
            "exec",
            LaunchOptions {
                interactive: tty,
                shell,
                login,
            },
        )
        .await?;
    let yield_ms = clamp_u64(input.get("yield_time_ms"), 10_000, 250, 30_000)?;
    tokio::select! {
        result = manager.wait_output(&id, Duration::from_millis(yield_ms)) => {
            let _ = result?;
        }
        () = cancellation.cancelled() => {
            let _ = manager.stop(&id).await?;
            manager.release(&id)?;
            return Err(ManagerError::Interrupted);
        }
    }
    format_exec_read(
        manager,
        &id,
        max_output_tokens(input),
        Duration::from_millis(yield_ms),
    )
}

async fn monitor(manager: &ProcessManager, input: &Value, timeout: Duration) -> ToolResult {
    let command = input
        .get("command")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let ws = input.get("ws").filter(|value| !value.is_null());
    if usize::from(command.is_some()) + usize::from(ws.is_some()) != 1 {
        return Err(ManagerError::Invalid);
    }
    if ws.is_some() {
        return Ok("Unsupported: WebSocket Monitor is outside Task38 network scope.".to_owned());
    }
    let command = command.ok_or(ManagerError::Invalid)?;
    if command.chars().any(hidden_control) {
        return Err(ManagerError::Invalid);
    }
    let persistent = input
        .get("persistent")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let process_timeout = if persistent {
        lotta_runtime::ports::PROCESS_TIMEOUT_DISABLED
    } else {
        timeout
    };
    let id = manager
        .start(
            command,
            &workdir(input, "workdir"),
            process_timeout,
            "monitor",
            false,
        )
        .await?;
    let lifetime = if persistent {
        "persistent — runs until TaskStop or session end".to_owned()
    } else {
        format!("timeout {}ms", timeout.as_millis())
    };
    Ok(format!(
        "Monitor started (task {id}, {lifetime}). You will be notified on each event. \
         Keep working — do not poll or sleep. Events may arrive while you are waiting for the \
         user — an event is not their reply."
    ))
}

fn hidden_control(value: char) -> bool {
    let code = u32::from(value);
    value != '\t' && value != '\n' && (code < 32 || (127..=159).contains(&code))
}

async fn task_output(manager: &ProcessManager, input: &Value) -> ToolResult {
    let id = required(input, "task_id")?;
    let block = input.get("block").and_then(Value::as_bool).unwrap_or(false);
    let timeout_ms = integer(input.get("timeout"), 0, 600_000)?;
    let result = if block {
        manager
            .wait_output(id, Duration::from_millis(timeout_ms))
            .await
    } else {
        manager.output(id)
    };
    let (output, status) = match result {
        Ok(value) => value,
        Err(ManagerError::Unknown) => {
            return Ok(format!("No background process found with ID: {id}"));
        }
        Err(error) => return Err(error),
    };
    if status == SessionStatus::Running && !block {
        return task_output_json("Task is still running...", status, None, None);
    }
    let message = if output.is_empty() {
        "(no output yet)"
    } else {
        &output
    };
    let read = manager.read_exec(id)?;
    task_output_json(message, status, read.exit_code, read.terminal)
}

fn task_output_json(
    message: &str,
    status: SessionStatus,
    exit_code: Option<i32>,
    terminal: Option<ManagerError>,
) -> ToolResult {
    let status = match status {
        SessionStatus::Running => "running",
        SessionStatus::Completed => "completed",
        SessionStatus::Failed => "failed",
        SessionStatus::Stopped => "stopped",
    };
    serde_json::to_string(&json!({
        "message": message,
        "status": status,
        "exit_code": exit_code,
        "terminal_reason": terminal.map(terminal_reason),
    }))
    .map_err(|_| ManagerError::Infrastructure)
}

fn terminal_reason(error: ManagerError) -> &'static str {
    match error {
        ManagerError::Timeout => "timeout",
        ManagerError::Interrupted => "stopped",
        ManagerError::Spawn => "spawn_failure",
        ManagerError::Limit => "limit_exceeded",
        _ => "failed",
    }
}
async fn task_stop(manager: &ProcessManager, input: &Value) -> ToolResult {
    Ok(format!(
        "{{\"killed\":{}}}",
        manager.stop(required(input, "task_id")?).await?
    ))
}
async fn write_stdin(manager: &ProcessManager, input: &Value) -> ToolResult {
    let id = input
        .get("session_id")
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_u64().map(|number| number.to_string()))
        })
        .ok_or(ManagerError::Invalid)?;
    let chars = input.get("chars").and_then(Value::as_str).unwrap_or("");
    if !chars.is_empty() {
        manager.write(&id, chars.as_bytes(), false).await?;
    }
    let (default, minimum, maximum) = if chars.is_empty() {
        (5_000, 5_000, 300_000)
    } else {
        (250, 250, 30_000)
    };
    let wait = clamp_u64(input.get("yield_time_ms"), default, minimum, maximum)?;
    let _ = manager
        .wait_output(&id, Duration::from_millis(wait))
        .await?;
    format_exec_read(
        manager,
        &id,
        max_output_tokens(input),
        Duration::from_millis(wait),
    )
}

fn format_exec_read(
    manager: &ProcessManager,
    id: &str,
    max_tokens: usize,
    wall_time: Duration,
) -> ToolResult {
    let read = manager.read_exec(id)?;
    let original_tokens = read.output.len().div_ceil(4);
    let output = truncate_tokens(&read.output, max_tokens);
    let chunk = chunk_id(id, read.wall_time.as_nanos());
    let state = if read.status == SessionStatus::Running {
        format!("Process running with session ID {id}")
    } else {
        format!(
            "Process exited with code {}",
            read.exit_code.unwrap_or_default()
        )
    };
    let formatted = format!(
        concat!(
            "Chunk ID: {chunk}\nWall time: {:.4} seconds\n{state}\n",
            "Original token count: {original_tokens}\nOutput:\n{output}"
        ),
        wall_time.as_secs_f64(),
        chunk = chunk,
        state = state,
        original_tokens = original_tokens,
        output = output,
    );
    if read.status != SessionStatus::Running {
        manager.release(id)?;
    }
    Ok(formatted)
}

fn chunk_id(id: &str, elapsed: u128) -> String {
    let elapsed = u64::try_from(elapsed).unwrap_or(u64::MAX);
    let mut value = elapsed ^ 0xcbf2_9ce4_8422_2325;
    for byte in id.bytes() {
        value ^= u64::from(byte);
        value = value.wrapping_mul(0x100_0000_01b3);
    }
    format!("{:06x}", value & 0xff_ffff)
}

fn max_output_tokens(input: &Value) -> usize {
    let value = input
        .get("max_output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(10_000);
    if value <= 0 {
        return 10_000;
    }
    usize::try_from(value).unwrap_or(250_000).min(250_000)
}

fn truncate_tokens(value: &str, maximum: usize) -> String {
    const MAX_INLINE_OUTPUT_CHARS: usize = 30_000;
    let chars = maximum.saturating_mul(4).clamp(1, MAX_INLINE_OUTPUT_CHARS);
    truncate_middle(value, chars)
}

fn truncate_middle(value: &str, chars: usize) -> String {
    let total = value.chars().count();
    if total <= chars {
        return value.to_owned();
    }
    let marker = format!(
        "
... [{} characters omitted] ...
",
        total - chars
    );
    let head = chars / 2;
    let tail = chars - head;
    let mut output = String::with_capacity(value.len().min(chars * 4) + marker.len());
    output.extend(value.chars().take(head));
    output.push_str(&marker);
    output.extend(value.chars().skip(total - tail));
    output
}

fn integer(value: Option<&Value>, minimum: u64, maximum: u64) -> Result<u64, ManagerError> {
    match value {
        Some(Value::Number(number)) => number
            .as_u64()
            .filter(|v| *v >= minimum && *v <= maximum)
            .ok_or(ManagerError::Invalid),
        None => Ok(minimum),
        _ => Err(ManagerError::Invalid),
    }
}

fn clamp_u64(
    value: Option<&Value>,
    default: u64,
    minimum: u64,
    maximum: u64,
) -> Result<u64, ManagerError> {
    let value = match value {
        None => default,
        Some(value) => value.as_u64().ok_or(ManagerError::Invalid)?,
    };
    Ok(value.clamp(minimum, maximum))
}

fn timeout(name: &str, input: &Value, definition: Duration) -> Result<Duration, ExecutorError> {
    let (field, default, minimum, maximum) = match name {
        "Bash" => ("timeout", 180_000, 1, 600_000),
        "Monitor" => ("timeout_ms", 300_000, 1_000, 3_600_000),
        "TaskOutput" | "TaskStop" | "write_stdin" => {
            return Ok(definition);
        }
        _ => ("timeout", 180_000, 1, LOCAL_TOOL_EXECUTION_TIMEOUT_MS_MAX),
    };
    if name == "Monitor" && input.get("persistent").and_then(Value::as_bool) == Some(true) {
        return Ok(definition);
    }
    let value = match input.get(field) {
        None => default,
        Some(value) => value
            .as_u64()
            .filter(|value| *value >= minimum && *value <= maximum)
            .ok_or(ExecutorError)?,
    };
    Ok(Duration::from_millis(value).min(definition))
}
fn workdir(input: &Value, field: &str) -> PathBuf {
    input
        .get(field)
        .and_then(Value::as_str)
        .map_or_else(PathBuf::new, PathBuf::from)
}
fn required<'a>(input: &'a Value, field: &str) -> Result<&'a str, ManagerError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(ManagerError::Invalid)
}
fn map_result(result: ToolResult) -> Result<RawToolOutcome, ExecutorError> {
    match result {
        Ok(value) => Ok(RawToolOutcome::Success(value)),
        Err(ManagerError::Timeout) => Ok(RawToolOutcome::Failure(ToolOutcome::Timeout {
            message: message("Shell command timed out.")?,
        })),
        Err(ManagerError::Interrupted) => Ok(RawToolOutcome::Failure(ToolOutcome::Interrupted {
            message: message("Shell command interrupted.")?,
        })),
        Err(ManagerError::Spawn) => Ok(RawToolOutcome::Failure(ToolOutcome::SpawnFailure {
            message: message("Shell command could not be spawned.")?,
        })),
        Err(error) => {
            let code = ToolOutcomeCode::new(code(error).to_owned()).map_err(|_| ExecutorError)?;
            let message = message("Shell tool failed.")?;
            Ok(RawToolOutcome::Failure(ToolOutcome::ToolDefinedError {
                code,
                message,
            }))
        }
    }
}
fn code(error: ManagerError) -> &'static str {
    match error {
        ManagerError::Invalid => "invalid_input",
        ManagerError::Limit => "limit_exceeded",
        ManagerError::Unknown => "unknown_session",
        ManagerError::Closed => "stdin_closed",
        _ => "shell_error",
    }
}
fn message(value: &str) -> Result<ToolOutcomeMessage, ExecutorError> {
    ToolOutcomeMessage::new(value.to_owned()).map_err(|_| ExecutorError)
}

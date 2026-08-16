use super::{ShellBundleError, manager::ProcessManager};
use crate::{pipeline::ToolExecutor, registry::ToolRegistration};
use lotta_domain::BoundedJsonValue;
use lotta_runtime::{
    bounds::{TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX},
    ports::{
        InternalToolName, ModelFacingToolName, PermissionAction, SecretRedactionSpec,
        ToolApprovalPolicy, ToolDefinition, ToolDescriptionAsset, ToolExecutionOwner,
        ToolInputSchema, ToolOutputLimit, ToolTimeout,
    },
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const TOOLS: &[&str] = &[
    "Bash",
    "Monitor",
    "TaskOutput",
    "TaskStop",
    "exec_command",
    "write_stdin",
    "run_shell_command",
];

pub(super) fn registrations(
    manager: &Arc<ProcessManager>,
) -> Result<Vec<ToolRegistration>, ShellBundleError> {
    let executor: Arc<dyn ToolExecutor> =
        Arc::new(super::executor::ShellExecutor::new(Arc::clone(manager)));
    let mut output = Vec::new();
    output
        .try_reserve_exact(TOOLS.len())
        .map_err(|_| ShellBundleError)?;
    for &name in TOOLS {
        output.push(ToolRegistration {
            definition: Arc::new(definition(name)?),
            executor: Arc::clone(&executor),
        });
    }
    Ok(output)
}

fn definition(name: &str) -> Result<ToolDefinition, ShellBundleError> {
    Ok(ToolDefinition::new(
        InternalToolName::new(name.to_owned()).map_err(|_| ShellBundleError)?,
        ModelFacingToolName::new(name.to_owned()).map_err(|_| ShellBundleError)?,
        schema(name)?,
        ToolDescriptionAsset::new(description(name).trim().to_owned())
            .map_err(|_| ShellBundleError)?,
        ToolExecutionOwner::Rust,
        approval(name),
        PermissionAction::new("execute".to_owned()).map_err(|_| ShellBundleError)?,
        ToolTimeout::new_shell(Duration::from_millis(
            super::LOCAL_TOOL_EXECUTION_TIMEOUT_MS_MAX,
        ))
        .map_err(|_| ShellBundleError)?,
        ToolOutputLimit::new(
            TOOL_RESULT_BYTES_MAX.value,
            TOOL_RESULT_MODEL_CHARS_MAX.value,
        )
        .map_err(|_| ShellBundleError)?,
        serde_json::from_value::<SecretRedactionSpec>(json!({"fields":[],"policy":"redact"}))
            .map_err(|_| ShellBundleError)?,
    ))
}

fn approval(name: &str) -> ToolApprovalPolicy {
    match name {
        "Bash" | "Monitor" | "TaskStop" | "exec_command" | "run_shell_command" => {
            ToolApprovalPolicy::Always
        }
        _ => ToolApprovalPolicy::Never,
    }
}

fn schema(name: &str) -> Result<ToolInputSchema, ShellBundleError> {
    let value: Value = serde_json::from_str(schema_asset(name)?).map_err(|_| ShellBundleError)?;
    ToolInputSchema::new(BoundedJsonValue::new(value).map_err(|_| ShellBundleError)?)
        .map_err(|_| ShellBundleError)
}

pub(super) fn schema_asset(name: &str) -> Result<&'static str, ShellBundleError> {
    match name {
        "Bash" => Ok(include_str!("assets/schemas/Bash.json")),
        "Monitor" => Ok(include_str!("assets/schemas/Monitor.json")),
        "TaskOutput" => Ok(include_str!("assets/schemas/TaskOutput.json")),
        "TaskStop" => Ok(include_str!("assets/schemas/TaskStop.json")),
        "exec_command" => Ok(include_str!("assets/schemas/ExecCommand.json")),
        "write_stdin" => Ok(include_str!("assets/schemas/WriteStdin.json")),
        "run_shell_command" => Ok(include_str!("assets/schemas/RunShellCommandGemini.json")),
        _ => Err(ShellBundleError),
    }
}

pub(super) fn description(name: &str) -> &'static str {
    match name {
        "Bash" => include_str!("assets/descriptions/Bash.md"),
        "Monitor" => include_str!("assets/descriptions/Monitor.md"),
        "TaskOutput" => include_str!("assets/descriptions/TaskOutput.md"),
        "TaskStop" => include_str!("assets/descriptions/TaskStop.md"),
        "exec_command" => include_str!("assets/descriptions/ExecCommand.md"),
        "write_stdin" => include_str!("assets/descriptions/WriteStdin.md"),
        "run_shell_command" => include_str!("assets/descriptions/RunShellCommandGemini.md"),
        _ => "",
    }
}

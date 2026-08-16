use super::{WorktreeBundleError, WorktreeManager, executor::WorktreeExecutor};
use crate::registry::ToolRegistration;
use lotta_domain::BoundedJsonValue;
use lotta_runtime::{
    bounds::{EXTERNAL_TOOL_CALL_TIMEOUT_MS, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX},
    ports::{
        InternalToolName, ModelFacingToolName, PermissionAction, SecretRedactionSpec,
        ToolApprovalPolicy, ToolDefinition, ToolDescriptionAsset, ToolExecutionOwner,
        ToolInputSchema, ToolOutputLimit, ToolTimeout,
    },
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const TOOLS: [&str; 2] = ["EnterWorktree", "ExitWorktree"];

pub(super) fn registrations(
    manager: &Arc<WorktreeManager>,
) -> Result<Vec<ToolRegistration>, WorktreeBundleError> {
    let executor: Arc<dyn crate::pipeline::ToolExecutor> =
        Arc::new(WorktreeExecutor::new(Arc::clone(manager)));
    let mut registrations = Vec::new();
    registrations
        .try_reserve_exact(2)
        .map_err(|_| WorktreeBundleError)?;
    for name in TOOLS {
        registrations.push(ToolRegistration {
            definition: Arc::new(definition(name)?),
            executor: Arc::clone(&executor),
        });
    }
    Ok(registrations)
}

fn definition(name: &str) -> Result<ToolDefinition, WorktreeBundleError> {
    Ok(ToolDefinition::new(
        InternalToolName::new(name.into()).map_err(|_| WorktreeBundleError)?,
        ModelFacingToolName::new(name.into()).map_err(|_| WorktreeBundleError)?,
        schema(name)?,
        ToolDescriptionAsset::new(description(name).trim().into())
            .map_err(|_| WorktreeBundleError)?,
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Always,
        PermissionAction::new("execute".into()).map_err(|_| WorktreeBundleError)?,
        ToolTimeout::new(Duration::from_millis(
            EXTERNAL_TOOL_CALL_TIMEOUT_MS.value as u64,
        ))
        .map_err(|_| WorktreeBundleError)?,
        ToolOutputLimit::new(
            TOOL_RESULT_BYTES_MAX.value,
            TOOL_RESULT_MODEL_CHARS_MAX.value,
        )
        .map_err(|_| WorktreeBundleError)?,
        serde_json::from_value::<SecretRedactionSpec>(json!({"fields":[],"policy":"redact"}))
            .map_err(|_| WorktreeBundleError)?,
    ))
}

fn schema(name: &str) -> Result<ToolInputSchema, WorktreeBundleError> {
    let value: Value = serde_json::from_str(schema_asset(name)).map_err(|_| WorktreeBundleError)?;
    ToolInputSchema::new(BoundedJsonValue::new(value).map_err(|_| WorktreeBundleError)?)
        .map_err(|_| WorktreeBundleError)
}

pub(super) fn schema_asset(name: &str) -> &'static str {
    match name {
        "EnterWorktree" => include_str!("assets/schemas/EnterWorktree.json"),
        "ExitWorktree" => include_str!("assets/schemas/ExitWorktree.json"),
        _ => "",
    }
}

pub(super) fn description(name: &str) -> &'static str {
    match name {
        "EnterWorktree" => include_str!("assets/descriptions/EnterWorktree.md"),
        "ExitWorktree" => include_str!("assets/descriptions/ExitWorktree.md"),
        _ => "",
    }
}

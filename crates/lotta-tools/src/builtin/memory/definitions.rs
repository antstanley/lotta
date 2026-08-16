use super::{MemoryBundleError, MemoryState, executor::MemoryExecutor};
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

const TOOLS: [&str; 2] = ["memory", "memory_apply_patch"];

pub(super) fn registrations(
    state: &Arc<MemoryState>,
) -> Result<Vec<ToolRegistration>, MemoryBundleError> {
    let executor: Arc<dyn crate::pipeline::ToolExecutor> =
        Arc::new(MemoryExecutor::new(Arc::clone(state)));
    let mut registrations = Vec::new();
    registrations
        .try_reserve_exact(TOOLS.len())
        .map_err(|_| MemoryBundleError)?;
    for name in TOOLS {
        registrations.push(ToolRegistration {
            definition: Arc::new(definition(name)?),
            executor: Arc::clone(&executor),
        });
    }
    Ok(registrations)
}

fn definition(name: &str) -> Result<ToolDefinition, MemoryBundleError> {
    Ok(ToolDefinition::new(
        InternalToolName::new(name.into()).map_err(|_| MemoryBundleError)?,
        ModelFacingToolName::new(name.into()).map_err(|_| MemoryBundleError)?,
        schema(name)?,
        ToolDescriptionAsset::new(description(name).trim().into())
            .map_err(|_| MemoryBundleError)?,
        ToolExecutionOwner::Rust,
        ToolApprovalPolicy::Always,
        PermissionAction::new("write".into()).map_err(|_| MemoryBundleError)?,
        ToolTimeout::new(Duration::from_millis(
            EXTERNAL_TOOL_CALL_TIMEOUT_MS.value as u64,
        ))
        .map_err(|_| MemoryBundleError)?,
        ToolOutputLimit::new(
            TOOL_RESULT_BYTES_MAX.value,
            TOOL_RESULT_MODEL_CHARS_MAX.value,
        )
        .map_err(|_| MemoryBundleError)?,
        serde_json::from_value::<SecretRedactionSpec>(json!({"fields":[],"policy":"redact"}))
            .map_err(|_| MemoryBundleError)?,
    ))
}

fn schema(name: &str) -> Result<ToolInputSchema, MemoryBundleError> {
    let value: Value = serde_json::from_str(schema_asset(name)).map_err(|_| MemoryBundleError)?;
    ToolInputSchema::new(BoundedJsonValue::new(value).map_err(|_| MemoryBundleError)?)
        .map_err(|_| MemoryBundleError)
}

pub(super) fn schema_asset(name: &str) -> &'static str {
    match name {
        "memory" => include_str!("assets/schemas/Memory.json"),
        "memory_apply_patch" => include_str!("assets/schemas/MemoryApplyPatch.json"),
        _ => "",
    }
}

pub(super) fn description(name: &str) -> &'static str {
    match name {
        "memory" => include_str!("assets/descriptions/Memory.md"),
        "memory_apply_patch" => include_str!("assets/descriptions/MemoryApplyPatch.md"),
        _ => "",
    }
}

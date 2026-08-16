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

use crate::pipeline::ToolExecutor;

pub(crate) fn registration(
    name: &str,
    schema: &'static str,
    description: &'static str,
    approval: ToolApprovalPolicy,
    action: &str,
    executor: Arc<dyn ToolExecutor>,
) -> Result<ToolRegistration, ()> {
    let schema: Value = serde_json::from_str(schema).map_err(|_| ())?;
    let schema =
        ToolInputSchema::new(BoundedJsonValue::new(schema).map_err(|_| ())?).map_err(|_| ())?;
    let secrets =
        serde_json::from_value::<SecretRedactionSpec>(json!({"fields":[],"policy":"redact"}))
            .map_err(|_| ())?;
    let definition = ToolDefinition::new(
        InternalToolName::new(name.to_owned()).map_err(|_| ())?,
        ModelFacingToolName::new(name.to_owned()).map_err(|_| ())?,
        schema,
        ToolDescriptionAsset::new(description.to_owned()).map_err(|_| ())?,
        ToolExecutionOwner::Rust,
        approval,
        PermissionAction::new(action.to_owned()).map_err(|_| ())?,
        ToolTimeout::new(Duration::from_millis(
            EXTERNAL_TOOL_CALL_TIMEOUT_MS.value as u64,
        ))
        .map_err(|_| ())?,
        ToolOutputLimit::new(
            TOOL_RESULT_BYTES_MAX.value,
            TOOL_RESULT_MODEL_CHARS_MAX.value,
        )
        .map_err(|_| ())?,
        secrets,
    );
    Ok(ToolRegistration {
        definition: Arc::new(definition),
        executor,
    })
}

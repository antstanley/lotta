use super::{FileBundleError, FileState, executor::FileExecutor};
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

const TOOLS: &[(&str, &str)] = &[
    ("Read", "read"),
    ("Write", "write"),
    ("Edit", "write"),
    ("MultiEdit", "write"),
    ("apply_patch", "write"),
    ("LS", "read"),
    ("Glob", "read"),
    ("Grep", "read"),
    ("view_image", "read"),
    ("read_artifact_file", "read"),
    ("write_artifact_file", "write"),
    ("read_file_gemini", "read"),
    ("read_many_files", "read"),
    ("write_file_gemini", "write"),
    ("replace", "write"),
    ("list_directory", "read"),
    ("glob_gemini", "read"),
    ("search_file_content", "read"),
];

pub(super) fn registrations(
    state: &Arc<FileState>,
) -> Result<Vec<ToolRegistration>, FileBundleError> {
    let executor = Arc::new(FileExecutor::new(Arc::clone(state)));
    let mut output = Vec::new();
    output
        .try_reserve_exact(TOOLS.len())
        .map_err(|_| FileBundleError)?;
    for &(name, action) in TOOLS {
        output.push(ToolRegistration {
            definition: Arc::new(definition(name, action)?),
            executor: executor.clone(),
        });
    }
    Ok(output)
}

fn definition(name: &str, action: &str) -> Result<ToolDefinition, FileBundleError> {
    Ok(ToolDefinition::new(
        InternalToolName::new(name.to_owned()).map_err(|_| FileBundleError)?,
        ModelFacingToolName::new(name.to_owned()).map_err(|_| FileBundleError)?,
        schema(name)?,
        ToolDescriptionAsset::new(description(name).trim().to_owned())
            .map_err(|_| FileBundleError)?,
        ToolExecutionOwner::Rust,
        approval_policy(name),
        PermissionAction::new(action.to_owned()).map_err(|_| FileBundleError)?,
        ToolTimeout::new(Duration::from_millis(EXTERNAL_TOOL_CALL_TIMEOUT_MS as u64))
            .map_err(|_| FileBundleError)?,
        ToolOutputLimit::new(
            TOOL_RESULT_BYTES_MAX.value,
            TOOL_RESULT_MODEL_CHARS_MAX.value,
        )
        .map_err(|_| FileBundleError)?,
        serde_json::from_value::<SecretRedactionSpec>(json!({"fields":[],"policy":"redact"}))
            .map_err(|_| FileBundleError)?,
    ))
}

pub(super) fn approval_policy(name: &str) -> ToolApprovalPolicy {
    match name {
        "Write" | "Edit" | "MultiEdit" | "apply_patch" | "write_file_gemini" | "replace" => {
            ToolApprovalPolicy::Always
        }
        _ => ToolApprovalPolicy::Never,
    }
}

fn schema(name: &str) -> Result<ToolInputSchema, FileBundleError> {
    let value: Value = serde_json::from_str(schema_asset(name)?).map_err(|_| FileBundleError)?;
    ToolInputSchema::new(BoundedJsonValue::new(value).map_err(|_| FileBundleError)?)
        .map_err(|_| FileBundleError)
}

pub(super) fn schema_asset(name: &str) -> Result<&'static str, FileBundleError> {
    match name {
        "Read" => Ok(include_str!("assets/schemas/Read.json")),
        "Write" => Ok(include_str!("assets/schemas/Write.json")),
        "Edit" => Ok(include_str!("assets/schemas/Edit.json")),
        "MultiEdit" => Ok(include_str!("assets/schemas/MultiEdit.json")),
        "apply_patch" => Ok(include_str!("assets/schemas/ApplyPatch.json")),
        "LS" => Ok(include_str!("assets/schemas/LS.json")),
        "Glob" => Ok(include_str!("assets/schemas/Glob.json")),
        "Grep" => Ok(include_str!("assets/schemas/Grep.json")),
        "view_image" => Ok(include_str!("assets/schemas/ViewImage.json")),
        "read_artifact_file" => Ok(include_str!("assets/schemas/ReadArtifactFile.json")),
        "write_artifact_file" => Ok(include_str!("assets/schemas/WriteArtifactFile.json")),
        "read_file_gemini" => Ok(include_str!("assets/schemas/ReadFileGemini.json")),
        "read_many_files" => Ok(include_str!("assets/schemas/ReadManyFilesGemini.json")),
        "write_file_gemini" => Ok(include_str!("assets/schemas/WriteFileGemini.json")),
        "replace" => Ok(include_str!("assets/schemas/ReplaceGemini.json")),
        "list_directory" => Ok(include_str!("assets/schemas/ListDirectoryGemini.json")),
        "glob_gemini" => Ok(include_str!("assets/schemas/GlobGemini.json")),
        "search_file_content" => Ok(include_str!("assets/schemas/SearchFileContentGemini.json")),
        _ => Err(FileBundleError),
    }
}

pub(super) fn description(name: &str) -> &'static str {
    match name {
        "Read" => include_str!("assets/descriptions/Read.md"),
        "Write" => include_str!("assets/descriptions/Write.md"),
        "Edit" => include_str!("assets/descriptions/Edit.md"),
        "MultiEdit" => include_str!("assets/descriptions/MultiEdit.md"),
        "apply_patch" => include_str!("assets/descriptions/ApplyPatch.md"),
        "LS" => include_str!("assets/descriptions/LS.md"),
        "Glob" => include_str!("assets/descriptions/Glob.md"),
        "Grep" => include_str!("assets/descriptions/Grep.md"),
        "view_image" => include_str!("assets/descriptions/ViewImage.md"),
        "read_artifact_file" => include_str!("assets/descriptions/ReadArtifactFile.md"),
        "write_artifact_file" => include_str!("assets/descriptions/WriteArtifactFile.md"),
        "read_file_gemini" => include_str!("assets/descriptions/ReadFileGemini.md"),
        "read_many_files" => include_str!("assets/descriptions/ReadManyFilesGemini.md"),
        "write_file_gemini" => include_str!("assets/descriptions/WriteFileGemini.md"),
        "replace" => include_str!("assets/descriptions/ReplaceGemini.md"),
        "list_directory" => include_str!("assets/descriptions/ListDirectoryGemini.md"),
        "glob_gemini" => include_str!("assets/descriptions/GlobGemini.md"),
        "search_file_content" => include_str!("assets/descriptions/SearchFileContentGemini.md"),
        _ => "",
    }
}

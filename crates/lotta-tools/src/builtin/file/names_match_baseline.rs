use super::{FileToolBundle, definitions, test_support::Fixture};
use crate::{ToolRegistry, ToolsetId};
use lotta_runtime::{
    bounds::{EXTERNAL_TOOL_CALL_TIMEOUT_MS, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX},
    ports::{ParallelSafety, ToolExecutionOwner},
};
use std::{collections::BTreeSet, sync::Arc};

#[test]
fn names_match_baseline() {
    let fixture = Fixture::new("names");
    let bundle = FileToolBundle::new(&fixture.workspace, &fixture.artifacts).unwrap();
    let registrations = bundle.registrations();
    assert_eq!(registrations.len(), 18);
    let internals: BTreeSet<_> = registrations
        .iter()
        .map(|item| item.definition.internal_name.as_str())
        .collect();
    assert_eq!(internals.len(), 18);
    let registry = ToolRegistry::new(registrations.to_vec()).unwrap();
    assert_model_names(&registry, &internals);
    alias_identity(
        &registry,
        ToolsetId::Codex,
        "ApplyPatch",
        ToolsetId::CodexSnake,
        "apply_patch",
    );
    alias_identity(
        &registry,
        ToolsetId::Codex,
        "ViewImage",
        ToolsetId::CodexSnake,
        "view_image",
    );
    assert_gemini_aliases(&registry);
    let default_rows: BTreeSet<_> = crate::names::rows()
        .iter()
        .filter(|row| row.toolset == ToolsetId::Default && internals.contains(row.internal))
        .map(|row| row.internal)
        .collect();
    assert_eq!(default_rows, BTreeSet::from(["Edit", "Read", "Write"]));
    for registration in registrations {
        assert_definition(&registration.definition);
    }
}

fn assert_model_names(registry: &ToolRegistry, internals: &BTreeSet<&str>) {
    for toolset in ToolsetId::ALL {
        let rows: Vec<_> = crate::names::rows()
            .iter()
            .filter(|row| row.toolset == toolset && internals.contains(row.internal))
            .collect();
        let allow: Vec<_> = rows.iter().map(|row| row.internal).collect();
        let snapshot = registry.update(toolset, &[], Some(&allow)).unwrap();
        let actual: BTreeSet<_> = snapshot
            .model_names()
            .into_iter()
            .map(|model| {
                (
                    model,
                    snapshot
                        .by_model(model)
                        .unwrap()
                        .definition
                        .internal_name
                        .as_str(),
                )
            })
            .collect();
        let expected: BTreeSet<_> = rows.iter().map(|row| (row.model, row.internal)).collect();
        assert_eq!(actual, expected);
    }
}

fn assert_gemini_aliases(registry: &ToolRegistry) {
    for name in [
        "read_file_gemini",
        "read_many_files",
        "write_file_gemini",
        "replace",
        "list_directory",
        "glob_gemini",
        "search_file_content",
    ] {
        let pascal = crate::names::model_name(ToolsetId::Gemini, name).unwrap();
        let snake = crate::names::model_name(ToolsetId::GeminiSnake, name).unwrap();
        alias_identity(
            registry,
            ToolsetId::Gemini,
            pascal,
            ToolsetId::GeminiSnake,
            snake,
        );
    }
}

fn assert_definition(definition: &lotta_runtime::ports::ToolDefinition) {
    let name = definition.internal_name.as_str();
    let schema: serde_json::Value =
        serde_json::from_str(definitions::schema_asset(name).unwrap()).unwrap();
    assert_eq!(definition.input_schema.as_value(), &schema);
    assert_eq!(
        definition.description.as_str(),
        definitions::description(name).trim()
    );
    let expected_approval = if requires_approval(name) {
        lotta_runtime::ports::ToolApprovalPolicy::Always
    } else {
        lotta_runtime::ports::ToolApprovalPolicy::Never
    };
    assert_eq!(definition.approval_policy, expected_approval);
    let expected_action = if is_write(name) { "write" } else { "read" };
    assert_eq!(definition.permission_action.as_str(), expected_action);
    assert_eq!(definition.execution_owner, ToolExecutionOwner::Rust);
    assert_eq!(definition.parallel_safety, ParallelSafety::Sequential);
    assert_eq!(
        definition.timeout.get().as_millis(),
        EXTERNAL_TOOL_CALL_TIMEOUT_MS.value as u128
    );
    assert_eq!(
        definition.output_limit.bytes_max(),
        TOOL_RESULT_BYTES_MAX.value
    );
    assert_eq!(
        definition.output_limit.model_chars_max(),
        TOOL_RESULT_MODEL_CHARS_MAX.value
    );
    assert!(definition.secret_redaction.fields().is_empty());
}

fn requires_approval(name: &str) -> bool {
    matches!(
        name,
        "Write" | "Edit" | "MultiEdit" | "apply_patch" | "write_file_gemini" | "replace"
    )
}

fn is_write(name: &str) -> bool {
    matches!(
        name,
        "Write"
            | "Edit"
            | "MultiEdit"
            | "apply_patch"
            | "write_artifact_file"
            | "write_file_gemini"
            | "replace"
    )
}

fn alias_identity(
    registry: &ToolRegistry,
    left_set: ToolsetId,
    left: &str,
    right_set: ToolsetId,
    right: &str,
) {
    let left_snapshot = registry.update(left_set, &[], Some(&[left])).unwrap();
    let left_tool = left_snapshot.by_model(left).unwrap().clone();
    let right_snapshot = registry.update(right_set, &[], Some(&[right])).unwrap();
    let right_tool = right_snapshot.by_model(right).unwrap();
    assert!(Arc::ptr_eq(&left_tool.definition, &right_tool.definition));
    assert!(Arc::ptr_eq(&left_tool.executor, &right_tool.executor));
}

//! Canonical built-in inventories and model-facing names.

use crate::toolset::ToolsetId;

/// One exact model-facing inventory row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolNameRow {
    /// Concrete toolset containing the built-in.
    pub toolset: ToolsetId,
    /// Canonical implementation/registration name.
    pub internal: &'static str,
    /// Name exposed to the model.
    pub model: &'static str,
}

macro_rules! row {
    ($set:ident, $internal:literal) => {
        ToolNameRow {
            toolset: ToolsetId::$set,
            internal: $internal,
            model: $internal,
        }
    };
    ($set:ident, $internal:literal, $model:literal) => {
        ToolNameRow {
            toolset: ToolsetId::$set,
            internal: $internal,
            model: $model,
        }
    };
}

/// Production model-facing rows, canonicalized to one implementation per alias family.
#[must_use]
pub const fn rows() -> &'static [ToolNameRow] {
    PRODUCTION_ROWS
}

/// Resolves a model-facing name to its canonical internal built-in name.
#[must_use]
pub fn internal_name(toolset: ToolsetId, model_name: &str) -> Option<&'static str> {
    if model_name == "Agent" {
        return Some("Task");
    }
    rows()
        .iter()
        .find_map(|row| (row.toolset == toolset && row.model == model_name).then_some(row.internal))
}

/// Resolves a canonical internal built-in name to its model-facing name.
#[must_use]
pub fn model_name(toolset: ToolsetId, internal_name: &str) -> Option<&'static str> {
    if internal_name == "Task" {
        return Some("Agent");
    }
    rows().iter().find_map(|row| {
        (row.toolset == toolset && row.internal == internal_name).then_some(row.model)
    })
}

const PRODUCTION_ROWS: &[ToolNameRow] = &[
    row!(Default, "AskUserQuestion"),
    row!(Default, "Bash"),
    row!(Default, "Monitor"),
    row!(Default, "TaskOutput"),
    row!(Default, "EnterWorktree"),
    row!(Default, "ExitWorktree"),
    row!(Default, "Edit"),
    row!(Default, "TaskStop"),
    row!(Default, "memory"),
    row!(Default, "Read"),
    row!(Default, "Skill"),
    row!(Default, "Task", "Agent"),
    row!(Default, "TaskCreate"),
    row!(Default, "TaskGet"),
    row!(Default, "TaskList"),
    row!(Default, "TaskUpdate"),
    row!(Default, "Write"),
    row!(CodexSnake, "exec_command"),
    row!(CodexSnake, "write_stdin"),
    row!(CodexSnake, "apply_patch"),
    row!(CodexSnake, "memory_apply_patch"),
    row!(CodexSnake, "update_plan"),
    row!(CodexSnake, "view_image"),
    row!(GeminiSnake, "run_shell_command"),
    row!(GeminiSnake, "read_file_gemini", "read_file"),
    row!(GeminiSnake, "list_directory"),
    row!(GeminiSnake, "glob_gemini", "glob"),
    row!(GeminiSnake, "search_file_content"),
    row!(GeminiSnake, "memory"),
    row!(GeminiSnake, "EnterWorktree"),
    row!(GeminiSnake, "ExitWorktree"),
    row!(GeminiSnake, "replace"),
    row!(GeminiSnake, "write_file_gemini", "write_file"),
    row!(GeminiSnake, "write_todos"),
    row!(GeminiSnake, "read_many_files"),
    row!(GeminiSnake, "Skill"),
    row!(GeminiSnake, "Task", "Agent"),
    row!(Codex, "AskUserQuestion"),
    row!(Codex, "EnterWorktree"),
    row!(Codex, "ExitWorktree"),
    row!(Codex, "memory_apply_patch"),
    row!(Codex, "Task", "Agent"),
    row!(Codex, "Monitor"),
    row!(Codex, "TaskOutput"),
    row!(Codex, "TaskStop"),
    row!(Codex, "Skill"),
    row!(Codex, "exec_command"),
    row!(Codex, "write_stdin"),
    row!(Codex, "view_image", "ViewImage"),
    row!(Codex, "apply_patch", "ApplyPatch"),
    row!(Codex, "update_plan", "UpdatePlan"),
    row!(Gemini, "AskUserQuestion"),
    row!(Gemini, "EnterWorktree"),
    row!(Gemini, "ExitWorktree"),
    row!(Gemini, "memory"),
    row!(Gemini, "Skill"),
    row!(Gemini, "Task", "Agent"),
    row!(Gemini, "run_shell_command", "RunShellCommand"),
    row!(Gemini, "read_file_gemini", "ReadFileGemini"),
    row!(Gemini, "list_directory", "ListDirectory"),
    row!(Gemini, "glob_gemini", "GlobGemini"),
    row!(Gemini, "search_file_content", "SearchFileContent"),
    row!(Gemini, "replace", "Replace"),
    row!(Gemini, "write_file_gemini", "WriteFileGemini"),
    row!(Gemini, "write_todos", "WriteTodos"),
    row!(Gemini, "read_many_files", "ReadManyFiles"),
];

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::registry::tests::support::{registration, registry_with};
    use std::sync::Arc;

    #[test]
    fn task_is_agent() {
        for set in ToolsetId::ALL {
            assert_eq!(model_name(set, "Task"), Some("Agent"));
            assert_eq!(internal_name(set, "Agent"), Some("Task"));
        }
        for set in [ToolsetId::CodexSnake, ToolsetId::None] {
            assert!(
                !rows()
                    .iter()
                    .any(|row| row.toolset == set && row.internal == "Task")
            );
        }
    }

    #[test]
    fn aliases_share_impl() {
        let executor = registration("apply_patch", "apply_patch").executor;
        let registry = registry_with([
            registration("apply_patch", "apply_patch").with_executor(Arc::clone(&executor))
        ]);
        registry
            .update(ToolsetId::Codex, &[], Some(&["apply_patch"]))
            .unwrap();
        let pascal = registry
            .snapshot()
            .unwrap()
            .by_model("ApplyPatch")
            .unwrap()
            .executor
            .clone();
        registry
            .update(ToolsetId::CodexSnake, &[], Some(&["apply_patch"]))
            .unwrap();
        let snake = registry
            .snapshot()
            .unwrap()
            .by_model("apply_patch")
            .unwrap()
            .executor
            .clone();
        assert!(Arc::ptr_eq(&pascal, &snake));
        registry
            .update(ToolsetId::Codex, &[], Some(&["apply_patch"]))
            .unwrap();
        let codex = registry
            .snapshot()
            .unwrap()
            .by_model("ApplyPatch")
            .unwrap()
            .clone();
        registry
            .update(ToolsetId::CodexSnake, &[], Some(&["apply_patch"]))
            .unwrap();
        let snake = registry
            .snapshot()
            .unwrap()
            .by_model("apply_patch")
            .unwrap()
            .clone();
        assert!(Arc::ptr_eq(&codex.definition, &snake.definition));
        assert!(Arc::ptr_eq(&codex.executor, &snake.executor));
    }
}

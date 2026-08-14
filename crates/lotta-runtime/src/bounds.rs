//! Immutable metadata for Task 06 runtime port resource bounds.

use lotta_domain::bounds::{BoundReached, ResourceBound};

macro_rules! bound {
    ($name:ident, $value:expr, $event:literal, $counter:literal) => {
        #[doc = concat!("Runtime resource bound `", stringify!($name), "`.")]
        pub const $name: ResourceBound = ResourceBound {
            name: stringify!($name),
            value: $value,
            event: $event,
            counter: $counter,
            reached: BoundReached::Reject,
        };
    };
}
bound!(
    MEMORY_FILES_MAX,
    100_000,
    "memory_file_count_limit",
    "memory_file_count_limit_total"
);
bound!(
    MEMORY_FILE_BYTES_MAX,
    8 * 1024 * 1024,
    "memory_file_byte_limit",
    "memory_file_byte_limit_total"
);
bound!(
    PROCESS_PROGRAM_BYTES_MAX,
    4_096,
    "process_program_byte_limit",
    "process_program_byte_limit_total"
);
bound!(
    PROCESS_ARGUMENT_BYTES_MAX,
    256 * 1024,
    "process_argument_byte_limit",
    "process_argument_byte_limit_total"
);
bound!(
    PROCESS_ARGUMENTS_ITEMS_MAX,
    256,
    "process_argument_count_limit",
    "process_argument_count_limit_total"
);
bound!(
    PROCESS_ENVIRONMENT_NAME_BYTES_MAX,
    4_096,
    "process_environment_name_byte_limit",
    "process_environment_name_byte_limit_total"
);
bound!(
    PROCESS_ENVIRONMENT_VALUE_BYTES_MAX,
    256 * 1024,
    "process_environment_value_byte_limit",
    "process_environment_value_byte_limit_total"
);
bound!(
    PROCESS_ENVIRONMENT_ITEMS_MAX,
    256,
    "process_environment_count_limit",
    "process_environment_count_limit_total"
);
bound!(
    PROCESS_STDIN_BYTES_MAX,
    1024 * 1024,
    "process_stdin_byte_limit",
    "process_stdin_byte_limit_total"
);
bound!(
    PROCESS_OUTPUT_CHUNK_BYTES_MAX,
    64 * 1024,
    "process_output_chunk_byte_limit",
    "process_output_chunk_byte_limit_total"
);
bound!(
    PROCESS_OUTPUT_TOTAL_BYTES_MAX,
    64 * 1024 * 1024,
    "process_output_total_byte_limit",
    "process_output_total_byte_limit_total"
);
bound!(
    COMMIT_MESSAGE_BYTES_MAX,
    64 * 1024,
    "commit_message_byte_limit",
    "commit_message_byte_limit_total"
);
bound!(
    REVISION_ID_BYTES_MAX,
    4_096,
    "revision_id_byte_limit",
    "revision_id_byte_limit_total"
);
bound!(
    WORKTREE_ID_BYTES_MAX,
    4_096,
    "worktree_id_byte_limit",
    "worktree_id_byte_limit_total"
);
bound!(
    MEMFS_DIFF_CHUNK_BYTES_MAX,
    64 * 1024,
    "memfs_diff_chunk_byte_limit",
    "memfs_diff_chunk_byte_limit_total"
);
bound!(
    REPOSITORY_PATH_BYTES_MAX,
    4_096,
    "repository_path_byte_limit",
    "repository_path_byte_limit_total"
);
bound!(
    REPOSITORY_PATH_COMPONENTS_MAX,
    256,
    "repository_path_component_limit",
    "repository_path_component_limit_total"
);
bound!(
    CONFINED_PATH_BYTES_MAX,
    4_096,
    "confined_path_byte_limit",
    "confined_path_byte_limit_total"
);
bound!(
    CONFINED_PATH_COMPONENTS_MAX,
    256,
    "confined_path_component_limit",
    "confined_path_component_limit_total"
);

/// All Task 06 runtime bounds in stable declaration order.
pub const RUNTIME_RESOURCE_BOUNDS: [ResourceBound; 19] = [
    MEMORY_FILES_MAX,
    MEMORY_FILE_BYTES_MAX,
    PROCESS_PROGRAM_BYTES_MAX,
    PROCESS_ARGUMENT_BYTES_MAX,
    PROCESS_ARGUMENTS_ITEMS_MAX,
    PROCESS_ENVIRONMENT_NAME_BYTES_MAX,
    PROCESS_ENVIRONMENT_VALUE_BYTES_MAX,
    PROCESS_ENVIRONMENT_ITEMS_MAX,
    PROCESS_STDIN_BYTES_MAX,
    PROCESS_OUTPUT_CHUNK_BYTES_MAX,
    PROCESS_OUTPUT_TOTAL_BYTES_MAX,
    COMMIT_MESSAGE_BYTES_MAX,
    REVISION_ID_BYTES_MAX,
    WORKTREE_ID_BYTES_MAX,
    MEMFS_DIFF_CHUNK_BYTES_MAX,
    REPOSITORY_PATH_BYTES_MAX,
    REPOSITORY_PATH_COMPONENTS_MAX,
    CONFINED_PATH_BYTES_MAX,
    CONFINED_PATH_COMPONENTS_MAX,
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the exact 19-row drift fixture is intentionally explicit"
    )]
    fn exact_nineteen_row_table_does_not_drift() {
        let expected = [
            (
                "MEMORY_FILES_MAX",
                100_000,
                "memory_file_count_limit",
                "memory_file_count_limit_total",
            ),
            (
                "MEMORY_FILE_BYTES_MAX",
                8 * 1024 * 1024,
                "memory_file_byte_limit",
                "memory_file_byte_limit_total",
            ),
            (
                "PROCESS_PROGRAM_BYTES_MAX",
                4_096,
                "process_program_byte_limit",
                "process_program_byte_limit_total",
            ),
            (
                "PROCESS_ARGUMENT_BYTES_MAX",
                256 * 1024,
                "process_argument_byte_limit",
                "process_argument_byte_limit_total",
            ),
            (
                "PROCESS_ARGUMENTS_ITEMS_MAX",
                256,
                "process_argument_count_limit",
                "process_argument_count_limit_total",
            ),
            (
                "PROCESS_ENVIRONMENT_NAME_BYTES_MAX",
                4_096,
                "process_environment_name_byte_limit",
                "process_environment_name_byte_limit_total",
            ),
            (
                "PROCESS_ENVIRONMENT_VALUE_BYTES_MAX",
                256 * 1024,
                "process_environment_value_byte_limit",
                "process_environment_value_byte_limit_total",
            ),
            (
                "PROCESS_ENVIRONMENT_ITEMS_MAX",
                256,
                "process_environment_count_limit",
                "process_environment_count_limit_total",
            ),
            (
                "PROCESS_STDIN_BYTES_MAX",
                1024 * 1024,
                "process_stdin_byte_limit",
                "process_stdin_byte_limit_total",
            ),
            (
                "PROCESS_OUTPUT_CHUNK_BYTES_MAX",
                64 * 1024,
                "process_output_chunk_byte_limit",
                "process_output_chunk_byte_limit_total",
            ),
            (
                "PROCESS_OUTPUT_TOTAL_BYTES_MAX",
                64 * 1024 * 1024,
                "process_output_total_byte_limit",
                "process_output_total_byte_limit_total",
            ),
            (
                "COMMIT_MESSAGE_BYTES_MAX",
                64 * 1024,
                "commit_message_byte_limit",
                "commit_message_byte_limit_total",
            ),
            (
                "REVISION_ID_BYTES_MAX",
                4_096,
                "revision_id_byte_limit",
                "revision_id_byte_limit_total",
            ),
            (
                "WORKTREE_ID_BYTES_MAX",
                4_096,
                "worktree_id_byte_limit",
                "worktree_id_byte_limit_total",
            ),
            (
                "MEMFS_DIFF_CHUNK_BYTES_MAX",
                64 * 1024,
                "memfs_diff_chunk_byte_limit",
                "memfs_diff_chunk_byte_limit_total",
            ),
            (
                "REPOSITORY_PATH_BYTES_MAX",
                4_096,
                "repository_path_byte_limit",
                "repository_path_byte_limit_total",
            ),
            (
                "REPOSITORY_PATH_COMPONENTS_MAX",
                256,
                "repository_path_component_limit",
                "repository_path_component_limit_total",
            ),
            (
                "CONFINED_PATH_BYTES_MAX",
                4_096,
                "confined_path_byte_limit",
                "confined_path_byte_limit_total",
            ),
            (
                "CONFINED_PATH_COMPONENTS_MAX",
                256,
                "confined_path_component_limit",
                "confined_path_component_limit_total",
            ),
        ];
        assert_eq!(expected.len(), RUNTIME_RESOURCE_BOUNDS.len());
        for (bound, (name, value, event, counter)) in RUNTIME_RESOURCE_BOUNDS.iter().zip(expected) {
            assert_eq!(
                (bound.name, bound.value, bound.event, bound.counter),
                (name, value, event, counter)
            );
            assert_eq!(bound.reached, BoundReached::Reject);
            assert!(name.ends_with("_MAX"));
            assert!(bound.observe(value - 1).is_none());
            assert!(!bound.observe(value).unwrap().exceeded);
            assert!(bound.observe(value + 1).unwrap().exceeded);
        }
    }
}

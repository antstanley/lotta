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
bound!(
    PROVIDER_REQUEST_BYTES_MAX,
    32 * 1024 * 1024,
    "provider_request_byte_limit",
    "provider_request_byte_limit_total"
);
bound!(
    PROVIDER_RESPONSE_EVENT_BYTES_MAX,
    8 * 1024 * 1024,
    "provider_response_event_byte_limit",
    "provider_response_event_byte_limit_total"
);
bound!(
    TOOL_ARGUMENT_BYTES_MAX,
    4 * 1024 * 1024,
    "tool_argument_byte_limit",
    "tool_argument_byte_limit_total"
);
bound!(
    PROVIDER_STREAM_EVENTS_MAX,
    256,
    "provider_stream_event_count_limit",
    "provider_stream_event_count_limit_total"
);
bound!(
    PROVIDER_MESSAGES_MAX,
    100_000,
    "provider_message_count_limit",
    "provider_message_count_limit_total"
);
bound!(
    PROVIDER_CONTENT_PARTS_MAX,
    100_000,
    "provider_content_part_count_limit",
    "provider_content_part_count_limit_total"
);
bound!(
    PROVIDER_TOOLS_MAX,
    10_000,
    "provider_tool_count_limit",
    "provider_tool_count_limit_total"
);

/// Task 07 provider bounds in stable declaration order.
pub const PROVIDER_RESOURCE_BOUNDS: [ResourceBound; 7] = [
    PROVIDER_REQUEST_BYTES_MAX,
    PROVIDER_RESPONSE_EVENT_BYTES_MAX,
    TOOL_ARGUMENT_BYTES_MAX,
    PROVIDER_STREAM_EVENTS_MAX,
    PROVIDER_MESSAGES_MAX,
    PROVIDER_CONTENT_PARTS_MAX,
    PROVIDER_TOOLS_MAX,
];

bound!(
    TOOL_INPUT_BYTES_MAX,
    4 * 1024 * 1024,
    "tool_input_byte_limit",
    "tool_input_byte_limit_total"
);
bound!(
    TOOL_RESULT_BYTES_MAX,
    1024 * 1024,
    "tool_result_byte_limit",
    "tool_result_byte_limit_total"
);
bound!(
    TOOL_RESULT_MODEL_CHARS_MAX,
    32_000,
    "tool_result_model_char_limit",
    "tool_result_model_char_limit_total"
);
bound!(
    EXTERNAL_TOOL_CALL_TIMEOUT_MS,
    300_000,
    "external_tool_call_timeout_limit",
    "external_tool_call_timeout_limit_total"
);
bound!(
    TOOL_NAME_BYTES_MAX,
    256,
    "tool_name_byte_limit",
    "tool_name_byte_limit_total"
);
bound!(
    TOOL_DESCRIPTION_BYTES_MAX,
    64 * 1024,
    "tool_description_byte_limit",
    "tool_description_byte_limit_total"
);
bound!(
    TOOL_SECRET_FIELDS_ITEMS_MAX,
    128,
    "tool_secret_field_count_limit",
    "tool_secret_field_count_limit_total"
);

/// Task 08 tool-contract bounds in stable specification order.
pub const TOOL_RESOURCE_BOUNDS: [ResourceBound; 7] = [
    TOOL_INPUT_BYTES_MAX,
    TOOL_RESULT_BYTES_MAX,
    TOOL_RESULT_MODEL_CHARS_MAX,
    EXTERNAL_TOOL_CALL_TIMEOUT_MS,
    TOOL_NAME_BYTES_MAX,
    TOOL_DESCRIPTION_BYTES_MAX,
    TOOL_SECRET_FIELDS_ITEMS_MAX,
];

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
    fn exact_seven_tool_rows_do_not_drift() {
        let expected = [
            (
                "TOOL_INPUT_BYTES_MAX",
                4 * 1024 * 1024,
                "tool_input_byte_limit",
                "tool_input_byte_limit_total",
                "bytes",
            ),
            (
                "TOOL_RESULT_BYTES_MAX",
                1024 * 1024,
                "tool_result_byte_limit",
                "tool_result_byte_limit_total",
                "bytes",
            ),
            (
                "TOOL_RESULT_MODEL_CHARS_MAX",
                32_000,
                "tool_result_model_char_limit",
                "tool_result_model_char_limit_total",
                "chars",
            ),
            (
                "EXTERNAL_TOOL_CALL_TIMEOUT_MS",
                300_000,
                "external_tool_call_timeout_limit",
                "external_tool_call_timeout_limit_total",
                "ms",
            ),
            (
                "TOOL_NAME_BYTES_MAX",
                256,
                "tool_name_byte_limit",
                "tool_name_byte_limit_total",
                "bytes",
            ),
            (
                "TOOL_DESCRIPTION_BYTES_MAX",
                64 * 1024,
                "tool_description_byte_limit",
                "tool_description_byte_limit_total",
                "bytes",
            ),
            (
                "TOOL_SECRET_FIELDS_ITEMS_MAX",
                128,
                "tool_secret_field_count_limit",
                "tool_secret_field_count_limit_total",
                "items",
            ),
        ];
        assert_eq!(TOOL_RESOURCE_BOUNDS.len(), expected.len());
        for (bound, (name, value, event, counter, units)) in
            TOOL_RESOURCE_BOUNDS.iter().zip(expected)
        {
            assert_eq!(
                (
                    bound.name,
                    bound.value,
                    bound.event,
                    bound.counter,
                    bound.reached
                ),
                (name, value, event, counter, BoundReached::Reject)
            );
            assert!(name.ends_with("_MAX") || name.ends_with("_MS"));
            assert!(matches!(units, "bytes" | "chars" | "ms" | "items"));
            assert!(bound.observe(value - 1).is_none());
            let at = bound.observe(value).unwrap();
            assert_eq!(
                (
                    at.name,
                    at.actual,
                    at.exceeded,
                    at.event,
                    at.counter,
                    at.reached
                ),
                (name, value, false, event, counter, BoundReached::Reject)
            );
            let over = bound.observe(value + 1).unwrap();
            assert_eq!(
                (
                    over.name,
                    over.actual,
                    over.exceeded,
                    over.event,
                    over.counter,
                    over.reached
                ),
                (name, value + 1, true, event, counter, BoundReached::Reject)
            );
        }
    }

    #[test]
    fn exact_seven_provider_rows_do_not_drift() {
        let expected = [
            (
                "PROVIDER_REQUEST_BYTES_MAX",
                32 * 1024 * 1024,
                "provider_request_byte_limit",
                "provider_request_byte_limit_total",
            ),
            (
                "PROVIDER_RESPONSE_EVENT_BYTES_MAX",
                8 * 1024 * 1024,
                "provider_response_event_byte_limit",
                "provider_response_event_byte_limit_total",
            ),
            (
                "TOOL_ARGUMENT_BYTES_MAX",
                4 * 1024 * 1024,
                "tool_argument_byte_limit",
                "tool_argument_byte_limit_total",
            ),
            (
                "PROVIDER_STREAM_EVENTS_MAX",
                256,
                "provider_stream_event_count_limit",
                "provider_stream_event_count_limit_total",
            ),
            (
                "PROVIDER_MESSAGES_MAX",
                100_000,
                "provider_message_count_limit",
                "provider_message_count_limit_total",
            ),
            (
                "PROVIDER_CONTENT_PARTS_MAX",
                100_000,
                "provider_content_part_count_limit",
                "provider_content_part_count_limit_total",
            ),
            (
                "PROVIDER_TOOLS_MAX",
                10_000,
                "provider_tool_count_limit",
                "provider_tool_count_limit_total",
            ),
        ];
        assert_eq!(PROVIDER_RESOURCE_BOUNDS.len(), expected.len());
        for (bound, (name, value, event, counter)) in PROVIDER_RESOURCE_BOUNDS.iter().zip(expected)
        {
            assert_eq!(
                (bound.name, bound.value, bound.event, bound.counter),
                (name, value, event, counter)
            );
            assert_eq!(bound.reached, BoundReached::Reject);
            assert!(name.ends_with("_MAX"));
            assert!(value > 0);
            assert!(bound.observe(value - 1).is_none());
            let at_limit = bound.observe(value).expect("limit must be observable");
            assert_eq!(
                (
                    at_limit.name,
                    at_limit.event,
                    at_limit.counter,
                    at_limit.reached
                ),
                (bound.name, bound.event, bound.counter, bound.reached)
            );
            assert_eq!(at_limit.actual, value);
            assert!(!at_limit.exceeded);
            let over_limit = bound
                .observe(value + 1)
                .expect("overage must be observable");
            assert_eq!(
                (
                    over_limit.name,
                    over_limit.event,
                    over_limit.counter,
                    over_limit.reached,
                ),
                (bound.name, bound.event, bound.counter, bound.reached)
            );
            assert_eq!(over_limit.actual, value + 1);
            assert!(over_limit.exceeded);
        }
    }

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

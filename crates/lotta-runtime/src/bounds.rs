//! Immutable metadata for Task 06 runtime port resource bounds.

use lotta_domain::bounds::{BoundReached, ResourceBound};

/// Maximum transient or busy retries after the initial provider attempt.
pub const TURN_PROVIDER_RETRIES_MAX: u32 = 3;
/// Maximum empty-response retries after the initial provider attempt.
pub const TURN_EMPTY_RESPONSE_RETRIES_MAX: u32 = 2;
/// Maximum context-overflow compactions before overflow becomes terminal.
pub const CONTEXT_OVERFLOW_COMPACTIONS_MAX: u8 = 3;
/// Maximum tool calls executed by one turn.
pub const TURN_TOOL_CALLS_MAX: usize = 256;
/// Maximum provider/tool steps performed by one turn.
pub const TURN_STEPS_MAX: usize = 256;
/// Cooperative cancellation grace before forceful child cleanup.
pub const TURN_CANCEL_GRACE_MS: u64 = 10_000;
/// Default local tool execution timeout.
pub const LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT: u64 = 180_000;
/// Maximum external tool call duration.
pub const EXTERNAL_TOOL_CALL_TIMEOUT_MS: usize = 300_000;
/// Maximum time an approval may remain pending.
pub const APPROVAL_WAIT_MS_MAX: u64 = 86_400_000;
/// Maximum queue items consumed before the caller must yield.
pub const QUEUE_PUMP_BATCH_MAX: usize = 64;

/// Terminal decision for a bounded operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundDecision {
    /// The operation may proceed.
    Allowed,
    /// The operation has reached its terminal bound.
    Terminal,
}

/// Outcome of comparing elapsed time with a timeout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutDecision {
    /// The operation may still complete.
    Pending,
    /// The deadline has expired, including exactly at the deadline.
    TimedOut,
}

/// Reports whether another provider retry is allowed.
#[must_use]
pub const fn provider_retry_decision(retries_completed: u32) -> BoundDecision {
    if retries_completed < TURN_PROVIDER_RETRIES_MAX {
        BoundDecision::Allowed
    } else {
        BoundDecision::Terminal
    }
}

/// Reports whether another empty-response retry is allowed.
#[must_use]
pub const fn empty_response_retry_decision(retries_completed: u32) -> BoundDecision {
    if retries_completed < TURN_EMPTY_RESPONSE_RETRIES_MAX {
        BoundDecision::Allowed
    } else {
        BoundDecision::Terminal
    }
}

/// Reports whether another context-overflow compaction is allowed.
#[must_use]
pub const fn compaction_decision(compactions_completed: u8) -> BoundDecision {
    if compactions_completed < CONTEXT_OVERFLOW_COMPACTIONS_MAX {
        BoundDecision::Allowed
    } else {
        BoundDecision::Terminal
    }
}

/// Reports whether another distinct tool call may start.
#[must_use]
pub const fn tool_call_decision(tool_calls_started: usize) -> BoundDecision {
    if tool_calls_started < TURN_TOOL_CALLS_MAX {
        BoundDecision::Allowed
    } else {
        BoundDecision::Terminal
    }
}

/// Reports whether the current provider/tool step may run.
#[must_use]
pub const fn step_decision(step_index: usize) -> BoundDecision {
    if step_index < TURN_STEPS_MAX {
        BoundDecision::Allowed
    } else {
        BoundDecision::Terminal
    }
}

/// Reports whether cancellation grace has elapsed.
#[must_use]
pub const fn cancel_grace_decision(elapsed_ms: u64) -> BoundDecision {
    if elapsed_ms < TURN_CANCEL_GRACE_MS {
        BoundDecision::Allowed
    } else {
        BoundDecision::Terminal
    }
}

/// Reports whether a local tool operation has reached its default timeout.
#[must_use]
pub const fn local_timeout_decision(elapsed_ms: u64) -> TimeoutDecision {
    if elapsed_ms < LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT {
        TimeoutDecision::Pending
    } else {
        TimeoutDecision::TimedOut
    }
}

/// Reports whether an external tool operation has reached its timeout.
#[must_use]
pub const fn external_timeout_decision(elapsed_ms: usize) -> TimeoutDecision {
    if elapsed_ms < EXTERNAL_TOOL_CALL_TIMEOUT_MS {
        TimeoutDecision::Pending
    } else {
        TimeoutDecision::TimedOut
    }
}

/// Clamps an approval wait and reports whether the resulting wait is expired.
#[must_use]
pub const fn approval_wait_decision(requested_ms: u64, elapsed_ms: u64) -> (u64, TimeoutDecision) {
    let wait_ms = if requested_ms > APPROVAL_WAIT_MS_MAX {
        APPROVAL_WAIT_MS_MAX
    } else {
        requested_ms
    };
    let decision = if elapsed_ms < wait_ms {
        TimeoutDecision::Pending
    } else {
        TimeoutDecision::TimedOut
    };
    (wait_ms, decision)
}

/// Returns the number of queue items one bounded pump may consume.
#[must_use]
pub const fn queue_pump_count(available: usize) -> usize {
    if available > QUEUE_PUMP_BATCH_MAX {
        QUEUE_PUMP_BATCH_MAX
    } else {
        available
    }
}

/// The ten runtime bounds in specification order.
pub const TURN_RUNTIME_BOUNDS: [(&str, u64); 10] = [
    (
        "TURN_PROVIDER_RETRIES_MAX",
        TURN_PROVIDER_RETRIES_MAX as u64,
    ),
    (
        "TURN_EMPTY_RESPONSE_RETRIES_MAX",
        TURN_EMPTY_RESPONSE_RETRIES_MAX as u64,
    ),
    (
        "CONTEXT_OVERFLOW_COMPACTIONS_MAX",
        CONTEXT_OVERFLOW_COMPACTIONS_MAX as u64,
    ),
    ("TURN_TOOL_CALLS_MAX", TURN_TOOL_CALLS_MAX as u64),
    ("TURN_STEPS_MAX", TURN_STEPS_MAX as u64),
    ("TURN_CANCEL_GRACE_MS", TURN_CANCEL_GRACE_MS),
    (
        "LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT",
        LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT,
    ),
    (
        "EXTERNAL_TOOL_CALL_TIMEOUT_MS",
        EXTERNAL_TOOL_CALL_TIMEOUT_MS as u64,
    ),
    ("APPROVAL_WAIT_MS_MAX", APPROVAL_WAIT_MS_MAX),
    ("QUEUE_PUMP_BATCH_MAX", QUEUE_PUMP_BATCH_MAX as u64),
];

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
    PROVIDER_IMAGE_BYTES_MAX,
    20 * 1024 * 1024,
    "provider_image_byte_limit",
    "provider_image_byte_limit_total"
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
bound!(
    TURN_TOOL_CALLS_BOUND,
    TURN_TOOL_CALLS_MAX,
    "turn_tool_call_count_limit",
    "turn_tool_call_count_limit_total"
);
bound!(
    TURN_STEPS_BOUND,
    TURN_STEPS_MAX,
    "turn_step_count_limit",
    "turn_step_count_limit_total"
);

/// Audited whole-turn resource bounds in stable declaration order.
pub const TURN_RESOURCE_BOUNDS: [ResourceBound; 2] = [TURN_TOOL_CALLS_BOUND, TURN_STEPS_BOUND];

/// Task 07 provider bounds in stable declaration order.
pub const PROVIDER_RESOURCE_BOUNDS: [ResourceBound; 8] = [
    PROVIDER_IMAGE_BYTES_MAX,
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
/// Resource metadata for the exact external tool timeout constant.
pub const EXTERNAL_TOOL_CALL_TIMEOUT_BOUND: ResourceBound = ResourceBound {
    name: "EXTERNAL_TOOL_CALL_TIMEOUT_MS",
    value: EXTERNAL_TOOL_CALL_TIMEOUT_MS,
    event: "external_tool_call_timeout_limit",
    counter: "external_tool_call_timeout_limit_total",
    reached: BoundReached::Reject,
};
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
    EXTERNAL_TOOL_CALL_TIMEOUT_BOUND,
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
    use quote::ToTokens;

    #[test]
    fn runtime_table_has_exact_names_and_defaults() {
        assert_eq!(
            TURN_RUNTIME_BOUNDS,
            [
                ("TURN_PROVIDER_RETRIES_MAX", 3),
                ("TURN_EMPTY_RESPONSE_RETRIES_MAX", 2),
                ("CONTEXT_OVERFLOW_COMPACTIONS_MAX", 3),
                ("TURN_TOOL_CALLS_MAX", 256),
                ("TURN_STEPS_MAX", 256),
                ("TURN_CANCEL_GRACE_MS", 10_000),
                ("LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT", 180_000),
                ("EXTERNAL_TOOL_CALL_TIMEOUT_MS", 300_000),
                ("APPROVAL_WAIT_MS_MAX", 86_400_000),
                ("QUEUE_PUMP_BATCH_MAX", 64),
            ]
        );
    }

    mod turn_provider_retries_max {
        use super::*;
        #[test]
        fn below() {
            assert_eq!(
                provider_retry_decision(TURN_PROVIDER_RETRIES_MAX - 1),
                BoundDecision::Allowed
            );
        }
        #[test]
        fn at() {
            assert_eq!(
                provider_retry_decision(TURN_PROVIDER_RETRIES_MAX),
                BoundDecision::Terminal
            );
        }
        #[test]
        fn above() {
            assert_eq!(
                provider_retry_decision(TURN_PROVIDER_RETRIES_MAX + 1),
                BoundDecision::Terminal
            );
        }
    }
    mod turn_empty_response_retries_max {
        use super::*;
        #[test]
        fn below() {
            assert_eq!(
                empty_response_retry_decision(TURN_EMPTY_RESPONSE_RETRIES_MAX - 1),
                BoundDecision::Allowed
            );
        }
        #[test]
        fn at() {
            assert_eq!(
                empty_response_retry_decision(TURN_EMPTY_RESPONSE_RETRIES_MAX),
                BoundDecision::Terminal
            );
        }
        #[test]
        fn above() {
            assert_eq!(
                empty_response_retry_decision(TURN_EMPTY_RESPONSE_RETRIES_MAX + 1),
                BoundDecision::Terminal
            );
        }
    }
    mod context_overflow_compactions_max {
        use super::*;
        #[test]
        fn below() {
            assert_eq!(
                compaction_decision(CONTEXT_OVERFLOW_COMPACTIONS_MAX - 1),
                BoundDecision::Allowed
            );
        }
        #[test]
        fn at() {
            assert_eq!(
                compaction_decision(CONTEXT_OVERFLOW_COMPACTIONS_MAX),
                BoundDecision::Terminal
            );
        }
        #[test]
        fn above() {
            assert_eq!(
                compaction_decision(CONTEXT_OVERFLOW_COMPACTIONS_MAX + 1),
                BoundDecision::Terminal
            );
        }
    }
    mod turn_tool_calls_max {
        use super::*;
        #[test]
        fn below() {
            assert_eq!(
                tool_call_decision(TURN_TOOL_CALLS_MAX - 1),
                BoundDecision::Allowed
            );
        }
        #[test]
        fn at() {
            assert_eq!(
                tool_call_decision(TURN_TOOL_CALLS_MAX),
                BoundDecision::Terminal
            );
        }
        #[test]
        fn above() {
            assert_eq!(
                tool_call_decision(TURN_TOOL_CALLS_MAX + 1),
                BoundDecision::Terminal
            );
        }
    }
    mod turn_steps_max {
        use super::*;
        #[test]
        fn below() {
            assert_eq!(step_decision(TURN_STEPS_MAX - 1), BoundDecision::Allowed);
        }
        #[test]
        fn at() {
            assert_eq!(step_decision(TURN_STEPS_MAX), BoundDecision::Terminal);
        }
        #[test]
        fn above() {
            assert_eq!(step_decision(TURN_STEPS_MAX + 1), BoundDecision::Terminal);
        }
    }
    mod turn_cancel_grace_ms {
        use super::*;
        #[test]
        fn below() {
            assert_eq!(
                cancel_grace_decision(TURN_CANCEL_GRACE_MS - 1),
                BoundDecision::Allowed
            );
        }
        #[test]
        fn at() {
            assert_eq!(
                cancel_grace_decision(TURN_CANCEL_GRACE_MS),
                BoundDecision::Terminal
            );
        }
        #[test]
        fn above() {
            assert_eq!(
                cancel_grace_decision(TURN_CANCEL_GRACE_MS + 1),
                BoundDecision::Terminal
            );
        }
    }
    mod local_tool_execution_timeout_ms_default {
        use super::*;
        #[test]
        fn below() {
            assert_eq!(
                local_timeout_decision(LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT - 1),
                TimeoutDecision::Pending
            );
        }
        #[test]
        fn at() {
            assert_eq!(
                local_timeout_decision(LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT),
                TimeoutDecision::TimedOut
            );
        }
        #[test]
        fn above() {
            assert_eq!(
                local_timeout_decision(LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT + 1),
                TimeoutDecision::TimedOut
            );
        }
    }
    mod external_tool_call_timeout_ms {
        use super::*;
        #[test]
        fn below() {
            assert_eq!(
                external_timeout_decision(EXTERNAL_TOOL_CALL_TIMEOUT_MS - 1),
                TimeoutDecision::Pending
            );
        }
        #[test]
        fn at() {
            assert_eq!(
                external_timeout_decision(EXTERNAL_TOOL_CALL_TIMEOUT_MS),
                TimeoutDecision::TimedOut
            );
        }
        #[test]
        fn above() {
            assert_eq!(
                external_timeout_decision(EXTERNAL_TOOL_CALL_TIMEOUT_MS + 1),
                TimeoutDecision::TimedOut
            );
        }
    }
    mod approval_wait_ms_max {
        use super::*;
        #[test]
        fn below() {
            assert_eq!(
                approval_wait_decision(APPROVAL_WAIT_MS_MAX - 1, APPROVAL_WAIT_MS_MAX - 2),
                (APPROVAL_WAIT_MS_MAX - 1, TimeoutDecision::Pending)
            );
        }
        #[test]
        fn at() {
            assert_eq!(
                approval_wait_decision(APPROVAL_WAIT_MS_MAX, APPROVAL_WAIT_MS_MAX),
                (APPROVAL_WAIT_MS_MAX, TimeoutDecision::TimedOut)
            );
        }
        #[test]
        fn above() {
            assert_eq!(
                approval_wait_decision(APPROVAL_WAIT_MS_MAX + 1, APPROVAL_WAIT_MS_MAX),
                (APPROVAL_WAIT_MS_MAX, TimeoutDecision::TimedOut)
            );
        }
    }
    mod queue_pump_batch_max {
        use super::*;
        #[test]
        fn below() {
            assert_eq!(
                queue_pump_count(QUEUE_PUMP_BATCH_MAX - 1),
                QUEUE_PUMP_BATCH_MAX - 1
            );
        }
        #[test]
        fn at() {
            assert_eq!(queue_pump_count(QUEUE_PUMP_BATCH_MAX), QUEUE_PUMP_BATCH_MAX);
        }
        #[test]
        fn above() {
            assert_eq!(
                queue_pump_count(QUEUE_PUMP_BATCH_MAX + 1),
                QUEUE_PUMP_BATCH_MAX
            );
        }
    }

    #[test]
    fn steps_and_tools_are_distinct_owned_bounds() {
        assert_eq!(TURN_STEPS_BOUND.value, TURN_STEPS_MAX);
        assert_eq!(TURN_TOOL_CALLS_BOUND.value, TURN_TOOL_CALLS_MAX);
        assert_ne!(TURN_STEPS_BOUND.event, TURN_TOOL_CALLS_BOUND.event);
        assert_ne!(TURN_STEPS_BOUND.counter, TURN_TOOL_CALLS_BOUND.counter);
    }

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
    fn exact_eight_provider_rows_do_not_drift() {
        let expected = [
            (
                "PROVIDER_IMAGE_BYTES_MAX",
                20 * 1024 * 1024,
                "provider_image_byte_limit",
                "provider_image_byte_limit_total",
            ),
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
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum LoopClass {
        NamedBound(&'static str),
        BoundedIterator(&'static str),
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct LoopSite {
        file: String,
        header: String,
        occurrence: usize,
        adjacent: String,
    }

    mod bounds {
        use super::*;

        #[test]
        #[allow(
            clippy::too_many_lines,
            reason = "the complete loop manifest is explicit evidence"
        )]
        fn loops_assert_progress() {
            const MANIFEST: &[(&str, &str, usize, LoopClass)] = &[
                (
                    "approval/recovery.rs",
                    "for request in self.journal.port().list(scope)? {",
                    0,
                    LoopClass::BoundedIterator("list(scope)?"),
                ),
                (
                    "approval/resolve.rs",
                    "for request in self.journal.port().list(scope)? {",
                    0,
                    LoopClass::BoundedIterator("list(scope)?"),
                ),
                (
                    "boundary.rs",
                    "for component in path.components() {",
                    0,
                    LoopClass::BoundedIterator("path.components()"),
                ),
                (
                    "compaction.rs",
                    "while cutoff > 0 && splits_tool_protocol(messages, cutoff) {",
                    0,
                    LoopClass::NamedBound("cutoff -= 1"),
                ),
                (
                    "observe/events.rs",
                    "for sink in &self.sinks {",
                    0,
                    LoopClass::BoundedIterator("&self.sinks"),
                ),
                (
                    "observe/metrics.rs",
                    "for (bucket, upper) in histogram.2.iter_mut().zip(LATENCY_BUCKETS_MS) {",
                    0,
                    LoopClass::BoundedIterator("LATENCY_BUCKETS_MS"),
                ),
                (
                    "ports/provider.rs",
                    "for message in request.messages.as_slice() {",
                    0,
                    LoopClass::BoundedIterator("request.messages.as_slice()"),
                ),
                (
                    "ports/provider.rs",
                    "for part in message.content.as_slice() {",
                    0,
                    LoopClass::BoundedIterator("message.content.as_slice()"),
                ),
                (
                    "ports/provider.rs",
                    "for tool in request.tools.as_slice() {",
                    0,
                    LoopClass::BoundedIterator("request.tools.as_slice()"),
                ),
                (
                    "ports/provider.rs",
                    "for item in self.0 {",
                    0,
                    LoopClass::BoundedIterator("self.0"),
                ),
                (
                    "ports/provider.rs",
                    "for part in self.0 {",
                    0,
                    LoopClass::BoundedIterator("self.0"),
                ),
                (
                    "ports/provider.rs",
                    "for message in self.messages.as_slice() {",
                    0,
                    LoopClass::BoundedIterator("self.messages.as_slice()"),
                ),
                (
                    "ports/provider.rs",
                    "for part in message.content.as_slice() {",
                    1,
                    LoopClass::BoundedIterator("message.content.as_slice()"),
                ),
                (
                    "ports/provider.rs",
                    "for part in content.as_slice() {",
                    0,
                    LoopClass::BoundedIterator("content.as_slice()"),
                ),
                (
                    "ports/provider.rs",
                    "for input in values {",
                    0,
                    LoopClass::BoundedIterator("values"),
                ),
                (
                    "ports/provider.rs",
                    "while let Some(current) = work.pop() {",
                    0,
                    LoopClass::NamedBound("work.pop()"),
                ),
                (
                    "ports/provider.rs",
                    "while self.receiver.try_recv().is_ok() {}",
                    0,
                    LoopClass::NamedBound("try_recv()"),
                ),
                (
                    "ports/tool.rs",
                    "for segment in value[1..].split('/') {",
                    0,
                    LoopClass::BoundedIterator("value[1..].split('/')"),
                ),
                (
                    "ports/tool.rs",
                    "while index < bytes.len() {",
                    0,
                    LoopClass::NamedBound("index += 1"),
                ),
                (
                    "ports/tool.rs",
                    "for (index, field) in values.iter().enumerate() {",
                    0,
                    LoopClass::BoundedIterator("values.iter().enumerate()"),
                ),
                (
                    "ports/tool.rs",
                    "for (index, item) in required.iter().enumerate() {",
                    0,
                    LoopClass::BoundedIterator("required.iter().enumerate()"),
                ),
                (
                    "retry/executor.rs",
                    "for expected_attempt in 1..=attempts_max {",
                    0,
                    LoopClass::NamedBound("attempt_count = attempt_count.saturating_add(1)"),
                ),
                (
                    "retry/executor.rs",
                    "while !state.complete() {",
                    0,
                    LoopClass::NamedBound("progress_count"),
                ),
                (
                    "retry/executor.rs",
                    "for _ in 0..pending_items_max {",
                    0,
                    LoopClass::BoundedIterator("0..pending_items_max"),
                ),
                (
                    "retry/fallback.rs",
                    "while !detail.is_char_boundary(end) {",
                    0,
                    LoopClass::NamedBound("end -= 1"),
                ),
                (
                    "schedule/cron.rs",
                    "for _ in 0..SEARCH_MINUTES_MAX {",
                    0,
                    LoopClass::NamedBound("SEARCH_MINUTES_MAX"),
                ),
                (
                    "schedule/cron.rs",
                    "for part in field.split(',') {",
                    0,
                    LoopClass::BoundedIterator("field.split(',')"),
                ),
                (
                    "schedule/jitter.rs",
                    "loop {",
                    0,
                    LoopClass::NamedBound("source.next_u64()"),
                ),
                (
                    "schedule/scheduler.rs",
                    "loop {",
                    0,
                    LoopClass::NamedBound("cancellation.cancelled()"),
                ),
                (
                    "schedule/scheduler.rs",
                    "for task in &file.tasks {",
                    0,
                    LoopClass::BoundedIterator("&file.tasks"),
                ),
                (
                    "schedule/scheduler.rs",
                    "for pending in drain_due(&self.state, now.as_utc().timestamp_millis()) {",
                    0,
                    LoopClass::BoundedIterator(
                        "drain_due(&self.state, now.as_utc().timestamp_millis())",
                    ),
                ),
                (
                    "schedule/store.rs",
                    "for task in &self.tasks {",
                    0,
                    LoopClass::BoundedIterator("&self.tasks"),
                ),
                (
                    "turn/cancel.rs",
                    "for call in &self.calls {",
                    0,
                    LoopClass::BoundedIterator("&self.calls"),
                ),
                (
                    "turn/cancel.rs",
                    "for call in &self.calls {",
                    1,
                    LoopClass::BoundedIterator("&self.calls"),
                ),
                (
                    "turn/cancel.rs",
                    "for call_id in context.unfinished.drain_pending() {",
                    0,
                    LoopClass::BoundedIterator("drain_pending()"),
                ),
                (
                    "turn/tool_calls.rs",
                    "for definition in definitions {",
                    0,
                    LoopClass::BoundedIterator("definitions"),
                ),
                (
                    "turn/loop.rs",
                    "for candidate in self .fallbacks .iter() .zip(fallbacks.iter().copied()) .map(Some) .chain(std::iter::once(None)) {",
                    0,
                    LoopClass::BoundedIterator("fallbacks.iter().copied()"),
                ),
                (
                    "turn/loop.rs",
                    "for step_index in 0..TURN_STEPS_MAX {",
                    0,
                    LoopClass::NamedBound("step_decision(step_index)"),
                ),
                (
                    "turn/loop.rs",
                    "loop {",
                    0,
                    LoopClass::NamedBound("compact_and_refresh"),
                ),
                (
                    "turn/loop.rs",
                    "loop {",
                    1,
                    LoopClass::NamedBound("future => break result"),
                ),
                (
                    "turn/loop.rs",
                    "loop {",
                    2,
                    LoopClass::NamedBound("receiver.try_recv()"),
                ),
                (
                    "turn/loop.rs",
                    "while let Some(event) = receive_retry(receiver).await {",
                    0,
                    LoopClass::NamedBound("receive_retry(receiver).await"),
                ),
                (
                    "turn/loop.rs",
                    "while let Some(event) = output.receive().await? {",
                    0,
                    LoopClass::NamedBound("output.receive().await?"),
                ),
                (
                    "turn/loop.rs",
                    "for result in completed {",
                    0,
                    LoopClass::BoundedIterator("completed"),
                ),
                (
                    "worktree_watcher.rs",
                    "loop {",
                    0,
                    LoopClass::NamedBound("WORKTREE_WATCHER_IDLE_STOP_MS"),
                ),
            ];
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
            let files = production_modules(&root);
            let mut actual = Vec::new();
            for file in files {
                actual.extend(scan_production_loops(&root, &file));
            }
            let expected: Vec<_> = MANIFEST
                .iter()
                .map(|(file, header, occurrence, _)| {
                    ((*file).to_owned(), normalized_header(header), *occurrence)
                })
                .collect();
            let found: Vec<_> = actual
                .iter()
                .map(|site| (site.file.clone(), site.header.clone(), site.occurrence))
                .collect();
            assert_eq!(
                found, expected,
                "production loop manifest must be bijective; new loops require classification"
            );
            for (site, (_, _, _, class)) in actual.iter().zip(MANIFEST) {
                let token = match class {
                    LoopClass::NamedBound(token) | LoopClass::BoundedIterator(token) => token,
                };
                let token = token.parse::<proc_macro2::TokenStream>().map_or_else(
                    |_| normalize(token),
                    |tokens| normalize(&tokens.to_string()),
                );
                assert!(
                    site.adjacent.contains(&token),
                    "loop classification token must be adjacent at {}#{}: {token}",
                    site.file,
                    site.occurrence
                );
            }
        }

        fn production_modules(root: &std::path::Path) -> Vec<std::path::PathBuf> {
            fn visit(path: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
                if files.iter().any(|file| file == path) {
                    return;
                }
                files.push(path.to_path_buf());
                let source = std::fs::read_to_string(path).expect("production module");
                let lines: Vec<_> = source.lines().collect();
                for index in 0..lines.len() {
                    let line = lines[index].trim();
                    if !line.contains("mod ") || !line.ends_with(';') {
                        continue;
                    }
                    let prior = lines[..index]
                        .iter()
                        .rev()
                        .take_while(|line| line.trim().starts_with("#["))
                        .map(|line| line.trim())
                        .collect::<Vec<_>>();
                    if prior
                        .iter()
                        .any(|attr| attr.replace(' ', "").contains("cfg(test)"))
                    {
                        continue;
                    }
                    let Some(name) = line
                        .split("mod ")
                        .nth(1)
                        .and_then(|tail| tail.split(';').next())
                        .map(str::trim)
                    else {
                        continue;
                    };
                    let explicit = prior.iter().find_map(|attr| {
                        attr.split("path = \"")
                            .nth(1)
                            .and_then(|tail| tail.split('"').next())
                    });
                    let child = if let Some(relative) = explicit {
                        path.parent().expect("module parent").join(relative)
                    } else {
                        let base = if matches!(
                            path.file_name().and_then(|value| value.to_str()),
                            Some("lib.rs" | "mod.rs")
                        ) {
                            path.parent().expect("module parent").to_path_buf()
                        } else {
                            path.with_extension("")
                        };
                        let flat = base.join(name).with_extension("rs");
                        if flat.exists() {
                            flat
                        } else {
                            base.join(name).join("mod.rs")
                        }
                    };
                    if child.exists() {
                        visit(&child, files);
                    }
                }
            }
            let mut files = Vec::new();
            visit(&root.join("lib.rs"), &mut files);
            files
        }

        fn scan_production_loops(root: &std::path::Path, path: &std::path::Path) -> Vec<LoopSite> {
            struct Visitor<'a> {
                file: &'a str,
                sites: Vec<LoopSite>,
                counts: std::collections::BTreeMap<String, usize>,
            }
            impl syn::visit::Visit<'_> for Visitor<'_> {
                fn visit_expr_for_loop(&mut self, node: &syn::ExprForLoop) {
                    let header = normalize(&format!(
                        "for {} in {} {{",
                        node.pat.to_token_stream(),
                        node.expr.to_token_stream()
                    ));
                    self.push(header, &node.to_token_stream().to_string());
                    syn::visit::visit_expr_for_loop(self, node);
                }
                fn visit_expr_while(&mut self, node: &syn::ExprWhile) {
                    let header = normalize(&format!("while {} {{", node.cond.to_token_stream()));
                    self.push(header, &node.to_token_stream().to_string());
                    syn::visit::visit_expr_while(self, node);
                }
                fn visit_expr_loop(&mut self, node: &syn::ExprLoop) {
                    self.push("loop {".to_owned(), &node.to_token_stream().to_string());
                    syn::visit::visit_expr_loop(self, node);
                }
            }
            impl Visitor<'_> {
                fn push(&mut self, header: String, body: &str) {
                    let occurrence = self.counts.entry(header.clone()).or_insert(0);
                    self.sites.push(LoopSite {
                        file: self.file.to_owned(),
                        header,
                        occurrence: *occurrence,
                        adjacent: normalize(body),
                    });
                    *occurrence += 1;
                }
            }
            let source = std::fs::read_to_string(path).expect("production source");
            let mut parsed = syn::parse_file(&source).expect("valid Rust source");
            parsed.items.retain(|item| !has_test_cfg(item_attrs(item)));
            let file = path
                .strip_prefix(root)
                .expect("relative module")
                .to_string_lossy()
                .replace('\\', "/");
            let mut visitor = Visitor {
                file: &file,
                sites: Vec::new(),
                counts: std::collections::BTreeMap::new(),
            };
            syn::visit::Visit::visit_file(&mut visitor, &parsed);
            visitor.sites
        }

        fn normalize(value: &str) -> String {
            value.split_whitespace().collect::<Vec<_>>().join(" ")
        }

        #[allow(
            clippy::items_after_statements,
            reason = "local visitor is scoped to selector parsing"
        )]
        fn normalized_header(value: &str) -> String {
            let trimmed = value.trim();
            let expression = if let Some(value) = trimmed.strip_suffix("{}") {
                value.trim()
            } else {
                trimmed
                    .strip_suffix('{')
                    .expect("manifest loop header")
                    .trim()
            };
            if expression == "loop" {
                return "loop {".to_owned();
            }
            let wrapper = format!("fn evidence() {{ {expression} {{}} }}");
            let file = syn::parse_file(&wrapper).expect("literal manifest selector");
            struct Header(Option<String>);
            impl syn::visit::Visit<'_> for Header {
                fn visit_expr_for_loop(&mut self, node: &syn::ExprForLoop) {
                    self.0 = Some(normalize(&format!(
                        "for {} in {} {{",
                        node.pat.to_token_stream(),
                        node.expr.to_token_stream()
                    )));
                }
                fn visit_expr_while(&mut self, node: &syn::ExprWhile) {
                    self.0 = Some(normalize(&format!(
                        "while {} {{",
                        node.cond.to_token_stream()
                    )));
                }
            }
            let mut header = Header(None);
            syn::visit::Visit::visit_file(&mut header, &file);
            header.0.expect("manifest loop selector")
        }
        fn has_test_cfg(attrs: &[syn::Attribute]) -> bool {
            attrs.iter().any(|attr| {
                attr.path().is_ident("cfg")
                    && attr
                        .meta
                        .to_token_stream()
                        .to_string()
                        .replace(' ', "")
                        .contains("test")
            })
        }
        fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
            match item {
                syn::Item::Const(v) => &v.attrs,
                syn::Item::Enum(v) => &v.attrs,
                syn::Item::ExternCrate(v) => &v.attrs,
                syn::Item::Fn(v) => &v.attrs,
                syn::Item::ForeignMod(v) => &v.attrs,
                syn::Item::Impl(v) => &v.attrs,
                syn::Item::Macro(v) => &v.attrs,
                syn::Item::Mod(v) => &v.attrs,
                syn::Item::Static(v) => &v.attrs,
                syn::Item::Struct(v) => &v.attrs,
                syn::Item::Trait(v) => &v.attrs,
                syn::Item::TraitAlias(v) => &v.attrs,
                syn::Item::Type(v) => &v.attrs,
                syn::Item::Union(v) => &v.attrs,
                syn::Item::Use(v) => &v.attrs,
                _ => &[],
            }
        }

        mod hardening_does_not_affect_baseline {
            use super::*;
            use serde_json::Value;

            #[derive(Clone, Debug, Default, Eq, PartialEq)]
            struct ObservedHardeningUsage {
                tool_lifecycle_messages: usize,
                max_turn_steps: usize,
                abort_reached_turn_finished: bool,
                child_kill_or_timeout_frames: usize,
                ordinary_child_operations: usize,
                local_tool_operations: usize,
                local_timeout_frames: usize,
                local_completed_before_timeout: bool,
                external_tool_operations: usize,
                external_timeout_frames: usize,
                external_completed_before_timeout: bool,
                approval_requests: usize,
                approval_expiries: usize,
                max_consecutive_dequeued: usize,
                max_queue_depth: usize,
            }

            fn trace(name: &str) -> Value {
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/reference-traces")
                    .join(name);
                serde_json::from_slice(&std::fs::read(path).expect("trace bytes"))
                    .expect("actual trace JSON")
            }

            fn wire_type(frame: &Value) -> &str {
                frame["wire"]["type"].as_str().unwrap_or_default()
            }

            fn message_type(frame: &Value) -> &str {
                frame["wire"]["message_type"]
                    .as_str()
                    .or_else(|| frame["wire"]["message"]["message_type"].as_str())
                    .or_else(|| frame["wire"]["payload"]["message_type"].as_str())
                    .unwrap_or_default()
            }

            fn all_frames() -> Vec<Value> {
                let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/reference-traces");
                let mut paths: Vec<_> = std::fs::read_dir(root)
                    .expect("reference traces")
                    .map(|entry| entry.expect("trace entry").path())
                    .filter(|path| {
                        path.file_name().and_then(|name| name.to_str()) != Some("index.json")
                    })
                    .collect();
                paths.sort();
                paths
                    .into_iter()
                    .flat_map(|path| {
                        let value: Value =
                            serde_json::from_slice(&std::fs::read(path).expect("trace bytes"))
                                .expect("trace JSON");
                        value["frames"].as_array().expect("trace frames").clone()
                    })
                    .collect()
            }

            fn is_tool_start(kind: &str) -> bool {
                matches!(
                    kind,
                    "client_tool_start" | "server_tool_start" | "tool_call_start" | "tool_started"
                )
            }

            fn is_tool_end(kind: &str) -> bool {
                matches!(
                    kind,
                    "client_tool_end" | "server_tool_end" | "tool_call_end" | "tool_finished"
                )
            }

            fn tool_usage() -> ObservedHardeningUsage {
                let mut usage = ObservedHardeningUsage::default();
                for frame in all_frames() {
                    let kind = message_type(&frame);
                    if is_tool_start(kind) || is_tool_end(kind) {
                        usage.tool_lifecycle_messages += 1;
                    }
                }
                usage
            }

            fn step_usage() -> ObservedHardeningUsage {
                let mut usage = ObservedHardeningUsage::default();
                let mut current = 0;
                for frame in all_frames() {
                    let kind = message_type(&frame);
                    if matches!(kind, "stream_delta" | "retry")
                        || is_tool_start(kind)
                        || is_tool_end(kind)
                    {
                        current += 1;
                    }
                    if wire_type(&frame) == "turn_finished" || kind == "turn_finished" {
                        usage.max_turn_steps = usage.max_turn_steps.max(current);
                        current = 0;
                    }
                }
                usage
            }

            fn cancel_usage() -> ObservedHardeningUsage {
                let value = trace("abort.json");
                let frames = value["frames"].as_array().expect("abort frames");
                let abort = frames
                    .iter()
                    .position(|frame| wire_type(frame) == "abort_message")
                    .expect("abort command");
                let terminal = frames
                    .iter()
                    .position(|frame| wire_type(frame) == "turn_finished")
                    .expect("abort terminal");
                let mut usage = ObservedHardeningUsage {
                    abort_reached_turn_finished: abort < terminal,
                    ..ObservedHardeningUsage::default()
                };
                for frame in &frames[abort..=terminal] {
                    let kind = wire_type(frame);
                    if kind.contains("child_kill") || kind.contains("timeout") {
                        usage.child_kill_or_timeout_frames += 1;
                    }
                    if kind.contains("child_start") || kind.contains("process_start") {
                        usage.ordinary_child_operations += 1;
                    }
                }
                usage
            }

            fn timeout_usage() -> ObservedHardeningUsage {
                let mut usage = ObservedHardeningUsage::default();
                for frame in all_frames() {
                    let kind = message_type(&frame);
                    let wire = wire_type(&frame);
                    if matches!(kind, "server_tool_start" | "tool_started") {
                        usage.local_tool_operations += 1;
                    }
                    if matches!(kind, "client_tool_start" | "tool_call_start") {
                        usage.external_tool_operations += 1;
                    }
                    if wire.contains("local_tool_timeout") || kind == "local_tool_timeout" {
                        usage.local_timeout_frames += 1;
                    }
                    if wire.contains("external_tool_timeout") || kind == "external_tool_timeout" {
                        usage.external_timeout_frames += 1;
                    }
                }
                usage.local_completed_before_timeout =
                    usage.local_tool_operations > 0 && usage.local_timeout_frames == 0;
                usage.external_completed_before_timeout =
                    usage.external_tool_operations > 0 && usage.external_timeout_frames == 0;
                usage
            }

            fn approval_usage() -> ObservedHardeningUsage {
                let mut usage = ObservedHardeningUsage::default();
                for frame in all_frames() {
                    let kind = message_type(&frame);
                    if matches!(kind, "approval_request" | "approval_requested") {
                        usage.approval_requests += 1;
                    }
                    if matches!(kind, "approval_expired" | "approval_expiry") {
                        usage.approval_expiries += 1;
                    }
                }
                usage
            }

            fn queue_usage() -> ObservedHardeningUsage {
                let value = trace("queue.json");
                let mut usage = ObservedHardeningUsage::default();
                let mut consecutive = 0;
                for frame in value["frames"].as_array().expect("queue frames") {
                    if wire_type(frame) == "turn_dequeued" {
                        consecutive += 1;
                    } else if !matches!(wire_type(frame), "broadcast_begin" | "lease_acquired") {
                        consecutive = 0;
                    }
                    usage.max_consecutive_dequeued =
                        usage.max_consecutive_dequeued.max(consecutive);
                    if wire_type(frame) == "update_queue" {
                        usage.max_queue_depth = usage
                            .max_queue_depth
                            .max(frame["wire"]["queue"].as_array().map_or(0, Vec::len));
                    }
                }
                usage
            }

            #[test]
            fn tool_calls() {
                let usage = tool_usage();
                assert!(
                    usage.tool_lifecycle_messages <= TURN_TOOL_CALLS_MAX * 2,
                    "{usage:?}"
                );
            }
            #[test]
            fn steps() {
                let usage = step_usage();
                assert!(usage.max_turn_steps < TURN_STEPS_MAX, "{usage:?}");
            }
            #[test]
            fn cancel_grace_clock() {
                let usage = cancel_usage();
                assert!(usage.abort_reached_turn_finished, "{usage:?}");
                assert_eq!(usage.child_kill_or_timeout_frames, 0, "{usage:?}");
                assert_eq!(usage.ordinary_child_operations, 0, "{usage:?}");
            }
            #[test]
            fn local_timeout() {
                let usage = timeout_usage();
                assert_eq!(usage.local_timeout_frames, 0, "{usage:?}");
                assert!(
                    usage.local_tool_operations == 0 || usage.local_completed_before_timeout,
                    "{usage:?}"
                );
            }
            #[test]
            fn external_timeout() {
                let usage = timeout_usage();
                assert_eq!(usage.external_timeout_frames, 0, "{usage:?}");
                assert!(
                    usage.external_tool_operations == 0 || usage.external_completed_before_timeout,
                    "{usage:?}"
                );
            }
            #[test]
            fn approval_wait() {
                let usage = approval_usage();
                assert_eq!(usage.approval_expiries, 0, "{usage:?}");
            }
            #[test]
            fn pump_batch() {
                let usage = queue_usage();
                assert!(
                    usage.max_consecutive_dequeued < QUEUE_PUMP_BATCH_MAX,
                    "{usage:?}"
                );
                assert!(usage.max_queue_depth < QUEUE_PUMP_BATCH_MAX, "{usage:?}");
            }
        }
    }
}

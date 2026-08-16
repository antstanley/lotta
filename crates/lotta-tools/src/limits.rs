//! Named tool-pipeline resource limits and allocation-free admission checks.

use serde::Serialize;
use std::io::{self, Write};

pub use lotta_runtime::bounds::{
    TOOL_INPUT_BYTES_MAX, TOOL_RESULT_BYTES_MAX, TOOL_RESULT_MODEL_CHARS_MAX,
};

/// Maximum combined output accepted from one child process, in bytes.
pub const CHILD_PROCESS_OUTPUT_BYTES_MAX: usize = 16 * 1024 * 1024;
/// Maximum aggregate resolved secret bytes for one invocation, in bytes.
pub const SECRET_DELIVERY_BYTES_MAX: usize = TOOL_INPUT_BYTES_MAX.value;

/// Fixed resource-limit failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitError {
    /// Tool input exceeded its compact JSON byte limit.
    ToolInput,
    /// Raw tool result exceeded its byte limit.
    ToolResult,
    /// Child output exceeded its cumulative byte limit.
    ChildProcessOutput,
}

/// Checks an already measured compact tool input length.
///
/// # Errors
/// Returns [`LimitError::ToolInput`] above the named input limit.
pub fn check_tool_input(bytes: usize) -> Result<(), LimitError> {
    check(bytes, TOOL_INPUT_BYTES_MAX.value, LimitError::ToolInput)
}

/// Measures compact JSON through a streaming counter without retaining an encoded copy.
///
/// # Errors
/// Returns [`LimitError::ToolInput`] if serialization fails or exceeds the input limit.
pub fn check_tool_input_value<T: Serialize>(value: &T) -> Result<(), LimitError> {
    let mut counter = Counter { count: 0 };
    serde_json::to_writer(&mut counter, value).map_err(|_| LimitError::ToolInput)
}

/// Checks an already measured raw tool result length.
///
/// # Errors
/// Returns [`LimitError::ToolResult`] above the named result limit.
pub fn check_tool_result(bytes: usize) -> Result<(), LimitError> {
    check(bytes, TOOL_RESULT_BYTES_MAX.value, LimitError::ToolResult)
}

/// Computes the next retained child-output length without mutating caller state on failure.
///
/// # Errors
/// Returns [`LimitError::ChildProcessOutput`] on arithmetic overflow or above the named limit.
pub fn check_child_output(current: usize, added: usize) -> Result<usize, LimitError> {
    let next = current
        .checked_add(added)
        .ok_or(LimitError::ChildProcessOutput)?;
    check(
        next,
        CHILD_PROCESS_OUTPUT_BYTES_MAX,
        LimitError::ChildProcessOutput,
    )?;
    Ok(next)
}

fn check(value: usize, max: usize, error: LimitError) -> Result<(), LimitError> {
    if value > max { Err(error) } else { Ok(()) }
}

struct Counter {
    count: usize,
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .count
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("tool input size overflow"))?;
        if next > TOOL_INPUT_BYTES_MAX.value {
            return Err(io::Error::other("tool input size limit"));
        }
        self.count = next;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_input_below() {
        assert!(check_tool_input(TOOL_INPUT_BYTES_MAX.value - 1).is_ok());
    }
    #[test]
    fn tool_input_at() {
        assert!(check_tool_input(TOOL_INPUT_BYTES_MAX.value).is_ok());
    }
    #[test]
    fn tool_input_above() {
        assert_eq!(
            check_tool_input(TOOL_INPUT_BYTES_MAX.value + 1),
            Err(LimitError::ToolInput)
        );
    }
    #[test]
    fn tool_result_below() {
        assert!(check_tool_result(TOOL_RESULT_BYTES_MAX.value - 1).is_ok());
    }
    #[test]
    fn tool_result_at() {
        assert!(check_tool_result(TOOL_RESULT_BYTES_MAX.value).is_ok());
    }
    #[test]
    fn tool_result_above() {
        assert_eq!(
            check_tool_result(TOOL_RESULT_BYTES_MAX.value + 1),
            Err(LimitError::ToolResult)
        );
    }
    #[test]
    fn child_process_output_below() {
        assert_eq!(
            check_child_output(CHILD_PROCESS_OUTPUT_BYTES_MAX - 1, 0),
            Ok(CHILD_PROCESS_OUTPUT_BYTES_MAX - 1)
        );
    }
    #[test]
    fn child_process_output_at() {
        assert_eq!(
            check_child_output(CHILD_PROCESS_OUTPUT_BYTES_MAX - 1, 1),
            Ok(CHILD_PROCESS_OUTPUT_BYTES_MAX)
        );
    }
    #[test]
    fn child_process_output_above() {
        assert_eq!(
            check_child_output(CHILD_PROCESS_OUTPUT_BYTES_MAX, 1),
            Err(LimitError::ChildProcessOutput)
        );
        assert_eq!(
            check_child_output(usize::MAX, 1),
            Err(LimitError::ChildProcessOutput)
        );
    }
}
